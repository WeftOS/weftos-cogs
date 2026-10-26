//! Which V4L2 node the cog opens, how frames are captured, and where they are kept.
//!
//! The node comes from the host: it hands the owner `WEFTOS_DEVICES` (ADR-122 4b) and the
//! cog picks the capture node among the `/dev/video*` entries in it. A UVC camera shows two
//! nodes (video, metadata); the capture node is the one whose sysfs `index` is 0.
//! Frames are captured as MJPEG by a `gst-launch-1.0` child (`v4l2src ! multifilesink`),
//! kept in the data directory under their BLAKE3 hash for `--ttl-ms`, and announced as
//! `video.frame-ref.v1` readings. The bytes never go on the bus.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

/// `/dev/video12` -> 12.
pub fn video_number(path: &str) -> Option<u32> {
    path.strip_prefix("/dev/video")?.parse().ok()
}

/// Pick the capture node from the handed devices. `sysfs` reads a file under
/// `/sys/class/video4linux/<name>/` (`index`, `name`); `want` is a substring of the node's
/// name. Without `want` the lowest capture node is used.
pub fn pick_device(
    devices: &[String],
    sysfs: &dyn Fn(&str, &str) -> Option<String>,
    want: Option<&str>,
) -> Result<String, String> {
    let mut nodes: Vec<(u32, String)> = devices
        .iter()
        .filter_map(|d| Some((video_number(d)?, d.clone())))
        .collect();
    nodes.sort();
    if nodes.is_empty() {
        return Err(
            "no video node in WEFTOS_DEVICES (the host hands the owner /dev/video*)".into(),
        );
    }
    let capture = |n: &u32| sysfs(&format!("video{n}"), "index").is_none_or(|i| i.trim() == "0");
    let named = |n: &u32| match want {
        None => true,
        Some(w) => sysfs(&format!("video{n}"), "name")
            .is_some_and(|s| s.to_lowercase().contains(&w.to_lowercase())),
    };
    nodes
        .into_iter()
        .find(|(n, _)| capture(n) && named(n))
        .map(|(_, p)| p)
        .ok_or_else(|| format!("no handed capture node matches {:?}", want.unwrap_or("")))
}

/// Width and height from a JPEG's first start-of-frame marker.
pub fn jpeg_dims(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 4 || b[0] != 0xff || b[1] != 0xd8 {
        return None;
    }
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xff {
            i += 1;
            continue;
        }
        let m = b[i + 1];
        if m == 0xff || m == 0x00 || (0xd0..=0xd8).contains(&m) || m == 0x01 {
            i += if m == 0xff { 1 } else { 2 };
            continue;
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        if matches!(m, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            let s = b.get(i + 4..i + 9)?;
            let h = u16::from_be_bytes([s[1], s[2]]) as u32;
            let w = u16::from_be_bytes([s[3], s[4]]) as u32;
            return (w > 0 && h > 0).then_some((w, h));
        }
        i += 2 + len;
    }
    None
}

/// A frame that carries the structure the cog reads (SOI, a comment with the counter, a
/// start-of-frame with the size, EOI) and no image data: for `--simulate`, never decodable.
pub fn synthetic_jpeg(n: u64, w: u16, h: u16) -> Vec<u8> {
    let mut v = vec![0xff, 0xd8];
    let com = format!("SYNTHETIC frame {n}");
    v.extend_from_slice(&[0xff, 0xfe]);
    v.extend_from_slice(&((com.len() + 2) as u16).to_be_bytes());
    v.extend_from_slice(com.as_bytes());
    v.extend_from_slice(&[0xff, 0xc0, 0x00, 0x0b, 0x08]);
    v.extend_from_slice(&h.to_be_bytes());
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
    v.extend_from_slice(&[0xff, 0xd9]);
    v
}

/// Frames kept for their ttl, named by hash.
pub struct FrameStore {
    dir: PathBuf,
    ttl: Duration,
    last_gc: Instant,
}

