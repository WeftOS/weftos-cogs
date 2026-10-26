//! Starting GenieX. Always through the wrapper (`~/.local/bin/geniex`): the
//! bare binary cannot find `libgeniex.so`. One `geniex serve` per resident
//! model, each on its own loopback port, so freeing a model is killing its
//! process (GenieX has no unload call). The child dies with this cog
//! (`PR_SET_PDEATHSIG`), so a crashed or stopped cog never leaves a model
//! holding NPU memory.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

use crate::catalog::Entry;

pub trait Proc: Send {
    /// The end of the server's own output, for an error message ("" if none kept).
    fn diagnostic(&self) -> String {
        String::new()
    }
    fn alive(&mut self) -> bool;
    fn pid(&self) -> Option<u32>;
    fn kill(&mut self);
}

pub trait Launcher: Send {
    /// Start a server for `e` on loopback `port`.
    fn spawn(&mut self, port: u16, e: &Entry) -> Result<Box<dyn Proc>, String>;
}

pub struct ChildProc(Child, Option<PathBuf>);

impl Proc for ChildProc {
    fn diagnostic(&self) -> String {
        let Some(p) = &self.1 else {
            return String::new();
        };
        let text = std::fs::read_to_string(p).unwrap_or_default();
        let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        first.chars().take(200).collect()
    }

    fn alive(&mut self) -> bool {
        matches!(self.0.try_wait(), Ok(None))
    }

    fn pid(&self) -> Option<u32> {
        Some(self.0.id())
    }

    fn kill(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for ChildProc {
    fn drop(&mut self) {
        self.kill();
    }
}

pub struct WrapperLauncher {
    pub bin: PathBuf,
    /// Where each server's output goes (`geniex-<port>.log`); discarded when `None`.
    pub log_dir: Option<PathBuf>,
    /// `$HOME` for GenieX, which finds its model cache through it and refuses
    /// to start without one. A placed cog has none, so it is set explicitly.
    pub home: PathBuf,
}

/// The home of the user the wrapper lives under: `<home>/.local/bin/geniex`.
pub fn home_of(wrapper: &std::path::Path) -> Option<PathBuf> {
    let bin = wrapper.parent()?;
    let local = bin.parent()?;
    let home = local.parent()?;
    (bin.file_name()? == "bin" && local.file_name()? == ".local").then(|| home.to_path_buf())
}

impl Launcher for WrapperLauncher {
    fn spawn(&mut self, port: u16, e: &Entry) -> Result<Box<dyn Proc>, String> {
        let mut cmd = Command::new(&self.bin);
        // A known environment, whatever the cog was started with.
        cmd.env_clear()
            .env("HOME", &self.home)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin");
        cmd.args(["--skip-update", "serve", "--host"])
            .arg(format!("127.0.0.1:{port}"))
            .args(["--compute", "npu", "--nctx"])
            .arg(e.nctx.to_string())
            // The cog decides when a model goes; GenieX must not.
            .args(["--keepalive", "86400"])
            .stdin(Stdio::null());
        let log = self
            .log_dir
            .as_ref()
            .map(|d| d.join(format!("geniex-{port}.log")));
        match log.as_ref().and_then(|p| {
            std::fs::create_dir_all(p.parent()?).ok()?;
            std::fs::File::create(p).ok()
        }) {
            Some(f) => {
                let g = f.try_clone().map_err(|e| e.to_string())?;
                cmd.stdout(f).stderr(g);
            }
            None => {
                cmd.stdout(Stdio::null()).stderr(Stdio::null());
            }
        }
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::process::CommandExt;
            // SAFETY: only async-signal-safe prctl between fork and exec.
            unsafe {
                cmd.pre_exec(|| {
                    libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong);
                    Ok(())
                });
            }
        }
        let child = cmd
            .spawn()
            .map_err(|err| format!("start {}: {err}", self.bin.display()))?;
        Ok(Box::new(ChildProc(child, log)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn home_is_found_from_the_wrapper_path() {
        assert_eq!(
            home_of(Path::new("/home/arduino/.local/bin/geniex")),
            Some(PathBuf::from("/home/arduino"))
        );
        assert_eq!(home_of(Path::new("/usr/bin/geniex")), None);
        assert_eq!(home_of(Path::new("geniex")), None);
    }

    // Linux only: on macOS a just-written script sometimes never starts under the
    // test harness (an OS scan, not the launcher), which made this test flaky.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_server_gets_a_clean_environment_with_a_home() {
        let dir = std::env::temp_dir().join(format!("npu-infer-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let wrapper = dir.join("geniex");
        // Stands in for the wrapper: writes what it was given to its log.
        std::fs::write(&wrapper, "#!/bin/sh\nenv | sort\necho \"args: $*\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut l = WrapperLauncher {
            bin: wrapper,
            log_dir: Some(dir.clone()),
            home: PathBuf::from("/home/tester"),
        };
        let e = crate::catalog::Catalog::ventuno()
            .resolve("gemma3-1b")
            .unwrap()
            .clone();
        let mut p = l.spawn(18999, &e).unwrap();
        for _ in 0..500 {
            if !p.alive() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let out = std::fs::read_to_string(dir.join("geniex-18999.log")).unwrap();
        assert!(
            out.contains("HOME=/home/tester") && out.contains("PATH=/usr/local/bin:/usr/bin:/bin"),
            "{out}"
        );
        assert!(
            !out.contains("WEFTOS_COGBUS") && !out.contains("COGNITUM"),
            "the cog's own environment does not leak: {out}"
        );
        assert!(
            out.contains("args: --skip-update serve --host 127.0.0.1:18999 --compute npu --nctx"),
            "{out}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
