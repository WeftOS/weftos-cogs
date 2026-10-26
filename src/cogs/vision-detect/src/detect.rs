//! What the vision model is asked and how its answer becomes `detections.v1` boxes.
//!
//! The model answers in text, so the prompt asks for a JSON list and the parser takes what it
//! can use: the first JSON array in the text (inside a code fence or not, or under a
//! `detections` / `objects` key), each item a label and, when the model gave one, a box
//! `[x, y, w, h]` normalized to the frame. An item with a label and no usable box becomes a
//! whole-frame box: the label is true of the frame, the position is not known.

use serde_json::{json, Value};

pub const SYSTEM: &str = "You are the vision system of a small embedded board. You are shown one camera frame. \
Reply with only a JSON array of the objects you can see. Each item is {\"label\": a short noun, \
\"bbox\": [x, y, w, h], \"score\": 0 to 1}. x, y, w, h are fractions of the frame width and height, \
from the top left corner. Give at most 10 items. If you see nothing clear, reply [].";

pub const PROMPT: &str = "List the objects in this frame as the JSON array described.";

pub const MAX_BOXES: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub label: String,
    pub score: f64,
    pub bbox: [f64; 4],
    /// The model gave no usable box; `bbox` is the whole frame.
    pub whole_frame: bool,
}

/// The first JSON array (or the list under a `detections`/`objects`/`items` key) in `text`.
fn find_list(text: &str) -> Option<Vec<Value>> {
    let try_parse = |s: &str| -> Option<Vec<Value>> {
        let list = match serde_json::from_str::<Value>(s.trim()).ok()? {
            Value::Array(a) => Some(a),
            Value::Object(m) => ["detections", "objects", "items", "boxes"]
                .iter()
                .find_map(|k| m.get(*k).and_then(|v| v.as_array().cloned())),
            _ => None,
        }?;
        // An array of numbers in the model's reasoning is not the answer.
        (list.is_empty() || list.iter().any(Value::is_object)).then_some(list)
    };
    // Whole text, then each balanced candidate starting at '[' or '{'.
    if let Some(a) = try_parse(text) {
        return Some(a);
    }
    for (open, close) in [('[', ']'), ('{', '}')] {
        let mut from = 0;
        while let Some(i) = text[from..].find(open) {
            let start = from + i;
            let mut depth = 0i32;
            for (j, c) in text[start..].char_indices() {
                if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        if let Some(a) = try_parse(&text[start..start + j + 1]) {
                            return Some(a);
                        }
                        break;
                    }
                }
            }
            from = start + 1;
        }
    }
    None
}

fn bbox_of(v: &Value) -> Option<[f64; 4]> {
    let a = v.as_array()?;
    if a.len() != 4 {
        return None;
    }
    let n: Vec<f64> = a.iter().map(|x| x.as_f64()).collect::<Option<_>>()?;
    let (x, y) = (n[0], n[1]);
    let (w, h) = (n[2], n[3]);
    let ok = x.is_finite()
        && y.is_finite()
        && w.is_finite()
        && h.is_finite()
        && (0.0..=1.0).contains(&x)
        && (0.0..=1.0).contains(&y)
        && w > 0.0
        && h > 0.0
        && w <= 1.0
        && h <= 1.0;
    ok.then(|| [x, y, w.min(1.0 - x), h.min(1.0 - y)])
        .filter(|b| b[2] > 0.0 && b[3] > 0.0)
}

/// Boxes from the model's reply; `None` when the reply holds no list at all.
pub fn parse(text: &str) -> Option<Vec<Found>> {
    let items = find_list(text)?;
    let mut out = Vec::new();
    for it in items.iter().take(MAX_BOXES) {
        let label = match it
            .get("label")
            .or_else(|| it.get("name"))
            .or_else(|| it.get("class"))
            .and_then(Value::as_str)
        {
            Some(l) => l.trim().chars().take(64).collect::<String>(),
            None => continue,
        };
        if label.is_empty() {
            continue;
        }
        let score = it
            .get("score")
            .or_else(|| it.get("confidence"))
            .and_then(Value::as_f64)
            .filter(|s| s.is_finite())
            .map_or(1.0, |s| s.clamp(0.0, 1.0));
        let (bbox, whole_frame) = match it.get("bbox").or_else(|| it.get("box")).and_then(bbox_of) {
            Some(b) => (b, false),
            None => ([0.0, 0.0, 1.0, 1.0], true),
        };
        out.push(Found {
            label,
            score,
            bbox,
            whole_frame,
        });
    }
    Some(out)
}

