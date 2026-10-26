//! The cogbus stream ring (weftos ADR-122 section 3 and B2) as a standalone client: a shared
//! memory object handed over the broker socket, one writer, many readers, every slot a seqlock.
//! This is the layout of `weftos-cogbus/src/stream/ring.rs`; the cog does not link the weftos
//! crate, which keeps it a standalone package. This copy keeps the reader; the writer is for tests.
//!
//! ```text
//! header (128 bytes, 8-byte words)
//!   0 magic "COGRING1"  8 version (1)  16 slot_count  24 slot_size  32 max_record  40 ring_id
//!   64 head: seq of the newest committed record (0 = none yet)   72 writer_epoch
//! slot i at 128 + i * slot_size
//!   0 seq (0 = empty, BUSY while written)  8 len  16 t_ns  24 w_ns  32 payload (LE words)
//! ```

use std::os::fd::{AsRawFd, BorrowedFd};
use std::ptr::NonNull;
use std::sync::atomic::{fence, AtomicU64, Ordering};

const HEADER: usize = 128;
const SLOT_HEADER: usize = 32;
#[cfg(test)]
const BUSY: u64 = u64::MAX;
const OFF_HEAD: usize = 64;
#[cfg(test)]
const OFF_EPOCH: usize = 72;

/// Shape of a ring, as the broker states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub slot_count: u64,
    pub slot_size: u64,
    pub max_record: u64,
}

impl Geometry {
    pub fn from_json(v: &serde_json::Value) -> Option<Self> {
        let g = Self {
            slot_count: v["slot_count"].as_u64()?,
            slot_size: v["slot_size"].as_u64()?,
            max_record: v["max_record"].as_u64()?,
        };
        g.valid().then_some(g)
    }

    pub fn bytes(&self) -> usize {
        HEADER + (self.slot_count * self.slot_size) as usize
    }

    fn valid(&self) -> bool {
        self.slot_count >= 2
            && self.slot_size.is_multiple_of(64)
            && self.slot_size >= SLOT_HEADER as u64 + self.max_record.div_ceil(8) * 8
            && self.slot_count.checked_mul(self.slot_size).is_some()
    }

    fn slot(&self, seq: u64) -> usize {
        HEADER + (((seq - 1) % self.slot_count) * self.slot_size) as usize
    }
}

/// One record read from a ring.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub seq: u64,
    pub t_ns: u64,
    pub w_ns: u64,
    pub data: Vec<u8>,
}

/// A mapping of a shared-memory object.
#[derive(Debug)]
pub struct Mapping {
    ptr: NonNull<u8>,
    len: usize,
    #[cfg_attr(not(test), allow(dead_code))]
    writable: bool,
}

// SAFETY: plain shared memory, touched only through atomics.
unsafe impl Send for Mapping {}
// SAFETY: as above.
unsafe impl Sync for Mapping {}

impl Mapping {
    /// Map `len` bytes of `fd`, refusing an object smaller than that (an access past its end
    /// would fault).
    pub fn new(fd: BorrowedFd<'_>, len: usize, writable: bool) -> std::io::Result<Self> {
        // SAFETY: fstat fills `st` for a descriptor we hold.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if len == 0 || (st.st_size as u64) < len as u64 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("shm holds {} bytes, need {len}", st.st_size),
            ));
        }
        let prot = if writable {
            libc::PROT_READ | libc::PROT_WRITE
        } else {
            libc::PROT_READ
        };
        // SAFETY: a fresh shared mapping of a descriptor we hold; the result is checked.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                prot,
                libc::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(std::io::Error::last_os_error());
        }
        let ptr = NonNull::new(p.cast::<u8>()).ok_or_else(std::io::Error::last_os_error)?;
        Ok(Self { ptr, len, writable })
    }

    pub(crate) fn word(&self, off: usize) -> &AtomicU64 {
        assert!(
            off.is_multiple_of(8) && off + 8 <= self.len,
            "shm word {off} of {}",
            self.len
        );
        // SAFETY: in bounds and 8-aligned (mmap is page-aligned); lives as long as `self`.
        unsafe { &*self.ptr.as_ptr().add(off).cast::<AtomicU64>() }
    }
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly what `new` mapped.
        unsafe { libc::munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}

/// The writer's side of a ring (tests stand in for the owner cog).
#[cfg(test)]
#[derive(Debug)]
pub struct RingWriter {
    map: Mapping,
    g: Geometry,
    head: u64,
}

#[cfg(test)]
impl RingWriter {
    pub fn attach(map: Mapping, g: Geometry) -> Result<Self, String> {
        if !g.valid() || map.len < g.bytes() || !map.writable {
            return Err("ring geometry does not match its mapping".into());
        }
        let head = map.word(OFF_HEAD).load(Ordering::Acquire);
        map.word(OFF_EPOCH).fetch_add(1, Ordering::AcqRel);
        Ok(Self { map, g, head })
    }

