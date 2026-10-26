//! SocketCAN frames: decoding what the kernel hands a raw socket, the `can.frame.v1` JSON for a
//! data frame, the error-frame classes, and the `candump -L` log lines used by `--replay`.
//!
//! Layout of `struct can_frame` (16 bytes) and `struct canfd_frame` (72 bytes), native endian:
//! `can_id` u32 (flags in the top three bits), length u8, then for FD a flags byte (BRS 0x01,
//! ESI 0x02), two reserved bytes, and the data from offset 8.

use serde_json::{json, Value};

pub const CAN_EFF_FLAG: u32 = 0x8000_0000;
pub const CAN_RTR_FLAG: u32 = 0x4000_0000;
pub const CAN_ERR_FLAG: u32 = 0x2000_0000;
pub const CAN_EFF_MASK: u32 = 0x1FFF_FFFF;
pub const CAN_SFF_MASK: u32 = 0x7FF;
pub const CLASSIC_LEN: usize = 16;
pub const FD_LEN: usize = 72;

/// What one read produced.
#[derive(Debug, Clone, PartialEq)]
pub enum Rx {
    Data(Frame),
    /// A remote-transmission request: no data, and `can.frame.v1` has no place for it.
    Remote,
    Error(ErrorFrame),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub id: u32,
    pub ext: bool,
    pub fd: bool,
    pub brs: bool,
    pub data: Vec<u8>,
}

/// An error frame, by class (`CAN_ERR_*` bits of its id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorFrame {
    pub class: u32,
    /// The controller's status byte for `CAN_ERR_CRTL` (data[1]).
    pub ctrl: u8,
}

pub const ERR_TX_TIMEOUT: u32 = 0x001;
pub const ERR_LOSTARB: u32 = 0x002;
pub const ERR_CRTL: u32 = 0x004;
pub const ERR_PROT: u32 = 0x008;
pub const ERR_TRX: u32 = 0x010;
pub const ERR_ACK: u32 = 0x020;
pub const ERR_BUSOFF: u32 = 0x040;
pub const ERR_BUSERROR: u32 = 0x080;
pub const ERR_RESTARTED: u32 = 0x100;

/// Names of the classes set in `class`.
pub fn class_names(class: u32) -> Vec<&'static str> {
    [
        (ERR_TX_TIMEOUT, "tx_timeout"),
        (ERR_LOSTARB, "lost_arbitration"),
        (ERR_CRTL, "controller"),
        (ERR_PROT, "protocol"),
        (ERR_TRX, "transceiver"),
        (ERR_ACK, "no_ack"),
        (ERR_BUSOFF, "bus_off"),
        (ERR_BUSERROR, "bus_error"),
        (ERR_RESTARTED, "restarted"),
    ]
    .iter()
    .filter(|(b, _)| class & b != 0)
    .map(|(_, n)| *n)
    .collect()
}

/// Data length code for a byte length: 0..=8 as is, then 12, 16, 20, 24, 32, 48, 64 -> 9..=15.
pub fn dlc_of(len: usize) -> Option<u8> {
    match len {
        0..=8 => Some(len as u8),
        12 => Some(9),
        16 => Some(10),
        20 => Some(11),
        24 => Some(12),
        32 => Some(13),
        48 => Some(14),
        64 => Some(15),
        _ => None,
    }
}

/// Decode one read of `buf` (16 or 72 bytes).
pub fn decode(buf: &[u8]) -> Result<Rx, String> {
    let fd = match buf.len() {
        CLASSIC_LEN => false,
        FD_LEN => true,
        n => {
            return Err(format!(
                "a CAN read of {n} bytes is neither a classic (16) nor an FD (72) frame"
            ))
        }
    };
    let can_id = u32::from_ne_bytes([buf[0], buf[1], buf[2], buf[3]]);
    let len = buf[4] as usize;
    if len > buf.len() - 8 || (!fd && len > 8) {
        return Err(format!(
            "frame length {len} does not fit its {}-byte read",
            buf.len()
        ));
    }
    if can_id & CAN_ERR_FLAG != 0 {
        return Ok(Rx::Error(ErrorFrame {
            class: can_id & 0x1FFF_FFFF,
            ctrl: if len > 1 { buf[8 + 1] } else { 0 },
        }));
    }
    if can_id & CAN_RTR_FLAG != 0 {
        return Ok(Rx::Remote);
    }
    let ext = can_id & CAN_EFF_FLAG != 0;
    let id = can_id & if ext { CAN_EFF_MASK } else { CAN_SFF_MASK };
    if dlc_of(len).is_none() {
        return Err(format!("frame length {len} is not a CAN length"));
    }
    Ok(Rx::Data(Frame {
        id,
        ext,
        fd,
        brs: fd && buf[5] & 0x01 != 0,
        data: buf[8..8 + len].to_vec(),
    }))
}

pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02x}")).collect()
}

/// The `can.frame.v1` body for a data frame.
pub fn frame_json(source: &str, node_id: &str, t_ns: u64, seq: u64, f: &Frame) -> Value {
    json!({
        "source_id": source, "node_id": node_id, "t_ns": t_ns, "seq": seq,
        "id": f.id, "ext": f.ext, "fd": f.fd, "dlc": dlc_of(f.data.len()).unwrap_or(0), "data": hex(&f.data),
    })
}

/// Approximate bits a frame occupies on the wire (no stuffing): overhead plus data, at the
/// nominal rate for classic frames. FD frames are counted at the nominal rate too, which
/// overstates the load of a bit-rate-switched frame.
pub fn wire_bits(f: &Frame) -> u64 {
    let overhead = if f.ext { 67 } else { 47 };
    overhead + 8 * f.data.len() as u64
}

/// A frame from a `candump -L` log line: `(1600000000.123456) can0 123#DEADBEEF` or an FD frame
/// `123##1DEADBEEF` (flags nibble, then data). Returns the capture time in ns and the frame.
pub fn parse_candump_line(line: &str) -> Result<(u64, Frame), String> {
    let mut it = line.split_whitespace();
    let ts = it.next().ok_or("empty line")?;
    let _iface = it.next().ok_or("no interface")?;
    let body = it.next().ok_or("no frame")?;
    let secs: f64 = ts
        .trim_matches(|c| c == '(' || c == ')')
        .parse()
        .map_err(|_| format!("bad timestamp {ts:?}"))?;
    let (id_s, fd, rest) = match body.split_once("##") {
        Some((i, r)) => (i, true, r.get(1..).ok_or("FD frame without flags")?),
        None => {
            let (i, r) = body
                .split_once('#')
                .ok_or_else(|| format!("no # in {body:?}"))?;
            (i, false, r)
        }
    };
    let ext = id_s.len() > 3;
    let id = u32::from_str_radix(id_s, 16).map_err(|_| format!("bad id {id_s:?}"))?;
    if id > if ext { CAN_EFF_MASK } else { CAN_SFF_MASK } {
        return Err(format!("id {id:#x} is too wide"));
    }
    if rest.len() % 2 != 0 || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("bad data {rest:?}"));
    }
    let data: Vec<u8> = (0..rest.len() / 2)
        .map(|i| u8::from_str_radix(&rest[2 * i..2 * i + 2], 16).unwrap())
        .collect();
    if dlc_of(data.len()).is_none() || (!fd && data.len() > 8) {
        return Err(format!("{} data bytes is not a CAN length", data.len()));
    }
    Ok((
        (secs * 1e9) as u64,
        Frame {
            id,
            ext,
            fd,
            brs: false,
            data,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(id: u32, len: u8, flags: u8, data: &[u8], fd: bool) -> Vec<u8> {
        let mut b = vec![0u8; if fd { FD_LEN } else { CLASSIC_LEN }];
        b[..4].copy_from_slice(&id.to_ne_bytes());
        b[4] = len;
        b[5] = flags;
        b[8..8 + data.len()].copy_from_slice(data);
        b
    }

    #[test]
    fn a_classic_standard_frame_decodes() {
        let Rx::Data(f) = decode(&raw(0x123, 3, 0, &[1, 2, 3], false)).unwrap() else {
            panic!()
        };
        assert_eq!(
            (f.id, f.ext, f.fd, f.data.as_slice()),
            (0x123, false, false, &[1, 2, 3][..])
        );
    }

    #[test]
    fn an_extended_id_loses_its_flag_and_an_fd_frame_keeps_its_length() {
        let Rx::Data(f) = decode(&raw(0x1ABC_DEF0 | CAN_EFF_FLAG, 8, 0, &[0; 8], false)).unwrap()
        else {
            panic!()
        };
        assert_eq!((f.id, f.ext), (0x1ABC_DEF0, true));
        let d: Vec<u8> = (0..24).collect();
        let Rx::Data(g) = decode(&raw(0x7FF, 24, 0x01, &d, true)).unwrap() else {
            panic!()
        };
        assert_eq!(
            (g.fd, g.brs, g.data.len(), dlc_of(g.data.len())),
            (true, true, 24, Some(12))
        );
    }

    #[test]
    fn remote_frames_and_error_frames_are_told_apart() {
        assert_eq!(
            decode(&raw(0x123 | CAN_RTR_FLAG, 0, 0, &[], false)).unwrap(),
            Rx::Remote
        );
        let e = decode(&raw(
            CAN_ERR_FLAG | ERR_BUSOFF | ERR_CRTL,
            8,
            0,
            &[0, 0x20, 0, 0, 0, 0, 0, 0],
            false,
        ))
        .unwrap();
        assert_eq!(
            e,
            Rx::Error(ErrorFrame {
                class: ERR_BUSOFF | ERR_CRTL,
                ctrl: 0x20
            })
        );
        assert_eq!(
            class_names(ERR_BUSOFF | ERR_CRTL),
            ["controller", "bus_off"]
        );
    }

    #[test]
    fn bad_reads_are_errors_not_frames() {
        assert!(decode(&[0; 10]).unwrap_err().contains("neither"));
        assert!(decode(&raw(0x1, 9, 0, &[0; 8], false))
            .unwrap_err()
            .contains("does not fit"));
        assert!(decode(&raw(0x1, 13, 0, &[0; 13], true))
            .unwrap_err()
            .contains("not a CAN length"));
    }

    #[test]
    fn dlc_covers_the_fd_lengths() {
        for (len, dlc) in [
            (0, 0),
            (8, 8),
            (12, 9),
            (16, 10),
            (20, 11),
            (24, 12),
            (32, 13),
            (48, 14),
            (64, 15),
        ] {
            assert_eq!(dlc_of(len), Some(dlc));
        }
        assert_eq!(dlc_of(9), None);
        assert_eq!(dlc_of(65), None);
    }

    #[test]
    fn the_json_has_the_schema_shape() {
        let f = Frame {
            id: 0x1a4,
            ext: false,
            fd: false,
            brs: false,
            data: vec![0xde, 0xad],
        };
        let v = frame_json("can0", &"a".repeat(32), 5, 7, &f);
        for k in [
            "source_id",
            "node_id",
            "t_ns",
            "seq",
            "id",
            "ext",
            "fd",
            "dlc",
            "data",
        ] {
            assert!(v.get(k).is_some(), "{k}");
        }
        assert_eq!(
            (v["dlc"].as_u64(), v["data"].as_str(), v["id"].as_u64()),
            (Some(2), Some("dead"), Some(0x1a4))
        );
        let fd = Frame {
            id: 1,
            ext: false,
            fd: true,
            brs: false,
            data: vec![0; 12],
        };
        assert_eq!(frame_json("c", "n", 0, 1, &fd)["dlc"], 9);
    }

    #[test]
    fn candump_lines_parse() {
        let (t, f) = parse_candump_line("(1600000000.500000) can0 123#DEADBEEF").unwrap();
        assert_eq!(
            (t, f.id, f.ext, f.data.as_slice()),
            (
                1_600_000_000_500_000_000,
                0x123,
                false,
                &[0xde, 0xad, 0xbe, 0xef][..]
            )
        );
        let (_, e) = parse_candump_line("(1.0) can0 1ABCDEF0#0102").unwrap();
        assert_eq!((e.id, e.ext), (0x1ABC_DEF0, true));
        let (_, g) = parse_candump_line("(1.0) can0 7FF##1000102030405060708090a0b").unwrap();
        assert_eq!((g.fd, g.data.len()), (true, 12));
        for bad in [
            "",
            "(1.0) can0",
            "(x) can0 123#00",
            "(1.0) can0 123#0",
            "(1.0) can0 FFF#00",
            "(1.0) can0 123#000000000000000000",
        ] {
            assert!(parse_candump_line(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn wire_bits_count_overhead_and_data() {
        assert_eq!(
            wire_bits(&Frame {
                id: 1,
                ext: false,
                fd: false,
                brs: false,
                data: vec![0; 8]
            }),
            47 + 64
        );
        assert_eq!(
            wire_bits(&Frame {
                id: 1,
                ext: true,
                fd: false,
                brs: false,
                data: vec![]
            }),
            67
        );
    }
}
