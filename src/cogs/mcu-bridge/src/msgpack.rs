//! The msgpack subset the arduino-router speaks (msgpack-RPC): encode a request, decode a
//! response. Values map to and from `serde_json::Value`; maps need string keys, and binary
//! and extension values are refused rather than guessed.

use serde_json::{json, Value};

const MAX_DEPTH: usize = 16;

pub fn encode(v: &Value, out: &mut Vec<u8>) -> Result<(), String> {
    match v {
        Value::Null => out.push(0xc0),
        Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                int(i, out);
            } else if let Some(u) = n.as_u64() {
                out.push(0xcf);
                out.extend_from_slice(&u.to_be_bytes());
            } else {
                out.push(0xcb);
                out.extend_from_slice(&n.as_f64().unwrap_or(0.0).to_be_bytes());
            }
        }
        Value::String(s) => {
            let b = s.as_bytes();
            match b.len() {
                0..=31 => out.push(0xa0 | b.len() as u8),
                32..=255 => out.extend_from_slice(&[0xd9, b.len() as u8]),
                256..=65535 => {
                    out.push(0xda);
                    out.extend_from_slice(&(b.len() as u16).to_be_bytes());
                }
                _ => return Err("string too long".into()),
            }
            out.extend_from_slice(b);
        }
        Value::Array(a) => {
            match a.len() {
                0..=15 => out.push(0x90 | a.len() as u8),
                16..=65535 => {
                    out.push(0xdc);
                    out.extend_from_slice(&(a.len() as u16).to_be_bytes());
                }
                _ => return Err("array too long".into()),
            }
            for x in a {
                encode(x, out)?;
            }
        }
        Value::Object(m) => {
            if m.len() > 15 {
                return Err("map too large".into());
            }
            out.push(0x80 | m.len() as u8);
            for (k, x) in m {
                encode(&Value::String(k.clone()), out)?;
                encode(x, out)?;
            }
        }
    }
    Ok(())
}

fn int(i: i64, out: &mut Vec<u8>) {
    match i {
        0..=127 => out.push(i as u8),
        -32..=-1 => out.push(i as i8 as u8),
        128..=255 => out.extend_from_slice(&[0xcc, i as u8]),
        256..=65535 => {
            out.push(0xcd);
            out.extend_from_slice(&(i as u16).to_be_bytes());
        }
        65536..=4294967295 => {
            out.push(0xce);
            out.extend_from_slice(&(i as u32).to_be_bytes());
        }
        i64::MIN..=-33 if i >= -128 => out.extend_from_slice(&[0xd0, i as i8 as u8]),
        i64::MIN..=-129 if i >= -32768 => {
            out.push(0xd1);
            out.extend_from_slice(&(i as i16).to_be_bytes());
        }
        i64::MIN..=-32769 if i >= -2147483648 => {
            out.push(0xd2);
            out.extend_from_slice(&(i as i32).to_be_bytes());
        }
        _ if i < 0 => {
            out.push(0xd3);
            out.extend_from_slice(&i.to_be_bytes());
        }
        _ => {
            out.push(0xcf);
            out.extend_from_slice(&(i as u64).to_be_bytes());
        }
    }
}

/// One request `[0, msgid, method, params]`.
pub fn request(msgid: u32, method: &str, params: &[Value]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    encode(&json!([0, msgid, method, params]), &mut out)?;
    Ok(out)
}

struct Cur<'a>(&'a [u8], usize);

impl Cur<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let s = self.0.get(self.1..self.1 + n).ok_or("truncated")?;
        self.1 += n;
        Ok(s)
    }

    fn be(&mut self, n: usize) -> Result<u64, String> {
        Ok(self.take(n)?.iter().fold(0u64, |a, b| a << 8 | *b as u64))
    }
}

fn value(c: &mut Cur, depth: usize) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Err("nested too deep".into());
    }
    let t = c.take(1)?[0];
    Ok(match t {
        0x00..=0x7f => json!(t),
        0x80..=0x8f => map(c, (t & 0x0f) as usize, depth)?,
        0x90..=0x9f => array(c, (t & 0x0f) as usize, depth)?,
        0xa0..=0xbf => string(c, (t & 0x1f) as usize)?,
        0xc0 => Value::Null,
        0xc2 => json!(false),
        0xc3 => json!(true),
        0xca => json!(f32::from_bits(c.be(4)? as u32) as f64),
        0xcb => json!(f64::from_bits(c.be(8)?)),
        0xcc => json!(c.be(1)?),
        0xcd => json!(c.be(2)?),
        0xce => json!(c.be(4)?),
        0xcf => json!(c.be(8)?),
        0xd0 => json!(c.be(1)? as u8 as i8),
        0xd1 => json!(c.be(2)? as u16 as i16),
        0xd2 => json!(c.be(4)? as u32 as i32),
        0xd3 => json!(c.be(8)? as i64),
        0xd9 => {
            let n = c.be(1)? as usize;
            string(c, n)?
        }
        0xda => {
            let n = c.be(2)? as usize;
            string(c, n)?
        }
        0xdb => {
            let n = c.be(4)? as usize;
            string(c, n)?
        }
        0xdc => {
            let n = c.be(2)? as usize;
            array(c, n, depth)?
        }
        0xdd => {
            let n = c.be(4)? as usize;
            array(c, n, depth)?
        }
        0xde => {
            let n = c.be(2)? as usize;
            map(c, n, depth)?
        }
        0xe0..=0xff => json!(t as i8),
        _ => return Err(format!("unsupported msgpack type 0x{t:02x}")),
    })
}

