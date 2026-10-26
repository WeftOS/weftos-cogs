//! A service call becomes a queued job, or an immediate refusal. The body is
//! the `infer.v1` request (weftos `contracts/cogs`): the same rules, checked
//! again here because the broker validates only the schema id.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::backend::{BlobRef, Request};
use crate::catalog::{Catalog, Entry, Kind};
use crate::scheduler::{Job, ReplyFn, Scheduler};

pub const DEFAULT_PRIORITY: u8 = 4;
const MAX_TEXT: usize = 32 * 1024;
const MAX_IMAGE_B64: usize = 40 * 1024;
const DEFAULT_MAX_TOKENS: u32 = 256;
/// Largest image read from a blob.
pub const MAX_IMAGE_BLOB: u64 = 4 * 1024 * 1024;

#[derive(Debug)]
pub struct Parsed {
    pub entry: Entry,
    pub req: Request,
    pub priority: u8,
}

fn opt_str<'a>(body: &'a Value, k: &str) -> Result<Option<&'a str>, String> {
    match body.get(k) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s)),
        Some(_) => Err(format!("{k} must be a string")),
    }
}

/// The `image_blob` field: a `blob.v1` handle for a PNG or JPEG of at most
/// [`MAX_IMAGE_BLOB`] bytes, for a vision model, not yet expired.
fn parse_blob(v: Option<&Value>, alias: &str, kind: Kind) -> Result<Option<BlobRef>, String> {
    let Some(v) = v.filter(|v| !v.is_null()) else {
        return Ok(None);
    };
    let o = v
        .as_object()
        .ok_or("image_blob must be a blob.v1 handle object")?;
    let hash = o.get("hash").and_then(Value::as_str).unwrap_or_default();
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err("image_blob.hash must be 64 lower-case hex chars".into());
    }
    let size = o
        .get("size")
        .and_then(Value::as_u64)
        .filter(|s| *s > 0)
        .ok_or("image_blob.size must be at least 1")?;
    if size > MAX_IMAGE_BLOB {
        return Err(format!(
            "image_blob is {size} bytes; at most {MAX_IMAGE_BLOB} are read"
        ));
    }
    let media = o
        .get("media_type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if media != "image/png" && media != "image/jpeg" {
        return Err("image_blob.media_type must be image/png or image/jpeg".into());
    }
    let expires = o
        .get("expires_ms")
        .and_then(Value::as_u64)
        .filter(|e| *e > 0)
        .ok_or("image_blob.expires_ms must be set")?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64);
    if expires <= now {
        return Err("image_blob has expired".into());
    }
    if kind != Kind::Vlm {
        return Err(format!("{alias} does not take images; use a vision model"));
    }
    Ok(Some(BlobRef {
        hash: hash.to_string(),
        size,
        media_type: media.to_string(),
    }))
}