/// The `detections.v1` body.
pub fn detections_body(
    source: &str,
    node_id: &str,
    t_ns: u64,
    seq: u64,
    frame_hash: &str,
    model: &str,
    found: &[Found],
) -> Value {
    json!({
        "source_id": source, "node_id": node_id, "t_ns": t_ns, "seq": seq,
        "frame_hash": frame_hash, "model": model,
        "boxes": found.iter().map(|f| json!({"label": f.label, "score": f.score, "bbox": f.bbox})).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_array_parses_with_boxes_and_scores() {
        let f = parse(r#"[{"label":"person","bbox":[0.1,0.2,0.3,0.5],"score":0.9},{"label":"chair","bbox":[0.6,0.5,0.2,0.4]}]"#).unwrap();
        assert_eq!(f.len(), 2);
        assert_eq!(
            (f[0].label.as_str(), f[0].score, f[0].bbox, f[0].whole_frame),
            ("person", 0.9, [0.1, 0.2, 0.3, 0.5], false)
        );
        assert_eq!(f[1].score, 1.0, "no score: not measured");
    }

    #[test]
    fn arrays_inside_prose_fences_and_keys_are_found() {
        for t in [
            "Sure! Here you go:\n```json\n[{\"label\":\"cup\",\"bbox\":[0,0,0.5,0.5]}]\n```\nHope that helps.",
            "{\"detections\": [{\"label\":\"cup\",\"bbox\":[0,0,0.5,0.5]}]}",
            "I can see: {\"objects\": [{\"label\":\"cup\",\"bbox\":[0,0,0.5,0.5]}]} and nothing else",
            "<think>hmm [1,2]</think> [{\"name\":\"cup\",\"box\":[0,0,0.5,0.5]}]",
        ] {
            let f = parse(t).unwrap_or_else(|| panic!("{t}"));
            assert_eq!((f.len(), f[0].label.as_str(), f[0].whole_frame), (1, "cup", false), "{t}");
        }
    }

    #[test]
    fn no_list_is_none_and_an_empty_list_is_no_objects() {
        assert!(parse("I see a desk with a monitor on it.").is_none());
        assert_eq!(parse("[]").unwrap(), vec![]);
        assert_eq!(parse("Nothing clear: []").unwrap(), vec![]);
    }

    #[test]
    fn a_label_without_a_usable_box_covers_the_whole_frame() {
        let f = parse(r#"[{"label":"monitor"},{"label":"lamp","bbox":[0.9,0.9,0.5,0.5]},{"label":"desk","bbox":[1,2,3]},{"label":"pc","bbox":[250,100,300,300]}]"#).unwrap();
        assert!(f[0].whole_frame && f[0].bbox == [0.0, 0.0, 1.0, 1.0]);
        assert!(
            !f[1].whole_frame
                && (f[1].bbox[2] - 0.1).abs() < 1e-9
                && (f[1].bbox[3] - 0.1).abs() < 1e-9,
            "clamped to stay inside the frame: {:?}",
            f[1].bbox
        );
        assert!(f[2].whole_frame, "three numbers are not a box");
        assert!(f[3].whole_frame, "pixel coordinates are not normalized");
    }

    #[test]
    fn junk_items_are_skipped_and_labels_bounded() {
        let long = "x".repeat(100);
        let t = format!(
            r#"[{{"bbox":[0,0,1,1]}},{{"label":"  "}},{{"label":"{long}","score":7}},5,"a"]"#
        );
        let f = parse(&t).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].label.len(), f[0].score), (64, 1.0), "score clamped");
        let many = format!(
            "[{}]",
            (0..100)
                .map(|i| format!("{{\"label\":\"o{i}\"}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_eq!(parse(&many).unwrap().len(), MAX_BOXES);
    }

    #[test]
    fn the_body_has_the_schema_shape() {
        let f = parse(r#"[{"label":"person","bbox":[0.1,0.2,0.3,0.5],"score":0.9}]"#).unwrap();
        let b = detections_body(
            "vis0",
            &"a".repeat(32),
            5,
            2,
            &"b".repeat(64),
            "qwen3-vl-4b",
            &f,
        );
        for k in [
            "source_id",
            "node_id",
            "t_ns",
            "seq",
            "frame_hash",
            "model",
            "boxes",
        ] {
            assert!(b.get(k).is_some(), "{k}");
        }
        assert_eq!(
            (
                b["boxes"][0]["label"].as_str(),
                b["boxes"][0]["score"].as_f64()
            ),
            (Some("person"), Some(0.9))
        );
        assert_eq!(b["boxes"][0]["bbox"].as_array().unwrap().len(), 4);
    }
}
