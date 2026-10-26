//! Who shares the Pluto's USB root hub, from sysfs.
//!
//! USB 2.0 gives one 480 Mb/s budget to everything on a root hub. An isochronous device (a camera
//! or a microphone streaming) reserves its share before bulk transfers get any, and the Pluto's
//! samples are bulk. Measured on the Ventuno Q: 5.00 MS/s clean, 4.33 MS/s with a 720p MJPEG
//! camera streaming on the same hub. So the cog lists the other devices on its hub, says whether
//! any of them is streaming isochronously right now, and advises a lower top rate when one is.

use std::path::Path;

use serde_json::{json, Value};

const PLUTO: (&str, &str) = ("0456", "b673");
/// Safe continuous rate with the hub to itself (measured gap-free for an hour).
pub const ADVISED_ALONE_SPS: u32 = 5_000_000;
/// With an isochronous device streaming (measured ceiling 4.33 MS/s).
pub const ADVISED_SHARED_SPS: u32 = 4_000_000;

fn read(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn class_name(c: &str) -> &'static str {
    match c {
        "01" => "audio",
        "0e" => "video",
        "03" => "hid",
        "08" => "storage",
        "02" | "0a" => "comms",
        "09" => "hub",
        "ff" => "vendor",
        _ => "other",
    }
}

/// True if an interface directory currently has an isochronous endpoint (the active alternate
/// setting of a streaming camera or microphone).
fn has_iso(iface: &Path) -> bool {
    std::fs::read_dir(iface)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("ep_"))
        .filter_map(|e| read(&e.path().join("bmAttributes")))
        .filter_map(|a| u8::from_str_radix(&a, 16).ok())
        .any(|a| a & 3 == 1)
}

/// The hub report for a sysfs tree (`/sys/bus/usb/devices`), or None without a Pluto in it.
pub fn scan(root: &Path) -> Option<Value> {
    let mut devs: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(n, _)| !n.contains(':') && !n.starts_with("usb") && n.contains('-'))
        .collect();
    devs.sort();
    let is_pluto = |p: &Path| {
        read(&p.join("idVendor")).as_deref() == Some(PLUTO.0)
            && read(&p.join("idProduct")).as_deref() == Some(PLUTO.1)
    };
    let (pname, _) = devs.iter().find(|(_, p)| is_pluto(p))?;
    let bus = pname.split('-').next()?.to_string();
    let hub = root.join(format!("usb{bus}"));
    let mut others = Vec::new();
    let mut iso_any = false;
    for (name, path) in devs
        .iter()
        .filter(|(n, _)| n.split('-').next() == Some(bus.as_str()) && n != pname)
    {
        let mut classes = Vec::new();
        let mut iso = false;
        for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
            let en = e.file_name().to_string_lossy().into_owned();
            if en.starts_with(&format!("{name}:")) {
                if let Some(c) = read(&e.path().join("bInterfaceClass")) {
                    let c = class_name(&c);
                    if !classes.contains(&c) {
                        classes.push(c);
                    }
                }
                iso |= has_iso(&e.path());
            }
        }
        if classes == ["hub"] {
            continue;
        }
        iso_any |= iso;
        others.push(json!({
            "port": name,
            "id": format!("{}:{}", read(&path.join("idVendor")).unwrap_or_default(), read(&path.join("idProduct")).unwrap_or_default()),
            "product": read(&path.join("product")),
            "classes": classes,
            "isochronous_active": iso,
        }));
    }
    let advised = if iso_any {
        ADVISED_SHARED_SPS
    } else {
        ADVISED_ALONE_SPS
    };
    Some(json!({
        "bus": bus,
        "root_hub_speed_mbps": read(&hub.join("speed")),
        "shared_with": others,
        "isochronous_active": iso_any,
        "advised_max_rate_sps": advised,
        "advice": if iso_any {
            "an isochronous device (camera or audio) is streaming on this hub; 5 MS/s loses samples, stay at 4 MS/s or less"
        } else if !others.is_empty() {
            "other devices share this hub but none is streaming now; 5 MS/s holds until one starts"
        } else {
            "the hub is not shared"
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn put(p: &Path, f: &str, v: &str) {
        fs::create_dir_all(p).unwrap();
        fs::write(p.join(f), format!("{v}\n")).unwrap();
    }

    /// A tree shaped like the Ventuno Q's: the Pluto on port 2, a UVC camera with audio on port 4.
    fn tree(camera_streaming: bool) -> std::path::PathBuf {
        let r =
            std::env::temp_dir().join(format!("sdr-hub-{}-{camera_streaming}", std::process::id()));
        fs::remove_dir_all(&r).ok();
        put(&r.join("usb1"), "speed", "480");
        put(&r.join("1-2"), "idVendor", "0456");
        put(&r.join("1-2"), "idProduct", "b673");
        put(&r.join("1-2"), "product", "PlutoSDR (ADALM-PLUTO)");
        put(&r.join("1-2/1-2:1.0"), "bInterfaceClass", "02");
        put(&r.join("1-4"), "idVendor", "046d");
        put(&r.join("1-4"), "idProduct", "0825");
        put(&r.join("1-4"), "product", "Webcam");
        put(&r.join("1-4/1-4:1.0"), "bInterfaceClass", "0e");
        put(&r.join("1-4/1-4:1.1"), "bInterfaceClass", "0e");
        put(&r.join("1-4/1-4:1.2"), "bInterfaceClass", "01");
        // the camera's streaming interface has an isochronous endpoint only while it streams
        if camera_streaming {
            put(&r.join("1-4/1-4:1.1/ep_81"), "bmAttributes", "05");
        } else {
            put(&r.join("1-4/1-4:1.1/ep_81"), "bmAttributes", "02");
        }
        put(&r.join("2-1"), "idVendor", "1234");
        put(&r.join("2-1"), "idProduct", "0001");
        r
    }

    #[test]
    fn an_idle_camera_on_the_hub_is_listed_but_does_not_lower_the_advice() {
        let v = scan(&tree(false)).unwrap();
        assert_eq!(v["bus"], "1");
        assert_eq!(v["root_hub_speed_mbps"], "480");
        let o = v["shared_with"].as_array().unwrap();
        assert_eq!(o.len(), 1, "the device on another bus is not listed");
        assert_eq!(o[0]["id"], "046d:0825");
        assert_eq!(o[0]["classes"], json!(["video", "audio"]));
        assert_eq!(v["isochronous_active"], false);
        assert_eq!(v["advised_max_rate_sps"], 5_000_000);
    }

    #[test]
    fn a_streaming_camera_lowers_the_advised_rate() {
        let v = scan(&tree(true)).unwrap();
        assert_eq!(v["isochronous_active"], true);
        assert_eq!(v["shared_with"][0]["isochronous_active"], true);
        assert_eq!(v["advised_max_rate_sps"], 4_000_000);
        assert!(v["advice"].as_str().unwrap().contains("4 MS/s"));
    }

    #[test]
    fn no_pluto_or_no_sysfs_means_no_report() {
        assert!(scan(Path::new("/nonexistent/usb")).is_none());
        let r = std::env::temp_dir().join(format!("sdr-hub-empty-{}", std::process::id()));
        put(&r.join("1-4"), "idVendor", "046d");
        assert!(scan(&r).is_none());
    }
}
