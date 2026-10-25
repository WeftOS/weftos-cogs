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
    fn alive(&mut self) -> bool;
    fn pid(&self) -> Option<u32>;
    fn kill(&mut self);
}

pub trait Launcher: Send {
    /// Start a server for `e` on loopback `port`.
    fn spawn(&mut self, port: u16, e: &Entry) -> Result<Box<dyn Proc>, String>;
}

pub struct ChildProc(Child);

impl Proc for ChildProc {
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
}

impl Launcher for WrapperLauncher {
    fn spawn(&mut self, port: u16, e: &Entry) -> Result<Box<dyn Proc>, String> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(["--skip-update", "serve", "--host"])
            .arg(format!("127.0.0.1:{port}"))
            .args(["--compute", "npu", "--nctx"])
            .arg(e.nctx.to_string())
            // The cog decides when a model goes; GenieX must not.
            .args(["--keepalive", "86400"])
            .stdin(Stdio::null());
        match self.log_dir.as_ref().and_then(|d| {
            std::fs::create_dir_all(d).ok()?;
            std::fs::File::create(d.join(format!("geniex-{port}.log"))).ok()
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
        Ok(Box::new(ChildProc(child)))
    }
}
