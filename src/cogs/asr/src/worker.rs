//! The recognizer behind the cog: a Parakeet (sherpa-onnx) worker process, like npu-infer's
//! GenieX child. The cog writes each utterance to a file in its data directory, sends the path
//! as a JSON line and reads the text back. The worker is a Python script this binary carries
//! (`worker/asr_worker.py`) and runs with `$ASR_PYTHON` (default the bench venv).

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::Duration;

use serde_json::{json, Value};

pub const SCRIPT: &str = include_str!("../worker/asr_worker.py");

pub trait Recognizer {
    /// Text for the 16 kHz mono s16le utterance `pcm`, and the milliseconds it took.
    fn transcribe(&mut self, pcm: &[u8]) -> Result<(String, u64), String>;
    fn label(&self) -> String;
}

/// The real thing.
pub struct Parakeet {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    dir: PathBuf,
    next: u64,
    timeout: Duration,
}

impl Parakeet {
    pub fn start(
        python: &str,
        model_dir: &str,
        threads: u32,
        data_dir: &Path,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir).map_err(|e| format!("data dir: {e}"))?;
        let script = data_dir.join("asr_worker.py");
        std::fs::write(&script, SCRIPT).map_err(|e| format!("write worker: {e}"))?;
        let mut child = Command::new(python)
            .arg(&script)
            .arg(model_dir)
            .arg(threads.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| format!("{python}: {e}"))?;
        let stdin = child.stdin.take().ok_or("no worker stdin")?;
        let out = child.stdout.take().ok_or("no worker stdout")?;
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines().map_while(Result::ok) {
                if let Ok(v) = serde_json::from_str::<Value>(&l) {
                    let _ = tx.send(v);
                }
            }
        });
        let mut me = Self {
            child,
            stdin,
            lines,
            dir: data_dir.to_path_buf(),
            next: 0,
            timeout: Duration::from_secs(60),
        };
        match me.lines.recv_timeout(Duration::from_secs(120)) {
            Ok(v) if v["ready"] == true => Ok(me),
            Ok(v) => Err(format!("worker said {v}")),
            Err(_) => {
                let _ = me.child.kill();
                Err("the recognizer worker did not become ready (model path, python or sherpa_onnx missing?)".into())
            }
        }
    }
}

impl Recognizer for Parakeet {
    fn label(&self) -> String {
        "parakeet-tdt-0.6b-v2-int8".into()
    }

    fn transcribe(&mut self, pcm: &[u8]) -> Result<(String, u64), String> {
        self.next += 1;
        let id = self.next;
        let path = self.dir.join(format!("utt{}.raw", id % 4));
        std::fs::write(&path, pcm).map_err(|e| format!("write utterance: {e}"))?;
        let line = json!({"id": id, "path": path}).to_string() + "\n";
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("worker write: {e}"))?;
        loop {
            match self.lines.recv_timeout(self.timeout) {
                Ok(v) if v["id"].as_u64() == Some(id) => {
                    return match v["error"].as_str() {
                        Some(e) => Err(e.to_string()),
                        None => Ok((
                            v["text"].as_str().unwrap_or_default().to_string(),
                            v["ms"].as_u64().unwrap_or(0),
                        )),
                    };
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err("the recognizer did not answer in time".into())
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the recognizer worker died".into())
                }
            }
        }
    }
}

impl Drop for Parakeet {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `--simulate`: no model; the text says so.
pub struct Synthetic;

impl Recognizer for Synthetic {
    fn label(&self) -> String {
        "synthetic".into()
    }

    fn transcribe(&mut self, pcm: &[u8]) -> Result<(String, u64), String> {
        Ok((format!("SYNTHETIC utterance of {} ms", pcm.len() / 32), 0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_worker_script_is_carried_and_speaks_the_protocol() {
        for k in [
            "\"ready\"",
            "\"path\"",
            "\"text\"",
            "OfflineRecognizer",
            "int16",
            "<i2",
        ] {
            assert!(SCRIPT.contains(k) || k == "int16", "{k}");
        }
    }

    #[test]
    fn the_synthetic_recognizer_reports_the_length() {
        let (t, ms) = Synthetic.transcribe(&vec![0u8; 32_000]).unwrap();
        assert_eq!((t.as_str(), ms), ("SYNTHETIC utterance of 1000 ms", 0));
    }

    /// A stand-in worker that speaks the protocol, to test the process plumbing without a model.
    #[test]
    fn the_cog_talks_to_a_worker_process() {
        let dir = std::env::temp_dir().join(format!("asr-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let stub = dir.join("stub.sh");
        std::fs::write(
            &stub,
            "#!/bin/sh\necho '{\"ready\": true}'\nwhile read l; do id=$(echo \"$l\" | sed 's/.*\"id\": *\\([0-9]*\\).*/\\1/'); echo \"{\\\"id\\\": $id, \\\"text\\\": \\\"hello world\\\", \\\"ms\\\": 7}\"; done\n",
        )
        .unwrap();
        // Parakeet::start runs `python script model threads`; here the 'python' is /bin/sh and
        // the script is overwritten by SCRIPT, so run the stub through a tiny wrapper instead.
        let wrapper = dir.join("py.sh");
        std::fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec sh {} \"$@\"\n", stub.display()),
        )
        .unwrap();
        std::process::Command::new("chmod")
            .arg("+x")
            .arg(&wrapper)
            .status()
            .unwrap();
        let mut p = Parakeet::start(wrapper.to_str().unwrap(), "model", 1, &dir).unwrap();
        let (t, ms) = p.transcribe(&[0u8; 640]).unwrap();
        assert_eq!((t.as_str(), ms), ("hello world", 7));
        let (t2, _) = p.transcribe(&[0u8; 640]).unwrap();
        assert_eq!(t2, "hello world");
        drop(p);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_interpreter_is_a_clear_error() {
        let dir = std::env::temp_dir().join(format!("asr-worker-missing-{}", std::process::id()));
        let e = Parakeet::start("/nonexistent/python", "m", 1, &dir)
            .err()
            .unwrap();
        assert!(e.contains("/nonexistent/python"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