    /// Commit one record and return its seq. `data` must fit `max_record`.
    pub fn write(&mut self, t_ns: u64, w_ns: u64, data: &[u8]) -> Result<u64, String> {
        if data.len() as u64 > self.g.max_record {
            return Err(format!(
                "record of {} bytes exceeds max_record {}",
                data.len(),
                self.g.max_record
            ));
        }
        let seq = self.head + 1;
        let base = self.g.slot(seq);
        let m = &self.map;
        m.word(base).store(BUSY, Ordering::Relaxed);
        fence(Ordering::Release);
        m.word(base + 8).store(data.len() as u64, Ordering::Relaxed);
        m.word(base + 16).store(t_ns, Ordering::Relaxed);
        m.word(base + 24).store(w_ns, Ordering::Relaxed);
        for (i, chunk) in data.chunks(8).enumerate() {
            let mut w = [0u8; 8];
            w[..chunk.len()].copy_from_slice(chunk);
            m.word(base + SLOT_HEADER + i * 8)
                .store(u64::from_le_bytes(w), Ordering::Relaxed);
        }
        m.word(base).store(seq, Ordering::Release);
        m.word(OFF_HEAD).store(seq, Ordering::Release);
        self.head = seq;
        Ok(seq)
    }
}

/// Write the header of a fresh ring (the broker does this; tests do it for a stand-in).
#[cfg(test)]
pub fn init(m: &Mapping, g: Geometry, ring_id: u64) {
    for (off, v) in [
        (0, u64::from_le_bytes(*b"COGRING1")),
        (8, 1),
        (16, g.slot_count),
        (24, g.slot_size),
        (32, g.max_record),
        (40, ring_id),
        (OFF_HEAD, 0),
    ] {
        m.word(off).store(v, Ordering::Relaxed);
    }
    fence(Ordering::Release);
}

/// A reader of a ring.
pub struct RingReader {
    map: Mapping,
    g: Geometry,
}

impl RingReader {
    pub fn attach(map: Mapping, g: Geometry) -> Self {
        Self { map, g }
    }

    /// The record at `*next` or the oldest still safe to read, advancing `*next` past it.
    pub fn read(&self, next: &mut u64) -> Option<Record> {
        let n = self.g.slot_count;
        for _ in 0..(2 * n + 16) {
            let head = self.map.word(OFF_HEAD).load(Ordering::Acquire);
            if head < *next {
                return None;
            }
            let oldest = (head + 2).saturating_sub(n).max(1);
            if *next < oldest {
                *next = oldest;
            }
            let base = self.g.slot(*next);
            let m = &self.map;
            let s1 = m.word(base).load(Ordering::Acquire);
            if s1 != *next {
                if s1 < *next {
                    *next += 1;
                }
                continue;
            }
            let len = (m
                .word(base + 8)
                .load(Ordering::Relaxed)
                .min(self.g.max_record)) as usize;
            let t_ns = m.word(base + 16).load(Ordering::Relaxed);
            let w_ns = m.word(base + 24).load(Ordering::Relaxed);
            let mut data = Vec::with_capacity(len.div_ceil(8) * 8);
            for i in 0..len.div_ceil(8) {
                let w = m.word(base + SLOT_HEADER + i * 8).load(Ordering::Relaxed);
                data.extend_from_slice(&w.to_le_bytes());
            }
            data.truncate(len);
            fence(Ordering::Acquire);
            if m.word(base).load(Ordering::Relaxed) != s1 {
                continue;
            }
            let seq = *next;
            *next += 1;
            return Some(Record {
                seq,
                t_ns,
                w_ns,
                data,
            });
        }
        None
    }
}

/// A ring backed by an unlinked temp file, for tests.
#[cfg(test)]
pub fn test_ring(g: Geometry) -> (RingWriter, RingReader) {
    use std::io::Write;
    let path = std::env::temp_dir().join(format!(
        "sdr-ring-{}-{}",
        std::process::id(),
        std::thread::current()
            .name()
            .unwrap_or("t")
            .replace("::", "-")
    ));
    let mut f = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    f.write_all(&vec![0u8; g.bytes()]).unwrap();
    std::fs::remove_file(&path).ok();
    let rw = Mapping::new(std::os::fd::AsFd::as_fd(&f), g.bytes(), true).unwrap();
    init(&rw, g, 1);
    let ro = Mapping::new(std::os::fd::AsFd::as_fd(&f), g.bytes(), false).unwrap();
    (
        RingWriter::attach(rw, g).unwrap(),
        RingReader::attach(ro, g),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom() -> Geometry {
        Geometry {
            slot_count: 4,
            slot_size: 64,
            max_record: 24,
        }
    }

    #[test]
    fn records_round_trip_and_a_slow_reader_skips_forward() {
        let (mut w, r) = test_ring(geom());
        let mut next = 1;
        assert!(r.read(&mut next).is_none());
        for i in 1..=3u64 {
            assert_eq!(w.write(i * 10, i * 100, &[i as u8; 13]).unwrap(), i);
        }
        let a = r.read(&mut next).unwrap();
        assert_eq!((a.seq, a.t_ns, a.w_ns, a.data.len()), (1, 10, 100, 13));
        assert_eq!(a.data, vec![1u8; 13]);
        for i in 4..=9u64 {
            w.write(i, i, &[0u8; 8]).unwrap();
        }
        // four slots, one kept clear: the reader jumps to the oldest safe record
        let b = r.read(&mut next).unwrap();
        assert_eq!(b.seq, 9 + 2 - 4);
        assert!(w.write(0, 0, &[0u8; 25]).is_err());
    }

    #[test]
    fn geometry_from_the_broker_is_validated() {
        let ok = serde_json::json!({"slot_count": 4, "slot_size": 64, "max_record": 24});
        assert_eq!(Geometry::from_json(&ok), Some(geom()));
        let bad = serde_json::json!({"slot_count": 1, "slot_size": 64, "max_record": 24});
        assert_eq!(Geometry::from_json(&bad), None);
        let small = serde_json::json!({"slot_count": 4, "slot_size": 64, "max_record": 64});
        assert_eq!(Geometry::from_json(&small), None);
    }
}