/// Check `body` and pick the model. Every refusal says what to change.
pub fn parse(catalog: &Catalog, body: &Value) -> Result<Parsed, String> {
    let obj = body
        .as_object()
        .ok_or("the request must be a JSON object")?;
    let name = opt_str(body, "model")?
        .filter(|m| !m.is_empty() && m.len() <= 128)
        .ok_or("model is required (1..=128 chars)")?;
    let entry = catalog.resolve(name).ok_or_else(|| {
        let have: Vec<&str> = catalog.entries().iter().map(|e| e.alias.as_str()).collect();
        format!(
            "unknown model {name:?}; this node serves: {}",
            have.join(", ")
        )
    })?;
    let priority = match obj.get("priority") {
        None | Some(Value::Null) => DEFAULT_PRIORITY,
        Some(v) => v
            .as_u64()
            .filter(|p| *p <= 9)
            .ok_or("priority must be 0..=9")? as u8,
    };
    let text = opt_str(body, "text")?;
    let system = opt_str(body, "system")?;
    for (k, v) in [("text", text), ("system", system)] {
        if v.is_some_and(|s| s.len() > MAX_TEXT) {
            return Err(format!("{k} is longer than {MAX_TEXT} bytes"));
        }
    }
    let blob = parse_blob(obj.get("image_blob"), &entry.alias, entry.kind)?;
    if blob.is_some()
        && (obj.get("image_b64").is_some_and(|v| !v.is_null())
            || obj.get("image_mime").is_some_and(|v| !v.is_null()))
    {
        return Err("give an image as image_b64 or as image_blob, not both".into());
    }
    let image = match (opt_str(body, "image_b64")?, opt_str(body, "image_mime")?) {
        (None, None) => None,
        (Some(b), Some(m)) => {
            if b.len() > MAX_IMAGE_B64 {
                return Err(format!("image_b64 is longer than {MAX_IMAGE_B64} chars; larger images need a blob handle"));
            }
            if m != "image/png" && m != "image/jpeg" {
                return Err("image_mime must be image/png or image/jpeg".into());
            }
            if !b
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || b"+/=".contains(&c))
            {
                return Err("image_b64 is not base64".into());
            }
            if entry.kind != Kind::Vlm {
                return Err(format!(
                    "{} does not take images; use a vision model",
                    entry.alias
                ));
            }
            Some((m.to_string(), b.to_string()))
        }
        _ => return Err("image_b64 and image_mime go together".into()),
    };
    if text.is_none() && image.is_none() && blob.is_none() {
        return Err("text or image_b64 is required".into());
    }
    let max_tokens = match obj.get("max_tokens") {
        None | Some(Value::Null) => DEFAULT_MAX_TOKENS,
        Some(v) => v
            .as_u64()
            .filter(|n| (1..=4096).contains(n))
            .ok_or("max_tokens must be 1..=4096")? as u32,
    };
    Ok(Parsed {
        entry: entry.clone(),
        req: Request {
            system: system.map(String::from),
            text: text.map(String::from),
            image,
            image_blob: blob,
            max_tokens,
        },
        priority,
    })
}

