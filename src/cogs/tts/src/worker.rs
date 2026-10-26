//! The synthesizer behind the cog: a Kokoro or Piper (sherpa-onnx) worker process, like the asr
//! cog's. The cog sends the text and a file path as a JSON line; the worker writes mono float32
//! samples to the file and answers with their rate and count. The worker is a Python script
//! carried in this binary (`worker/tts_worker.py`) and run with `$TTS_PYTHON`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::time::Duration;

use serde_json::{json, Value};

pub const SCRIPT: &str = include_str!("../worker/tts_worker.py");

/// Mono samples and their rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Speech {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub ms: u64,
}

pub trait Synth {
    fn say(&mut self, text: &str) -> Result<Speech, String>;
    fn label(&self) -> String;
}

pub struct Sherpa {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Value>,
    dir: PathBuf,
    engine: String,
    next: u64,
}

impl Sherpa {
    pub fn start(
        python: &str,
        engine: &str,
        models_root: &str,
        threads: u32,
        speaker: u32,
        speed: f64,
        data_dir: &Path,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(data_dir).map_err(|e| format!("data dir: {e}"))?;
        let script = data_dir.join("tts_worker.py");
        std::fs::write(&script, SCRIPT).map_err(|e| format!("write worker: {e}"))?;
        let mut child = Command::new(python)
            .arg(&script)
            .args([
                engine,
                models_root,
                &threads.to_string(),
                &speaker.to_string(),
                &speed.to_string(),
            ])
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
            engine: engine.into(),
            next: 0,
        };
        match me.lines.recv_timeout(Duration::from_secs(120)) {
            Ok(v) if v["ready"] == true => Ok(me),
            Ok(v) => Err(format!("worker said {v}")),
            Err(_) => {
                let _ = me.child.kill();
                Err("the synthesizer worker did not become ready (model path, python or sherpa_onnx missing?)".into())
            }
        }
    }
}

impl Synth for Sherpa {
    fn label(&self) -> String {
        format!("sherpa-onnx {}", self.engine)
    }

    fn say(&mut self, text: &str) -> Result<Speech, String> {
        self.next += 1;
        let id = self.next;
        let out = self.dir.join(format!("speech{}.f32", id % 4));
        let line = json!({"id": id, "text": text, "out": out}).to_string() + "\n";
        self.stdin
            .write_all(line.as_bytes())
            .and_then(|_| self.stdin.flush())
            .map_err(|e| format!("worker write: {e}"))?;
        loop {
            match self.lines.recv_timeout(Duration::from_secs(60)) {
                Ok(v) if v["id"].as_u64() == Some(id) => {
                    if let Some(e) = v["error"].as_str() {
                        return Err(e.to_string());
                    }
                    let raw = std::fs::read(&out).map_err(|e| format!("read speech: {e}"))?;
                    let samples: Vec<f32> = raw
                        .chunks_exact(4)
                        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                        .collect();
                    return Ok(Speech {
                        samples,
                        rate: v["rate"].as_u64().unwrap_or(24_000) as u32,
                        ms: v["ms"].as_u64().unwrap_or(0),
                    });
                }
                Ok(_) => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err("the synthesizer did not answer in time".into())
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("the synthesizer worker died".into())
                }
            }
        }
    }
}

impl Drop for Sherpa {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// `--simulate`: a tone of 80 ms per word at 24 kHz, no model.
pub struct Tone;

impl Synth for Tone {
    fn label(&self) -> String {
        "synthetic".into()
    }

    fn say(&mut self, text: &str) -> Result<Speech, String> {
        let words = text.split_whitespace().count().max(1);
        let n = 24_000 * 80 / 1000 * words;
        let samples = (0..n)
            .map(|i| ((i as f64 / 24_000.0) * 330.0 * std::f64::consts::TAU).sin() as f32 * 0.3)
            .collect();
        Ok(Speech {
            samples,
            rate: 24_000,
            ms: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_is_carried_and_speaks_the_protocol() {
        for k in [
            "\"ready\"",
            "\"out\"",
            "tofile",
            "OfflineTts",
            "kokoro",
            "piper",
        ] {
            assert!(SCRIPT.contains(k), "{k}");
        }
    }

    #[test]
    fn the_tone_is_80_ms_per_word() {
        let s = Tone.say("one two three").unwrap();
        assert_eq!((s.rate, s.samples.len()), (24_000, 24_000 * 80 / 1000 * 3));
        assert_eq!(
            Tone.say("").unwrap().samples.len(),
            24_000 * 80 / 1000,
            "at least one word"
        );
    }

    #[test]
    fn the_cog_talks_to_a_worker_process() {
        let dir = std::env::temp_dir().join(format!("tts-worker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A stand-in that writes 4 samples (1.0, 0.5, -0.5, 0.0) and answers.
        let stub = dir.join("stub.sh");
        std::fs::write(
            &stub,
            "#!/bin/sh\necho '{\"ready\": true, \"rate\": 22050}'\nwhile read l; do\n id=$(echo \"$l\" | sed 's/.*\"id\": *\\([0-9]*\\).*/\\1/')\n out=$(echo \"$l\" | sed 's/.*\"out\": *\"\\([^\"]*\\)\".*/\\1/')\n printf '\\000\\000\\200\\077\\000\\000\\000\\077\\000\\000\\000\\277\\000\\000\\000\\000' > \"$out\"\n echo \"{\\\"id\\\": $id, \\\"rate\\\": 22050, \\\"samples\\\": 4, \\\"ms\\\": 9}\"\ndone\n",
        )
        .unwrap();
        let wrapper = dir.join("py.sh");
        std::fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec sh {} \"$@\"\n", stub.display()),
        )
        .unwrap();
        Command::new("chmod")
            .arg("+x")
            .arg(&wrapper)
            .status()
            .unwrap();
        let mut s = Sherpa::start(
            wrapper.to_str().unwrap(),
            "piper",
            "models",
            1,
            0,
            1.0,
            &dir,
        )
        .unwrap();
        let sp = s.say("hello").unwrap();
        assert_eq!((sp.rate, sp.ms), (22_050, 9));
        assert_eq!(sp.samples, [1.0, 0.5, -0.5, 0.0]);
        drop(s);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_interpreter_is_a_clear_error() {
        let dir = std::env::temp_dir().join(format!("tts-worker-missing-{}", std::process::id()));
        let e = Sherpa::start("/nonexistent/python", "kokoro", "m", 1, 0, 1.0, &dir)
            .err()
            .unwrap();
        assert!(e.contains("/nonexistent/python"), "{e}");
        std::fs::remove_dir_all(&dir).ok();
    }
}
