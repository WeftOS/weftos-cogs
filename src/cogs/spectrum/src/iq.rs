//! The `iq.cs16.v1` message, as the owner cog (`sdr-pluto-rx`) writes it. This copy decodes; the
//! encoder is for the tests. The field list is in the owner's `src/iq.rs` and ADR-176.
//!
//! The stream carries the registered schema (WeftOS `contracts/cogs`, `iq.cs16.v1`): a JSON
//! object with the common fields `source_id`, `node_id`, `t_ns`, `seq`, then `center_hz`,
//! `sample_rate_hz`, `samples` and `data_b64` (standard base64 of `samples * 4` bytes, I then Q,
//! signed 16-bit little-endian; the AD9363's 12 bits are sign-extended). Unknown fields are
//! ignored by readers, so this cog adds optional ones in `v1`:
//!
//! | Field | Meaning |
//! |---|---|
//! | `bandwidth_hz` | the analog bandwidth |
//! | `segment` | counts dwells; a new segment starts at every retune |
//! | `sample_index` | index of the first sample on the segment's sample clock; lost samples are counted in it, so a gap is a jump |
//! | `lost` | samples lost immediately before this chunk (0 when none) |
//! | `discontinuity`, `band_changed`, `all_zero` | present and true when set: samples were lost just before; the tuning changed; every sample is zero (a dead receive path) |
//!
//! A gap is also written as its own record: `samples` 0, `data_b64` empty, `gap_samples` the
//! number lost. It is not a sample chunk (the registered validator needs at least one sample), so
//! a reader that validates chunks skips it; the next chunk's `lost` carries the same number.
//! The ring record's `seq` is the message `seq`; its `t_ns` is the host monotonic clock at the
//! end of the chunk's capture; its `w_ns` is the wall clock at the write.

use serde_json::Value;

pub const KIND_SAMPLES: u8 = 0;
pub const KIND_GAP: u8 = 1;
pub const F_DISCONT: u8 = 1;
pub const F_BAND_CHANGED: u8 = 2;
pub const F_ALL_ZERO: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub kind: u8,
    pub flags: u8,
    pub segment: u32,
    pub centre_hz: u64,
    pub rate_sps: u32,
    pub bw_hz: u32,
    pub sample_index: u64,
    pub lost: u32,
    pub n_samples: u32,
}

/// Who the message is from.
#[cfg(test)]
pub struct Meta<'a> {
    pub source_id: &'a str,
    pub node_id: &'a str,
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

#[cfg(test)]
pub fn b64_encode(b: &[u8]) -> String {
    let mut v = Vec::with_capacity(b.len().div_ceil(3) * 4);
    b64_encode_into(b, &mut v);
    String::from_utf8(v).unwrap_or_default()
}