/// Queue the call, or answer it at once. `timeout` is what the broker gave
/// the caller; the job is dropped unrun once it has passed.
pub fn submit(
    catalog: &Catalog,
    sched: &Scheduler,
    body: &Value,
    timeout: Duration,
    reply: ReplyFn,
) {
    let p = match parse(catalog, body) {
        Ok(p) => p,
        Err(m) => return reply(Err(m)),
    };
    let now = Instant::now();
    let job = Job {
        entry: p.entry,
        req: p.req,
        priority: p.priority,
        enq: now,
        deadline: now + timeout,
        reply,
    };
    if let Err(job) = sched.submit(job) {
        (job.reply)(Err("busy: the queue is full".into()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn c() -> Catalog {
        Catalog::ventuno()
    }

    #[test]
    fn a_good_request_is_parsed_with_defaults() {
        let p = parse(&c(), &json!({"model": "qwen3-4b", "text": "hi"})).unwrap();
        assert_eq!(
            (p.entry.alias.as_str(), p.priority, p.req.max_tokens),
            ("qwen3-4b", 4, 256)
        );
        let p = parse(&c(), &json!({"model": "unsloth/gemma-3-1b-it-GGUF:Q4_0", "text": "hi", "priority": 9, "max_tokens": 5, "system": "s"})).unwrap();
        assert_eq!(
            (p.entry.alias.as_str(), p.priority, p.req.max_tokens),
            ("gemma3-1b", 9, 5)
        );
    }

    fn handle(media: &str, size: u64, expires: u64) -> Value {
        json!({"hash": "ab".repeat(32), "size": size, "media_type": media, "expires_ms": expires})
    }

    fn far() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
            + 600_000
    }

    #[test]
    fn an_image_can_arrive_as_a_blob_handle_for_a_vision_model() {
        let ok = |h: Value| {
            parse(
                &c(),
                &json!({"model": "qwen3-vl-4b", "text": "what?", "image_blob": h}),
            )
        };
        let p = ok(handle("image/jpeg", 200_000, far())).unwrap();
        let b = p.req.image_blob.unwrap();
        assert_eq!(
            (b.size, b.media_type.as_str(), b.hash.len()),
            (200_000, "image/jpeg", 64)
        );
        assert!(p.req.image.is_none());
        // a handle alone is enough input
        assert!(parse(
            &c(),
            &json!({"model": "qwen3-vl-4b", "image_blob": handle("image/png", 5, far())})
        )
        .is_ok());
        for (bad, why) in [
            (handle("image/gif", 5, far()), "media_type"),
            (handle("image/png", 0, far()), "size"),
            (handle("image/png", MAX_IMAGE_BLOB + 1, far()), "at most"),
            (handle("image/png", 5, 1), "expired"),
            (
                json!({"hash": "xyz", "size": 5, "media_type": "image/png", "expires_ms": far()}),
                "hash",
            ),
            (json!("hash"), "object"),
        ] {
            let e = ok(bad).unwrap_err();
            assert!(e.contains(why), "{why}: {e}");
        }
    }

    #[test]
    fn a_blob_goes_only_to_a_vision_model_and_not_beside_an_inline_image() {
        let e = parse(
            &c(),
            &json!({"model": "qwen3-4b", "text": "x", "image_blob": handle("image/png", 5, far())}),
        )
        .unwrap_err();
        assert!(e.contains("does not take images"), "{e}");
        let both = json!({"model": "qwen3-vl-4b", "text": "x", "image_blob": handle("image/png", 5, far()),
            "image_b64": "AAAA", "image_mime": "image/png"});
        assert!(parse(&c(), &both).unwrap_err().contains("not both"));
    }

    #[test]
    fn an_unknown_model_is_refused_with_the_list() {
        let e = parse(&c(), &json!({"model": "gemma4-e4b", "text": "hi"})).unwrap_err();
        assert!(
            e.contains("unknown model") && e.contains("gemma3-1b"),
            "{e}"
        );
        assert!(parse(&c(), &json!({"text": "hi"})).is_err());
        assert!(parse(&c(), &json!({"model": 3, "text": "hi"})).is_err());
        assert!(parse(&c(), &json!("hi")).is_err());
    }

    #[test]
    fn the_infer_v1_rules_are_kept() {
        let m = |extra: Value| {
            let mut b = json!({"model": "qwen3-vl-4b", "text": "hi"});
            b.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            parse(&c(), &b)
        };
        assert!(m(json!({"priority": 10})).is_err());
        assert!(m(json!({"priority": "high"})).is_err());
        assert!(m(json!({"max_tokens": 0})).is_err());
        assert!(m(json!({"max_tokens": 5000})).is_err());
        assert!(
            m(json!({"image_b64": "AAAA"})).is_err(),
            "mime goes with the image"
        );
        assert!(m(json!({"image_b64": "AAAA", "image_mime": "image/gif"})).is_err());
        assert!(m(json!({"image_b64": "AA!A", "image_mime": "image/png"})).is_err());
        assert!(m(json!({"image_b64": "A".repeat(41 * 1024), "image_mime": "image/png"})).is_err());
        assert!(m(json!({"image_b64": "AAAA", "image_mime": "image/png"})).is_ok());
        assert!(m(json!({"text": "x".repeat(33 * 1024)})).is_err());
        assert!(
            parse(&c(), &json!({"model": "qwen3-vl-4b"})).is_err(),
            "text or image"
        );
        let e = parse(
            &c(),
            &json!({"model": "qwen3-4b", "image_b64": "AAAA", "image_mime": "image/png"}),
        )
        .unwrap_err();
        assert!(e.contains("does not take images"), "{e}");
    }

    #[test]
    fn a_bad_call_is_answered_at_once_and_a_full_queue_says_busy() {
        use std::sync::mpsc;
        let (tx, rx) = mpsc::channel();
        let s = Scheduler::new(1, 1000);
        let mk = |tx: &mpsc::Sender<Result<Value, String>>| -> ReplyFn {
            let tx = tx.clone();
            Box::new(move |r| {
                let _ = tx.send(r);
            })
        };
        submit(
            &c(),
            &s,
            &json!({"model": "nope", "text": "x"}),
            Duration::from_secs(1),
            mk(&tx),
        );
        assert!(rx
            .try_recv()
            .unwrap()
            .unwrap_err()
            .contains("unknown model"));
        submit(
            &c(),
            &s,
            &json!({"model": "gemma3-1b", "text": "x"}),
            Duration::from_secs(1),
            mk(&tx),
        );
        assert!(rx.try_recv().is_err(), "queued, no worker yet");
        submit(
            &c(),
            &s,
            &json!({"model": "gemma3-1b", "text": "x"}),
            Duration::from_secs(1),
            mk(&tx),
        );
        assert!(rx.try_recv().unwrap().unwrap_err().starts_with("busy"));
    }
}
