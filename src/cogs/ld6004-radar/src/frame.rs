//! Incremental reassembler for the HLK-LD6004 TinyFrame UART format.
//!
//! Shenzhen Hi-Link, "HLK-LD6004 模组通信协议" (3D presence radar module
//! communication protocol), cover date 2025-09, §4.2 and §7.2.
//!
//! ```text
//! 0x01 | id:u16be | len:u16be | type:u16be | hcksum | payload[len] | dcksum
//! ```
//!
//! `cksum = !(XOR of the covered bytes)`. The header checksum covers the first
//! seven bytes. The data checksum covers the payload. A zero-length frame has
//! no data checksum (§3.2 query example). On any mismatch, drop one byte and
//! rescan. This module does not assign meanings to type ids.

/// Start-of-frame byte. Protocol §4.2: SOF is 0x01.
pub const SOF: u8 = 0x01;

/// §4.2: the program defaults to a total frame of at most 1024 bytes.
/// Header is 8 bytes; a non-empty payload adds a 1-byte checksum, so a length
/// above `1024 - 8 - 1` is refused.
pub const MAX_PAYLOAD: usize = 1015;

const HEADER_LEN: usize = 8;

/// Query firmware status, TYPE 0xFFFF, copied from protocol §3.2 / §5.1.
/// `01 00 01 00 00 FF FF FF`.
pub const VENDOR_FW_QUERY: &[u8] = &[0x01, 0x00, 0x01, 0x00, 0x00, 0xFF, 0xFF, 0xFF];

/// Empty point-cloud upload, TYPE 0x0A08, copied from protocol §3.2.
/// Payload is four zero bytes (`target_num = 0`).
pub const VENDOR_EMPTY_CLOUD: &[u8] = &[
    0x01, 0x00, 0x06, 0x00, 0x04, 0x0A, 0x08, 0xFE, 0x00, 0x00, 0x00, 0x00, 0xFF,
];

/// Trigger-speed report, TYPE 0x0A0F, value 2 (fast). Protocol §6.13 example,
/// header and data checksums both match §7.2.
pub const VENDOR_TRIGGER_FAST: &[u8] =
    &[0x01, 0x00, 0x01, 0x00, 0x01, 0x0A, 0x0F, 0xFB, 0x02, 0xFD];

/// Firmware-status response copied from protocol §5.2.
/// Payload `06 03 0A 00`: project code 6 (not in the 0..=5 table), version 3.10.0.
/// Printed in protocol §5.2. Project code 6 is outside the name table.
/// Referenced by tests; the running cog does not special-case these bytes.
#[cfg_attr(not(test), allow(dead_code))]
pub const VENDOR_FW_STATUS: &[u8] = &[
    0x01, 0x04, 0xAF, 0x00, 0x04, 0xFF, 0xFF, 0x51, 0x06, 0x03, 0x0A, 0x00, 0xF0,
];

/// `!(XOR of bytes)`. Protocol §7.2 `getCksum`.
pub fn cksum(buf: &[u8]) -> u8 {
    !buf.iter().fold(0u8, |acc, b| acc ^ b)
}

/// One checksum-valid frame. `ty` is not interpreted here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub id: u16,
    pub ty: u16,
    pub payload: Vec<u8>,
}

/// Cumulative framer counters. A window takes [`FramerStats::since`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FramerStats {
    pub frames: u64,
    pub frame_bytes: u64,
    pub header_checksum_errors: u64,
    pub data_checksum_errors: u64,
    pub oversize_frames: u64,
    pub resync_bytes: u64,
}

impl FramerStats {
    pub fn since(&self, earlier: &FramerStats) -> FramerStats {
        FramerStats {
            frames: self.frames - earlier.frames,
            frame_bytes: self.frame_bytes - earlier.frame_bytes,
            header_checksum_errors: self.header_checksum_errors - earlier.header_checksum_errors,
            data_checksum_errors: self.data_checksum_errors - earlier.data_checksum_errors,
            oversize_frames: self.oversize_frames - earlier.oversize_frames,
            resync_bytes: self.resync_bytes - earlier.resync_bytes,
        }
    }

    pub fn checksum_errors(&self) -> u64 {
        self.header_checksum_errors + self.data_checksum_errors
    }
}