impl FrameStore {
    pub fn new(dir: PathBuf, ttl: Duration) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("frames dir {}: {e}", dir.display()))?;
        Ok(Self {
            dir,
            ttl,
            last_gc: Instant::now(),
        })
    }

    /// Store `bytes`; returns the BLAKE3 hash (lower-case hex, 64) it is kept under.
    pub fn put(&self, bytes: &[u8]) -> Result<String, String> {
        let hash = blake3::hash(bytes).to_hex().to_string();
        let path = self.dir.join(format!("{hash}.jpg"));
        let tmp = self.dir.join(format!(".{hash}.tmp"));
        std::fs::write(&tmp, bytes)
            .and_then(|_| std::fs::rename(&tmp, &path))
            .map_err(|e| format!("store frame: {e}"))?;
        Ok(hash)
    }

    #[cfg(test)]
    pub fn path(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.jpg"))
    }

    /// Delete frames older than the ttl; at most once a second. Returns how many.
    pub fn gc(&mut self) -> usize {
        if self.last_gc.elapsed() < Duration::from_secs(1) {
            return 0;
        }
        self.last_gc = Instant::now();
        let now = SystemTime::now();
        let mut n = 0;
        for e in std::fs::read_dir(&self.dir).into_iter().flatten().flatten() {
            let old = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| now.duration_since(t).ok())
                .is_some_and(|age| age > self.ttl);
            if old
                && e.path().extension().is_some_and(|x| x == "jpg")
                && std::fs::remove_file(e.path()).is_ok()
            {
                n += 1;
            }
        }
        n
    }
}

/// `raw000123.jpg` -> 123.
fn raw_index(p: &Path) -> Option<u64> {
    p.file_name()?
        .to_str()?
        .strip_prefix("raw")?
        .strip_suffix(".jpg")?
        .parse()
        .ok()
}

/// The oldest frames in `dir` that are complete: a file is complete once the next index exists.
/// Returns them in order, `after` being the last index already taken.
pub fn complete_frames(dir: &Path, after: Option<u64>) -> Vec<(u64, PathBuf)> {
    let mut all: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((raw_index(&e.path())?, e.path())))
        .collect();
    all.sort();
    let newest = all.last().map(|(i, _)| *i);
    all.into_iter()
        .filter(|(i, _)| Some(*i) != newest && after.is_none_or(|a| *i > a))
        .collect()
}

/// A running `gst-launch-1.0` capture; it dies with this value.
pub struct Capture {
    child: Child,
    pub dir: PathBuf,
    taken: Option<u64>,
}

fn pipeline(dev: &str, w: u32, h: u32, fps: u32, dir: &Path, buffers: Option<u32>) -> Vec<String> {
    // The camera offers a few fixed rates (the U20CAM: 30, not 10 or 2), so ask for its own
    // rate and let videorate drop to the requested one.
    let mut a: Vec<String> = vec!["-q".into(), "v4l2src".into(), format!("device={dev}")];
    if let Some(n) = buffers {
        a.push(format!("num-buffers={n}"));
    }
    a.extend([
        "!".to_string(),
        format!("image/jpeg,width={w},height={h}"),
        "!".into(),
        "videorate".into(),
        "!".into(),
        format!("image/jpeg,framerate={fps}/1"),
        "!".into(),
        "multifilesink".into(),
        format!("location={}/raw%06d.jpg", dir.display()),
        "max-files=16".into(),
    ]);
    a
}