fn string(c: &mut Cur, n: usize) -> Result<Value, String> {
    String::from_utf8(c.take(n)?.to_vec())
        .map(Value::String)
        .map_err(|_| "string is not UTF-8".to_string())
}

fn array(c: &mut Cur, n: usize, depth: usize) -> Result<Value, String> {
    if n > 4096 {
        return Err("array too long".into());
    }
    (0..n)
        .map(|_| value(c, depth + 1))
        .collect::<Result<Vec<_>, _>>()
        .map(Value::Array)
}

fn map(c: &mut Cur, n: usize, depth: usize) -> Result<Value, String> {
    let mut m = serde_json::Map::new();
    for _ in 0..n {
        let Value::String(k) = value(c, depth + 1)? else {
            return Err("map key is not a string".into());
        };
        m.insert(k, value(c, depth + 1)?);
    }
    Ok(Value::Object(m))
}

/// Decode one value; returns it and the bytes consumed. `Err("truncated")` means feed more.
pub fn decode(b: &[u8]) -> Result<(Value, usize), String> {
    let mut c = Cur(b, 0);
    let v = value(&mut c, 0)?;
    Ok((v, c.1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(v: Value) -> Value {
        let mut out = Vec::new();
        encode(&v, &mut out).unwrap();
        let (back, n) = decode(&out).unwrap();
        assert_eq!(n, out.len());
        back
    }

    #[test]
    fn values_round_trip() {
        for v in [
            json!(null),
            json!(true),
            json!(0),
            json!(127),
            json!(128),
            json!(255),
            json!(256),
            json!(65535),
            json!(65536),
            json!(4294967295u64),
            json!(4294967296u64),
            json!(-1),
            json!(-32),
            json!(-33),
            json!(-128),
            json!(-129),
            json!(-32768),
            json!(-32769),
            json!(-2147483648i64),
            json!(-2147483649i64),
            json!(1.5),
            json!("hello"),
            json!("x".repeat(40)),
            json!("y".repeat(300)),
            json!([1, "a", [true, null]]),
            json!({"a": 1, "b": [2, 3]}),
        ] {
            assert_eq!(rt(v.clone()), v);
        }
    }

    #[test]
    fn known_encodings_match_the_spec() {
        let mut o = Vec::new();
        encode(&json!([0, 1, "weftos_ping", []]), &mut o).unwrap();
        assert_eq!(
            o,
            [&[0x94, 0x00, 0x01, 0xab][..], b"weftos_ping", &[0x90]].concat()
        );
        assert_eq!(request(1, "weftos_ping", &[]).unwrap(), o);
        let (v, _) = decode(&[0xcc, 0x2a]).unwrap();
        assert_eq!(v, 42);
        let (v, _) = decode(&[0xca, 0x3f, 0xc0, 0x00, 0x00]).unwrap();
        assert_eq!(v, 1.5, "float32");
        let (v, _) = decode(&[0xff]).unwrap();
        assert_eq!(v, -1, "negative fixint");
    }

    #[test]
    fn a_response_decodes_and_a_partial_one_asks_for_more() {
        let resp = [0x94, 0x01, 0x01, 0xc0, 0x2a];
        let (v, n) = decode(&resp).unwrap();
        assert_eq!((v, n), (json!([1, 1, null, 42]), 5));
        assert_eq!(decode(&resp[..3]).unwrap_err(), "truncated");
    }

    #[test]
    fn hostile_input_is_refused_not_trusted() {
        assert!(
            decode(&[0xc4, 0x01, 0x00])
                .unwrap_err()
                .contains("unsupported"),
            "bin is refused"
        );
        assert!(
            decode(&[0x81, 0x01, 0x02]).unwrap_err().contains("key"),
            "non-string map key"
        );
        let deep = vec![0x91; 40];
        assert!(decode(&deep).unwrap_err().contains("deep"));
        assert!(decode(&[0xdd, 0xff, 0xff, 0xff, 0xff])
            .unwrap_err()
            .contains("long"));
        assert!(decode(&[0xa2, 0xff, 0xfe]).unwrap_err().contains("UTF-8"));
    }
}
