//! Serial session for one HLK-LD6004. Opens 115200 8N1, sends the read
//! queries, and folds reports into a shared snapshot. Writes happen only
//! when the UI sends a command.

use crate::proto::{self, Parser, READ_CMDS, Snapshot, TY_FW, TY_TARGETS};
use std::collections::VecDeque;
use std::io::Write;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const DEFAULT_PORT: &str = "/dev/cu.usbserial-214110";

#[derive(Clone, Debug)]
pub enum Cmd {
    QueryAll,
    Control(u32),
    Frame { ty: u16, data: Vec<u8> },
    Reopen(String),
    Stop,
}

#[derive(Clone, Debug)]
pub struct Shared {
    pub snap: Snapshot,
    pub status: String,
    pub port: String,
    pub open: bool,
    pub bytes: u64,
    pub frames: u64,
    pub resyncs: u64,
    pub pairs_per_s: f32,
}

impl Shared {
    fn new(port: &str) -> Self {
        Self {
            snap: Snapshot::default(),
            status: "opening".into(),
            port: port.into(),
            open: false,
            bytes: 0,
            frames: 0,
            resyncs: 0,
            pairs_per_s: 0.0,
        }
    }
}

pub struct Handle {
    pub tx: Sender<Cmd>,
    pub shared: Arc<Mutex<Shared>>,
    _join: std::thread::JoinHandle<()>,
}

pub fn start(port: &str) -> std::io::Result<Handle> {
    let (tx, rx) = mpsc::channel();
    let shared = Arc::new(Mutex::new(Shared::new(port)));
    let path = port.to_string();
    let shared_thread = Arc::clone(&shared);
    let join = std::thread::Builder::new()
        .name("ld6004".into())
        .spawn(move || worker(path, rx, shared_thread))?;
    Ok(Handle {
        tx,
        shared,
        _join: join,
    })
}

enum End {
    Stop,
    Reopen(String),
    Lost(String),
}

fn worker(mut path: String, rx: Receiver<Cmd>, shared: Arc<Mutex<Shared>>) {
    loop {
        set_status(&shared, &path, false, "opening");
        match open_port(&path) {
            Ok(port) => match session(port, &rx, &shared) {
                End::Stop => {
                    set_status(&shared, &path, false, "closed");
                    return;
                }
                End::Reopen(next) => path = next,
                End::Lost(err) => {
                    set_status(&shared, &path, false, &err);
                    match wait_retry(&rx) {
                        Retry::Quit => return,
                        Retry::Switch(next) => path = next,
                        Retry::Continue => {}
                    }
                }
            },
            Err(err) => {
                set_status(&shared, &path, false, &format!("open failed: {err}"));
                match wait_retry(&rx) {
                    Retry::Quit => return,
                    Retry::Switch(next) => path = next,
                    Retry::Continue => {}
                }
            }
        }
    }
}

enum Retry {
    Continue,
    Switch(String),
    Quit,
}

fn wait_retry(rx: &Receiver<Cmd>) -> Retry {
    match rx.recv_timeout(Duration::from_millis(500)) {
        Ok(Cmd::Stop) => Retry::Quit,
        Ok(Cmd::Reopen(path)) => Retry::Switch(path),
        _ => Retry::Continue,
    }
}