/// Feed raw UART bytes, get whole frames back. Holds at most one partial frame.
#[derive(Debug, Default)]
pub struct Framer {
    buf: Vec<u8>,
    stats: FramerStats,
}

impl Framer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn stats(&self) -> FramerStats {
        self.stats
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Vec<Frame> {
        self.buf.extend_from_slice(chunk);
        let mut out = Vec::new();
        let n = self.buf.len();
        let mut off = 0;
        while n - off >= HEADER_LEN {
            if self.buf[off] != SOF {
                off += 1;
                self.stats.resync_bytes += 1;
                continue;
            }
            if cksum(&self.buf[off..off + 7]) != self.buf[off + 7] {
                off += 1;
                self.stats.header_checksum_errors += 1;
                self.stats.resync_bytes += 1;
                continue;
            }
            let id = u16::from_be_bytes([self.buf[off + 1], self.buf[off + 2]]);
            let len = u16::from_be_bytes([self.buf[off + 3], self.buf[off + 4]]) as usize;
            let ty = u16::from_be_bytes([self.buf[off + 5], self.buf[off + 6]]);
            if len > MAX_PAYLOAD {
                off += 1;
                self.stats.oversize_frames += 1;
                self.stats.resync_bytes += 1;
                continue;
            }
            let total = HEADER_LEN + len + usize::from(len > 0);
            if n - off < total {
                break;
            }
            let payload = &self.buf[off + HEADER_LEN..off + HEADER_LEN + len];
            if len > 0 && cksum(payload) != self.buf[off + HEADER_LEN + len] {
                off += 1;
                self.stats.data_checksum_errors += 1;
                self.stats.resync_bytes += 1;
                continue;
            }
            out.push(Frame {
                id,
                ty,
                payload: payload.to_vec(),
            });
            self.stats.frames += 1;
            self.stats.frame_bytes += total as u64;
            off += total;
        }
        self.buf.drain(..off);
        out
    }
}