/// Standard padded base64, appended to `out`.
#[cfg(test)]
pub fn b64_encode_into(b: &[u8], out: &mut Vec<u8>) {
    out.reserve(b.len().div_ceil(3) * 4);
    let mut chunks = b.chunks_exact(3);
    for c in &mut chunks {
        let n = (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2]);
        out.extend_from_slice(&[
            B64[(n >> 18) as usize & 63],
            B64[(n >> 12) as usize & 63],
            B64[(n >> 6) as usize & 63],
            B64[n as usize & 63],
        ]);
    }
    let r = chunks.remainder();
    if !r.is_empty() {
        let n = (u32::from(r[0]) << 16) | (u32::from(*r.get(1).unwrap_or(&0)) << 8);
        out.push(B64[(n >> 18) as usize & 63]);
        out.push(B64[(n >> 12) as usize & 63]);
        out.push(if r.len() > 1 {
            B64[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(b'=');
    }
}

/// Value of each base64 character, or 255.
const LUT: [u8; 256] = {
    let mut t = [255u8; 256];
    let mut i = 0;
    while i < 64 {
        t[B64[i] as usize] = i as u8;
        i += 1;
    }
    t
};

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if !b.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 4 * 3);
    let quads = b.len() / 4;
    for (i, q) in b.chunks_exact(4).enumerate() {
        let pad = if i + 1 == quads {
            q.iter().rev().take_while(|&&c| c == b'=').count()
        } else {
            0
        };
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for (j, &c) in q.iter().enumerate() {
            n <<= 6;
            if j < 4 - pad {
                let v = LUT[c as usize];
                if v == 255 {
                    return None;
                }
                n |= u32::from(v);
            }
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

/// Split the `data_b64` text out of a message without copying it into a JSON value: the rest
/// of the message (with the field emptied) is small. The owner writes the field once, with no
/// escapes; a message that does not look like that falls back to the full parse.
fn split_data(b: &[u8]) -> Option<(Vec<u8>, &str)> {
    const KEY: &[u8] = b"\"data_b64\":\"";
    let at = b.windows(KEY.len()).position(|w| w == KEY)? + KEY.len();
    let end = at + b[at..].iter().position(|&c| c == b'"')?;
    if b[at..end].contains(&b'\\') {
        return None;
    }
    let mut rest = Vec::with_capacity(b.len() - (end - at));
    rest.extend_from_slice(&b[..at]);
    rest.extend_from_slice(&b[end..]);
    Some((rest, std::str::from_utf8(&b[at..end]).ok()?))
}

impl Header {
    /// The message for ring record `seq` captured at `t_ns`. Written by hand rather than through
    /// a JSON value: it runs 600 times a second at 5 MS/s. `source_id` and `node_id` are plain
    /// `[a-z0-9._-]` and hex, which the cog checks at start, so nothing needs escaping.
    #[cfg(test)]
    pub fn encode(&self, samples: &[u8], meta: &Meta, t_ns: u64, seq: u64) -> Vec<u8> {
        use std::io::Write;
        let mut out = Vec::with_capacity(samples.len().div_ceil(3) * 4 + 512);
        let _ = write!(
            out,
            "{{\"source_id\":\"{}\",\"node_id\":\"{}\",\"t_ns\":{t_ns},\"seq\":{seq},\"center_hz\":{},\"sample_rate_hz\":{},\"samples\":{},\"bandwidth_hz\":{},\"segment\":{},\"sample_index\":{},\"lost\":{}",
            meta.source_id,
            meta.node_id,
            self.centre_hz,
            self.rate_sps,
            self.n_samples,
            self.bw_hz,
            self.segment,
            self.sample_index,
            self.lost
        );
        if self.kind == KIND_GAP {
            let _ = write!(out, ",\"gap_samples\":{}", self.lost);
        }
        for (bit, name) in [
            (F_DISCONT, "discontinuity"),
            (F_BAND_CHANGED, "band_changed"),
            (F_ALL_ZERO, "all_zero"),
        ] {
            if self.flags & bit != 0 {
                let _ = write!(out, ",\"{name}\":true");
            }
        }
        out.extend_from_slice(b",\"data_b64\":\"");
        b64_encode_into(samples, &mut out);
        out.extend_from_slice(b"\"}");
        out
    }

    /// The header and the sample bytes of a record, or why it is not one (this cog reads the
    /// owner's records).
    pub fn decode(b: &[u8]) -> Result<(Header, Vec<u8>), String> {
        let (v, text): (Value, std::borrow::Cow<str>) = match split_data(b) {
            Some((rest, text)) => (
                serde_json::from_slice(&rest).map_err(|e| format!("not JSON: {e}"))?,
                std::borrow::Cow::Borrowed(text),
            ),
            None => {
                let v: Value = serde_json::from_slice(b).map_err(|e| format!("not JSON: {e}"))?;
                let t = v["data_b64"]
                    .as_str()
                    .ok_or("data_b64 missing")?
                    .to_string();
                (v, std::borrow::Cow::Owned(t))
            }
        };
        let u = |k: &str| v[k].as_u64().ok_or_else(|| format!("{k} missing"));
        let n = u("samples")? as u32;
        let data = b64_decode(&text).ok_or("data_b64 is not base64")?;
        if data.len() != n as usize * 4 {
            return Err(format!(
                "{n} samples declared, {} bytes present",
                data.len()
            ));
        }
        let flag = |k: &str, bit: u8| if v[k].as_bool() == Some(true) { bit } else { 0 };
        let h = Header {
            kind: if v.get("gap_samples").is_some() {
                KIND_GAP
            } else {
                KIND_SAMPLES
            },
            flags: flag("discontinuity", F_DISCONT)
                | flag("band_changed", F_BAND_CHANGED)
                | flag("all_zero", F_ALL_ZERO),
            segment: v["segment"].as_u64().unwrap_or(0) as u32,
            centre_hz: u("center_hz")?,
            rate_sps: u("sample_rate_hz")? as u32,
            bw_hz: v["bandwidth_hz"].as_u64().unwrap_or(0) as u32,
            sample_index: v["sample_index"].as_u64().unwrap_or(0),
            lost: v["lost"].as_u64().unwrap_or(0) as u32,
            n_samples: n,
        };
        Ok((h, data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head() -> Header {
        Header {
            kind: KIND_SAMPLES,
            flags: F_DISCONT | F_BAND_CHANGED,
            segment: 7,
            centre_hz: 2_437_000_000,
            rate_sps: 5_000_000,
            bw_hz: 4_000_000,
            sample_index: 1 << 33,
            lost: 123,
            n_samples: 2,
        }
    }

    const META: Meta = Meta {
        source_id: "venq-pluto",
        node_id: "0123456789abcdef0123456789abcdef",
    };

    #[test]
    fn base64_matches_the_registered_vectors_and_round_trips() {
        // from contracts/cogs/vectors/iq.cs16.v1/valid-wifi-ch6.json
        let raw: Vec<u8> = (0u8..16).collect();
        assert_eq!(b64_encode(&raw), "AAECAwQFBgcICQoLDA0ODw==");
        assert_eq!(b64_decode("AAECAwQFBgcICQoLDA0ODw==").unwrap(), raw);
        assert_eq!(b64_encode(&[0, 1, 2, 3]), "AAECAw==");
        for n in 0..40usize {
            let d: Vec<u8> = (0..n).map(|i| (i * 7 + 3) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&d)).unwrap(), d, "{n}");
        }
        assert!(b64_decode("AAE").is_none());
        assert!(b64_decode("AA=A").is_none());
        assert!(b64_decode("A!==").is_none());
    }

    #[test]
    fn a_record_is_a_registered_iq_message_and_round_trips() {
        let s = [1u8, 0, 2, 0, 3, 0, 4, 0];
        let b = head().encode(&s, &META, 99, 5);
        let v: Value = serde_json::from_slice(&b).unwrap();
        // the registered fields, with the registered rules
        assert_eq!(v["source_id"], "venq-pluto");
        assert_eq!(v["node_id"].as_str().unwrap().len(), 32);
        assert_eq!((v["t_ns"].as_u64(), v["seq"].as_u64()), (Some(99), Some(5)));
        assert_eq!(
            (
                v["center_hz"].as_u64(),
                v["sample_rate_hz"].as_u64(),
                v["samples"].as_u64()
            ),
            (Some(2_437_000_000), Some(5_000_000), Some(2))
        );
        assert_eq!(
            b64_decode(v["data_b64"].as_str().unwrap()).unwrap().len(),
            8
        );
        let (h, body) = Header::decode(&b).unwrap();
        assert_eq!(h, head());
        assert_eq!(body, s);
    }

    #[test]
    fn a_gap_record_has_no_samples_and_says_how_many_were_lost() {
        let g = Header {
            kind: KIND_GAP,
            flags: F_DISCONT,
            n_samples: 0,
            lost: 777,
            ..head()
        };
        let b = g.encode(&[], &META, 1, 2);
        let v: Value = serde_json::from_slice(&b).unwrap();
        assert_eq!(
            (
                v["samples"].as_u64(),
                v["gap_samples"].as_u64(),
                v["data_b64"].as_str()
            ),
            (Some(0), Some(777), Some(""))
        );
        let (h, body) = Header::decode(&b).unwrap();
        assert_eq!((h.kind, h.lost, body.len()), (KIND_GAP, 777, 0));
    }

    #[test]
    fn malformed_records_are_refused() {
        assert!(Header::decode(b"{").is_err());
        let mut b = head().encode(&[0u8; 8], &META, 1, 1);
        let s = String::from_utf8(b.clone())
            .unwrap()
            .replace("\"samples\":2", "\"samples\":3");
        b = s.into_bytes();
        assert!(Header::decode(&b).is_err());
    }
}