impl Capture {
    pub fn start(
        dev: &str,
        w: u32,
        h: u32,
        fps: u32,
        dir: PathBuf,
        buffers: Option<u32>,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&dir).map_err(|e| format!("capture dir: {e}"))?;
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if raw_index(&e.path()).is_some() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        let child = Command::new("gst-launch-1.0")
            .args(pipeline(dev, w, h, fps, &dir, buffers))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("gst-launch-1.0: {e}"))?;
        Ok(Self {
            child,
            dir,
            taken: None,
        })
    }

    /// Complete frames not yet taken, oldest first.
    pub fn poll(&mut self) -> Vec<PathBuf> {
        let v = complete_frames(&self.dir, self.taken);
        if let Some((i, _)) = v.last() {
            self.taken = Some(*i);
        }
        v.into_iter().map(|(_, p)| p).collect()
    }

    /// The child's exit status, once it has exited.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// For a bounded capture: wait for the child, then every frame it wrote (the last is complete too).
    pub fn finish(&mut self, wait: Duration) -> Result<Vec<PathBuf>, String> {
        let end = Instant::now() + wait;
        loop {
            if let Some(st) = self.exited() {
                if !st.success() {
                    return Err(format!("gst-launch-1.0 {st}"));
                }
                break;
            }
            if Instant::now() >= end {
                return Err("the camera delivered no frames in time".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let mut all: Vec<(u64, PathBuf)> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| Some((raw_index(&e.path())?, e.path())))
            .collect();
        all.sort();
        Ok(all.into_iter().map(|(_, p)| p).collect())
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn devs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn fake_sysfs(node: &str, file: &str) -> Option<String> {
        match (node, file) {
            ("video0", "index") | ("video2", "index") => Some("0\n".into()),
            ("video1", "index") => Some("1\n".into()),
            ("video0", "name") => Some("Innomaker U20CAM-1080p\n".into()),
            ("video1", "name") => Some("Innomaker U20CAM-1080p\n".into()),
            ("video2", "name") => Some("Qualcomm camss\n".into()),
            _ => None,
        }
    }

    #[test]
    fn the_capture_node_is_the_one_with_index_zero() {
        let d = devs(&[
            "/dev/video1",
            "/dev/video0",
            "/dev/video2",
            "/dev/snd/pcmC1D0c",
        ]);
        assert_eq!(
            pick_device(&d, &fake_sysfs, None).unwrap(),
            "/dev/video0",
            "the metadata node (index 1) is skipped"
        );
        assert_eq!(
            pick_device(&d, &fake_sysfs, Some("camss")).unwrap(),
            "/dev/video2"
        );
        assert_eq!(
            pick_device(&devs(&["/dev/video1"]), &fake_sysfs, None).unwrap_err(),
            "no handed capture node matches \"\""
        );
        assert!(pick_device(&d, &fake_sysfs, Some("nope")).is_err());
        assert!(
            pick_device(&devs(&["/dev/snd/pcmC1D0c"]), &fake_sysfs, None)
                .unwrap_err()
                .contains("no video node")
        );
        assert_eq!(
            pick_device(&devs(&["/dev/video7"]), &|_, _| None, None).unwrap(),
            "/dev/video7",
            "no sysfs: take it"
        );
    }

    #[test]
    fn jpeg_size_comes_from_the_start_of_frame() {
        let j = synthetic_jpeg(1, 1280, 720);
        assert_eq!(jpeg_dims(&j), Some((1280, 720)));
        // A real-shaped file: APP0, then a DQT segment before SOF2 (progressive).
        let mut r = vec![
            0xff, 0xd8, 0xff, 0xe0, 0x00, 0x04, 0, 0, 0xff, 0xdb, 0x00, 0x03, 0x00,
        ];
        r.extend_from_slice(&[
            0xff, 0xc2, 0x00, 0x0b, 0x08, 0x04, 0x38, 0x07, 0x80, 0x01, 0x01, 0x11, 0x00, 0xff,
            0xd9,
        ]);
        assert_eq!(jpeg_dims(&r), Some((1920, 1080)));
        assert_eq!(jpeg_dims(&[1, 2, 3, 4]), None);
        assert_eq!(jpeg_dims(&[0xff, 0xd8, 0xff, 0xd9]), None);
    }

    #[test]
    fn synthetic_frames_differ_by_counter_so_their_hashes_do() {
        assert_ne!(synthetic_jpeg(1, 640, 480), synthetic_jpeg(2, 640, 480));
        assert_eq!(synthetic_jpeg(1, 640, 480), synthetic_jpeg(1, 640, 480));
    }

    #[test]
    fn the_store_names_a_frame_by_its_hash_and_collects_old_ones() {
        let dir = std::env::temp_dir().join(format!("camera-store-{}", std::process::id()));
        let mut s = FrameStore::new(dir.clone(), Duration::from_millis(0)).unwrap();
        let bytes = synthetic_jpeg(1, 64, 48);
        let h = s.put(&bytes).unwrap();
        assert_eq!(h, blake3::hash(&bytes).to_hex().to_string());
        assert_eq!(h.len(), 64);
        assert_eq!(std::fs::read(s.path(&h)).unwrap(), bytes);
        std::thread::sleep(Duration::from_millis(1100));
        s.last_gc = Instant::now() - Duration::from_secs(5);
        assert_eq!(s.gc(), 1);
        assert!(!s.path(&h).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_frame_is_complete_once_the_next_one_exists() {
        let dir = std::env::temp_dir().join(format!("camera-poll-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let w = |i: u64| std::fs::write(dir.join(format!("raw{i:06}.jpg")), b"x").unwrap();
        assert!(complete_frames(&dir, None).is_empty());
        w(0);
        assert!(
            complete_frames(&dir, None).is_empty(),
            "the only file may still be written"
        );
        w(1);
        w(2);
        let got: Vec<u64> = complete_frames(&dir, None)
            .iter()
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(got, [0, 1]);
        let got: Vec<u64> = complete_frames(&dir, Some(0))
            .iter()
            .map(|(i, _)| *i)
            .collect();
        assert_eq!(got, [1], "already taken frames are not returned again");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_pipeline_asks_for_mjpeg_at_the_size_and_rate() {
        let a = pipeline("/dev/video0", 1280, 720, 5, Path::new("/d"), Some(3)).join(" ");
        assert!(a.contains("device=/dev/video0 num-buffers=3 ! image/jpeg,width=1280,height=720 ! videorate ! image/jpeg,framerate=5/1 ! multifilesink location=/d/raw%06d.jpg"), "{a}");
        assert!(!pipeline("/dev/video0", 1, 1, 1, Path::new("/d"), None)
            .join(" ")
            .contains("num-buffers"));
    }
}