fn session(
    mut port: Box<dyn serialport::SerialPort>,
    rx: &Receiver<Cmd>,
    shared: &Arc<Mutex<Shared>>,
) -> End {
    let mut parser = Parser::default();
    let mut id = 1u16;
    let mut out = VecDeque::new();
    enqueue_reads(&mut out, &mut id);
    let mut next_tx = Instant::now();
    let again = Instant::now() + Duration::from_millis(1200);
    let mut retried = false;
    let mut pairs: VecDeque<Instant> = VecDeque::new();
    let mut buf = [0u8; 512];
    let mut bytes = 0u64;
    let mut frames = 0u64;
    set_status(shared, "", true, "streaming");

    loop {
        match take_cmd(rx) {
            Some(Cmd::Stop) => return End::Stop,
            Some(Cmd::Reopen(path)) => return End::Reopen(path),
            Some(Cmd::QueryAll) => enqueue_reads(&mut out, &mut id),
            Some(Cmd::Control(cmd)) => {
                out.push_back(proto::control(id, cmd));
                id = id.wrapping_add(1);
            }
            Some(Cmd::Frame { ty, data }) => {
                out.push_back(proto::encode(id, ty, &data));
                id = id.wrapping_add(1);
            }
            None => {}
        }

        if !retried && Instant::now() >= again {
            let have_fw = shared
                .lock()
                .map(|g| g.snap.firmware.is_some())
                .unwrap_or(false);
            if !have_fw {
                enqueue_reads(&mut out, &mut id);
            }
            retried = true;
        }

        if Instant::now() >= next_tx
            && let Some(frame) = out.pop_front()
        {
            if let Err(err) = port.write_all(&frame).and_then(|_| port.flush()) {
                return End::Lost(format!("write failed: {err}"));
            }
            next_tx = Instant::now() + Duration::from_millis(35);
        }

        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                bytes += n as u64;
                parser.push(&buf[..n]);
                while let Some(frame) = parser.next_frame() {
                    frames += 1;
                    if frame.ty == TY_TARGETS && !frame.data.is_empty() {
                        pairs.push_back(Instant::now());
                    }
                    if let Ok(mut guard) = shared.lock() {
                        proto::apply(&mut guard.snap, &frame);
                    }
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::TimedOut => {}
            Err(err) => return End::Lost(format!("read failed: {err}")),
        }

        let now = Instant::now();
        while pairs
            .front()
            .is_some_and(|t| now.duration_since(*t) > Duration::from_secs(1))
        {
            pairs.pop_front();
        }
        if let Ok(mut guard) = shared.lock() {
            guard.bytes = bytes;
            guard.frames = frames;
            guard.resyncs = parser.resyncs;
            guard.pairs_per_s = pairs.len() as f32;
            if guard.open {
                guard.status = "streaming".into();
            }
        }
    }
}

fn take_cmd(rx: &Receiver<Cmd>) -> Option<Cmd> {
    match rx.try_recv() {
        Ok(cmd) => Some(cmd),
        Err(TryRecvError::Empty) => None,
        Err(TryRecvError::Disconnected) => Some(Cmd::Stop),
    }
}

fn enqueue_reads(out: &mut VecDeque<Vec<u8>>, id: &mut u16) {
    for cmd in READ_CMDS {
        out.push_back(proto::control(*id, *cmd));
        *id = id.wrapping_add(1);
    }
    out.push_back(proto::encode(*id, TY_FW, &[]));
    *id = id.wrapping_add(1);
}

fn open_port(path: &str) -> serialport::Result<Box<dyn serialport::SerialPort>> {
    serialport::new(path, 115_200)
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .timeout(Duration::from_millis(40))
        .open()
}

fn set_status(shared: &Arc<Mutex<Shared>>, port: &str, open: bool, status: &str) {
    if let Ok(mut guard) = shared.lock() {
        if !port.is_empty() {
            guard.port = port.into();
        }
        guard.open = open;
        guard.status = status.into();
    }
}

/// Open, query, and print once. Used by `--probe`. Does not change saved config.
pub fn probe(port: &str) -> Result<String, String> {
    let handle = start(port).map_err(|err| format!("thread: {err}"))?;
    let deadline = Instant::now() + Duration::from_secs(4);
    let mut last = String::new();
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
        let guard = handle.shared.lock().map_err(|_| "lock".to_string())?;
        last = render(&guard);
        if guard.snap.required_ok() && guard.frames > 4 {
            drop(guard);
            let _ = handle.tx.send(Cmd::Stop);
            return Ok(last);
        }
    }
    let _ = handle.tx.send(Cmd::Stop);
    Err(last)
}

pub fn render(shared: &Shared) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "{}  {}  frames {}  bytes {}  resyncs {}  {:.0} target frames/s",
        shared.port, shared.status, shared.frames, shared.bytes, shared.resyncs, shared.pairs_per_s
    ));
    for row in proto::rows(&shared.snap) {
        let mark = match row.verdict {
            proto::Verdict::Pass => "PASS",
            proto::Verdict::Wait => "WAIT",
            proto::Verdict::Held => "HELD",
        };
        lines.push(format!("{mark}  {:<36} {}", row.name, row.detail));
    }
    if shared.snap.required_ok() {
        lines.push("core queries: all answered".into());
    } else {
        lines.push("core queries: still missing a reply".into());
    }
    lines.join("\n")
}