/// Build a frame with the §7.2 checksum. Tests and `--simulate` use this for
/// bytes the manual does not print in full. Vendor examples are separate
/// constants and are not produced here.
pub fn encode(id: u16, ty: u16, payload: &[u8]) -> Vec<u8> {
    let mut f = vec![SOF];
    f.extend_from_slice(&id.to_be_bytes());
    f.extend_from_slice(&(u16::try_from(payload.len()).unwrap_or(u16::MAX)).to_be_bytes());
    f.extend_from_slice(&ty.to_be_bytes());
    f.push(cksum(&f));
    if !payload.is_empty() {
        f.extend_from_slice(payload);
        f.push(cksum(payload));
    }
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_matches_the_manuals_get_cksum() {
        assert_eq!(cksum(&[]), 0xFF);
        assert_eq!(cksum(&[0x01, 0x02]), !0x03);
    }

    #[test]
    fn vendor_query_frame_parses_as_empty_ffff() {
        let got = Framer::new().feed(VENDOR_FW_QUERY);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ty, 0xFFFF);
        assert!(got[0].payload.is_empty());
        assert_eq!(VENDOR_FW_QUERY.len(), 8);
    }

    #[test]
    fn vendor_firmware_status_and_empty_cloud_and_trigger() {
        let mut bytes = VENDOR_FW_STATUS.to_vec();
        bytes.extend_from_slice(VENDOR_EMPTY_CLOUD);
        bytes.extend_from_slice(VENDOR_TRIGGER_FAST);
        let got = Framer::new().feed(&bytes);
        assert_eq!(
            got.iter()
                .map(|f| (f.ty, f.payload.clone()))
                .collect::<Vec<_>>(),
            vec![
                (0xFFFF, vec![0x06, 0x03, 0x0A, 0x00]),
                (0x0A08, vec![0x00, 0x00, 0x00, 0x00]),
                (0x0A0F, vec![0x02]),
            ]
        );
    }

    #[test]
    fn header_fields_are_big_endian() {
        let bytes = encode(0x1234, 0x0A04, &[1, 2, 3]);
        assert_eq!(&bytes[..7], &[0x01, 0x12, 0x34, 0x00, 0x03, 0x0A, 0x04]);
        let got = Framer::new().feed(&bytes);
        assert_eq!(got[0].id, 0x1234);
        assert_eq!(got[0].ty, 0x0A04);
        assert_eq!(got[0].payload, vec![1, 2, 3]);
    }

    #[test]
    fn resyncs_after_leading_garbage_including_a_stray_sof() {
        let mut bytes = vec![0xFF, 0x00, 0x01, 0x42, 0x13, 0x37, 0x99, 0x01, 0x02];
        bytes.extend_from_slice(VENDOR_EMPTY_CLOUD);
        let mut f = Framer::new();
        let got = f.feed(&bytes);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ty, 0x0A08);
        assert!(f.stats().header_checksum_errors >= 1);
        assert_eq!(f.stats().resync_bytes, 9);
    }

    #[test]
    fn bad_data_checksum_drops_the_frame_and_recovers() {
        let mut bad = VENDOR_TRIGGER_FAST.to_vec();
        let last = bad.len() - 1;
        bad[last] ^= 0x5A;
        bad.extend_from_slice(VENDOR_EMPTY_CLOUD);
        let mut f = Framer::new();
        let got = f.feed(&bad);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ty, 0x0A08);
        assert_eq!(f.stats().data_checksum_errors, 1);
    }

    #[test]
    fn corrupted_header_is_skipped() {
        let mut bad = VENDOR_TRIGGER_FAST.to_vec();
        bad[4] ^= 0x01;
        bad.extend_from_slice(VENDOR_EMPTY_CLOUD);
        let mut f = Framer::new();
        let got = f.feed(&bad);
        assert_eq!(got.len(), 1);
        assert!(f.stats().header_checksum_errors >= 1);
    }

    #[test]
    fn oversize_length_is_desync() {
        let mut hdr = vec![SOF, 0x00, 0x00, 0x03, 0xF8, 0x0A, 0x04]; // len 1016 > MAX
        hdr.push(cksum(&hdr));
        hdr.extend_from_slice(VENDOR_EMPTY_CLOUD);
        let mut f = Framer::new();
        let got = f.feed(&hdr);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].ty, 0x0A08);
        assert_eq!(f.stats().oversize_frames, 1);
    }

    #[test]
    fn frame_split_across_every_boundary() {
        let bytes = [
            VENDOR_EMPTY_CLOUD,
            VENDOR_TRIGGER_FAST,
            encode(3, 0x0A14, &[0x01]).as_slice(),
        ]
        .concat();
        for cut in 0..=bytes.len() {
            let mut f = Framer::new();
            let mut got = f.feed(&bytes[..cut]);
            got.extend(f.feed(&bytes[cut..]));
            assert_eq!(got.len(), 3, "split at {cut}");
            assert_eq!(f.stats().resync_bytes, 0, "split at {cut}");
        }
    }

    #[test]
    fn byte_at_a_time_matches_bulk() {
        let bytes = [
            vec![0x55, 0x01],
            VENDOR_EMPTY_CLOUD.to_vec(),
            VENDOR_TRIGGER_FAST.to_vec(),
        ]
        .concat();
        let bulk = Framer::new().feed(&bytes);
        let mut f = Framer::new();
        let trickle: Vec<Frame> = bytes.iter().flat_map(|b| f.feed(&[*b])).collect();
        assert_eq!(bulk, trickle);
    }

    #[test]
    fn unknown_type_passes_through_without_a_meaning() {
        let got = Framer::new().feed(&encode(9, 0xBEEF, &[1, 2, 3, 4]));
        assert_eq!(got[0].ty, 0xBEEF);
    }

    #[test]
    fn buffer_does_not_grow_on_noise() {
        let mut f = Framer::new();
        for _ in 0..1000 {
            f.feed(&[0xFFu8; 64]);
        }
        assert!(f.buf.len() < HEADER_LEN);
        assert_eq!(f.stats().frames, 0);
    }

    #[test]
    fn section_6_1_example_checksum_does_not_match_the_algorithm() {
        // Protocol §6.1 prints header checksum F3 for 01 00 00 00 04 02 01.
        // §7.2 yields F9. The cog follows §7.2, not that example byte.
        let hdr = [0x01u8, 0x00, 0x00, 0x00, 0x04, 0x02, 0x01];
        assert_eq!(cksum(&hdr), 0xF9);
        assert_ne!(cksum(&hdr), 0xF3);
    }
}
