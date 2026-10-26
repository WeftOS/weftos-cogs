//! Memory probes. The NPU's weights live in DMA memory that a process's RSS
//! does not count, so a model's cost is the drop in system `MemAvailable`
//! while it loads, floored by the serving process's own RSS.

use std::path::Path;

fn kib_field(text: &str, key: &str) -> Option<u64> {
    text.lines()
        .find(|l| l.starts_with(key))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse::<u64>().ok())
}

/// `MemAvailable` in MiB from `<root>/proc/meminfo`.
pub fn mem_available_mib(root: &Path) -> Option<u64> {
    let t = std::fs::read_to_string(root.join("proc/meminfo")).ok()?;
    kib_field(&t, "MemAvailable:").map(|k| k / 1024)
}

/// `VmRSS` of `pid` in MiB from `<root>/proc/<pid>/status`.
pub fn rss_mib(root: &Path, pid: u32) -> Option<u64> {
    let t = std::fs::read_to_string(root.join(format!("proc/{pid}/status"))).ok()?;
    kib_field(&t, "VmRSS:").map(|k| k / 1024)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_meminfo_and_status() {
        let d = std::env::temp_dir().join(format!("npu-infer-mem-{}", std::process::id()));
        std::fs::create_dir_all(d.join("proc/42")).unwrap();
        std::fs::write(
            d.join("proc/meminfo"),
            "MemTotal: 100 kB\nMemAvailable:    8555592 kB\n",
        )
        .unwrap();
        std::fs::write(
            d.join("proc/42/status"),
            "Name:\tgeniex\nVmRSS:\t  433972 kB\n",
        )
        .unwrap();
        assert_eq!(mem_available_mib(&d), Some(8355));
        assert_eq!(rss_mib(&d, 42), Some(423));
        assert_eq!(rss_mib(&d, 43), None);
        std::fs::remove_dir_all(&d).ok();
    }
}
