//! cog-ld6004-radar: HLK-LD6004 60 GHz 3D presence radar reader.
//!
//! Usage:
//!   cog-ld6004-radar [--once] [--interval SECS] [--device /dev/ttyUSB0]
//!                    [--baud 115200] [--simulate] [--command keep|query-firmware]
//!                    [--api-bind 127.0.0.1:8055] [--source-id ld6004-radar]
//!   (feature `spatial-evidence` adds `--spatial-out export|FILE`, `--radar-pose`,
//!    `--spatial-region`, `--spatial-uncertainty-m`)
//!
//! The cog owns the UART: it resyncs, checks the TinyFrame checksum, and prints
//! decoded values. `--command keep` (the default) sends nothing. `--simulate`
//! feeds documented and synthetic frames through the same parser and labels the
//! line simulated. `--once` with no device prints one `no_source` line and exits 0.
//!
//! Word length and stop bit are an assumption. The datasheet states 115200 and
//! no parity, and does not state 8 data bits or 1 stop bit. This process opens 8N1.

mod decode;
mod export;
mod frame;
mod guide;
mod senml;
#[cfg(feature = "spatial-evidence")]
mod spatial;
mod window;

use std::io::{ErrorKind, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use decode::Position;
use export::{Ring, Shared};
use frame::Framer;
use window::{LinkState, Report, Window, WindowContext};

const DEFAULT_DEVICE: &str = "/dev/ttyUSB0";
const DEFAULT_BAUD: u32 = 115_200;
const DEFAULT_INTERVAL: u64 = 1;
const DEFAULT_BIND: &str = "127.0.0.1:8055";
const DEFAULT_SOURCE: &str = "ld6004-radar";
const READ_TIMEOUT: Duration = Duration::from_millis(50);
const TAG: &str = "ld6004-radar";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    Keep,
    QueryFirmware,
}

impl Command {
    fn parse(v: &str) -> Result<Command, String> {
        match v {
            "keep" => Ok(Command::Keep),
            "query-firmware" => Ok(Command::QueryFirmware),
            _ => Err(format!(
                "--command must be keep or query-firmware, got {v:?}"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Command::Keep => "none",
            Command::QueryFirmware => "query-firmware",
        }
    }
}

#[derive(Debug, PartialEq)]
struct Config {
    once: bool,
    interval: u64,
    device: String,
    baud: u32,
    simulate: bool,
    command: Command,
    api_bind: String,
    source_id: String,
    #[cfg(feature = "spatial-evidence")]
    spatial: Option<spatial::Settings>,
}

fn parse_args(args: &[String]) -> Result<Config, String> {
    let mut cfg = Config {
        once: false,
        interval: DEFAULT_INTERVAL,
        device: DEFAULT_DEVICE.into(),
        baud: DEFAULT_BAUD,
        simulate: false,
        command: Command::Keep,
        api_bind: DEFAULT_BIND.into(),
        source_id: DEFAULT_SOURCE.into(),
        #[cfg(feature = "spatial-evidence")]
        spatial: None,
    };
    #[cfg(feature = "spatial-evidence")]
    let mut sp = spatial::Args::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{a} needs a value"));
        match a.as_str() {
            "--once" => cfg.once = true,
            "--simulate" => cfg.simulate = true,
            "--interval" => {
                let v = value()?;
                cfg.interval = v
                    .parse()
                    .ok()
                    .filter(|n| (1..=3600).contains(n))
                    .ok_or(format!("--interval must be an integer 1..=3600, got {v:?}"))?;
            }
            "--baud" => {
                let v = value()?;
                cfg.baud = v
                    .parse()
                    .ok()
                    .filter(|n| (9600..=921_600).contains(n))
                    .ok_or(format!(
                        "--baud must be an integer 9600..=921600, got {v:?}"
                    ))?;
            }
            "--device" => {
                let v = value()?;
                validate_device(&v)?;
                cfg.device = v;
            }
            "--command" => cfg.command = Command::parse(&value()?)?,
            "--api-bind" => cfg.api_bind = parse_bind(&value()?)?,
            "--source-id" => cfg.source_id = normalize_id(&value()?)?,
            #[cfg(feature = "spatial-evidence")]
            f if spatial::Args::FLAGS.contains(&f) => sp.set(f, &value()?)?,
            other => return Err(format!("unknown argument {other:?} (see --help)")),
        }
    }
    if cfg.simulate && cfg.command == Command::QueryFirmware {
        return Err(
            "--command query-firmware writes the 0xFFFF query, so it cannot be combined with --simulate"
                .into(),
        );
    }
    #[cfg(feature = "spatial-evidence")]
    {
        cfg.spatial = sp.finish(cfg.source_id.clone())?;
    }
    Ok(cfg)
}

fn normalize_id(s: &str) -> Result<String, String> {
    let id = s.trim().trim_end_matches('/');
    if valid_id(id) {
        Ok(id.to_string())
    } else {
        Err(format!(
            "--source-id must match [A-Za-z0-9._:/-@]{{1,128}}, got {s:?}"
        ))
    }
}

fn valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:/-@".contains(&b))
}

fn validate_device(path: &str) -> Result<(), String> {
    let ok = path.starts_with("/dev/")
        && path.len() <= 128
        && !path.split('/').any(|c| c == "..")
        && path.bytes().all(|b| b.is_ascii_graphic());
    if ok {
        Ok(())
    } else {
        Err(format!("--device must be a path under /dev/, got {path:?}"))
    }
}

fn parse_bind(s: &str) -> Result<String, String> {
    let (host, port) = s
        .rsplit_once(':')
        .ok_or_else(|| format!("--api-bind must be 127.0.0.1:PORT, got {s:?}"))?;
    if host != "127.0.0.1" {
        return Err(format!("--api-bind accepts only 127.0.0.1, got {s:?}"));
    }
    let port: u16 = port
        .parse()
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| format!("--api-bind port must be 1..=65535, got {s:?}"))?;
    Ok(format!("127.0.0.1:{port}"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn open_port(cfg: &Config) -> Result<Box<dyn serialport::SerialPort>, String> {
    // 8 data bits and 1 stop bit are an assumption. The LD6004 datasheet states
    // the baud (115200) and "no parity" and does not state the word length.
    serialport::new(&cfg.device, cfg.baud)
        .data_bits(serialport::DataBits::Eight)
        .stop_bits(serialport::StopBits::One)
        .parity(serialport::Parity::None)
        .flow_control(serialport::FlowControl::None)
        .timeout(READ_TIMEOUT)
        .open()
        .map_err(|e| e.to_string())
}

struct SpatialState {
    #[cfg(feature = "spatial-evidence")]
    emitter: Option<spatial::Emitter>,
    #[cfg(feature = "spatial-evidence")]
    status: Option<&'static str>,
    #[cfg(feature = "spatial-evidence")]
    refusal: Option<String>,
    #[cfg(feature = "spatial-evidence")]
    error: Option<String>,
}

impl SpatialState {
    fn from_config(cfg: &Config) -> SpatialState {
        #[cfg(feature = "spatial-evidence")]
        {
            match cfg.spatial.clone() {
                None => SpatialState {
                    emitter: None,
                    status: None,
                    refusal: None,
                    error: None,
                },
                Some(s) => {
                    let status = s.refusal_status();
                    match spatial::Emitter::new(s, cfg.simulate) {
                        Ok(e) => SpatialState {
                            emitter: Some(e),
                            status: Some("emitting"),
                            refusal: None,
                            error: None,
                        },
                        Err(why) => {
                            eprintln!("{TAG}: not emitting spatial.evidence.v1: {why}");
                            SpatialState {
                                emitter: None,
                                status,
                                refusal: Some(why),
                                error: None,
                            }
                        }
                    }
                }
            }
        }
        #[cfg(not(feature = "spatial-evidence"))]
        {
            let _ = cfg;
            SpatialState {}
        }
    }

    fn note(&mut self, report: &mut Report) {
        #[cfg(feature = "spatial-evidence")]
        {
            report.spatial = self.status;
            if let Some(why) = &self.refusal {
                report
                    .reasons
                    .push(format!("spatial_evidence_refused: {why}"));
            }
            if let Some(e) = self.error.take() {
                report.reasons.push(format!("spatial_evidence_error: {e}"));
                report.spatial = Some("write_error");
            }
        }
        #[cfg(not(feature = "spatial-evidence"))]
        {
            let _ = (self, report);
        }
    }

    fn emit(&mut self, t_ms: u64, pts: &[Position], ring: &Shared) {
        #[cfg(feature = "spatial-evidence")]
        {
            let export_sink = self
                .emitter
                .as_ref()
                .is_some_and(|e| e.file_path().is_none());
            let path = self
                .emitter
                .as_ref()
                .and_then(|e| e.file_path().map(str::to_string));
            let Some(emitter) = self.emitter.as_mut() else {
                return;
            };
            let lines = emitter.tracks(t_ms, pts);
            if lines.is_empty() {
                return;
            }
            if export_sink {
                if let Ok(mut guard) = ring.lock() {
                    for line in lines {
                        guard.push_evidence(t_ms, line);
                    }
                }
            } else if let Some(path) = path {
                if let Err(err) = spatial::append(&path, &lines) {
                    self.error = Some(format!("{path}: {err}"));
                }
            }
        }
        #[cfg(not(feature = "spatial-evidence"))]
        {
            let _ = (self, t_ms, pts, ring);
        }
    }
}

fn feed(
    framer: &mut Framer,
    win: &mut Window,
    link: &mut LinkState,
    spatial: &mut SpatialState,
    ring: &Shared,
    bytes: &[u8],
) {
    let frames = framer.feed(bytes);
    let t_ms = now_ms();
    for f in &frames {
        let pts = win.ingest(f, link);
        spatial.emit(t_ms, &pts, ring);
    }
}

#[allow(clippy::too_many_arguments)] // One UART window shares the framer, the report, and the sinks.
fn read_window(
    port: &mut dyn serialport::SerialPort,
    framer: &mut Framer,
    win: &mut Window,
    link: &mut LinkState,
    spatial: &mut SpatialState,
    ring: &Shared,
    deadline: Instant,
    bytes_read: &mut u64,
) -> Option<String> {
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match port.read(&mut buf) {
            Ok(0) => {}
            Ok(n) => {
                *bytes_read += n as u64;
                feed(framer, win, link, spatial, ring, &buf[..n]);
            }
            Err(e) if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::Interrupted) => {}
            Err(e) => return Some(e.to_string()),
        }
    }
    None
}

fn publish(ring: &Shared, report: &serde_json::Value, senml: Option<String>, source: &str) {
    if let Ok(mut guard) = ring.lock() {
        guard.source = source.into();
        guard.latest_report = Some(report.clone());
        guard.senml = senml;
    }
}

fn emit_stdout(report_line: &str, senml: &Option<String>) {
    let mut out = std::io::stdout().lock();
    let senml_write = match senml {
        Some(line) => writeln!(out, "{report_line}\n{line}"),
        None => writeln!(out, "{report_line}"),
    };
    if senml_write.and_then(|_| out.flush()).is_err() {
        std::process::exit(0);
    }
}

fn finish_window(
    win: Window,
    ctx: WindowContext,
    spatial: &mut SpatialState,
    ring: &Shared,
    source: &str,
) {
    let (mut report, senml) = win.finish(ctx);
    spatial.note(&mut report);
    let value = match serde_json::to_value(&report) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{TAG}: serialize failed: {e}");
            return;
        }
    };
    let line = value.to_string();
    publish(ring, &value, senml.clone(), source);
    emit_stdout(&line, &senml);
}

fn run(cfg: &Config) {
    let window_len = Duration::from_secs(cfg.interval);
    let ring: Shared = Arc::new(Mutex::new(Ring {
        source: "none".into(),
        ..Ring::default()
    }));
    match export::serve(&cfg.api_bind, ring.clone()) {
        Ok(addr) => eprintln!("{TAG}: export http://{addr}/"),
        Err(e) => eprintln!("{TAG}: export disabled: {e}"),
    }
    let mut spatial = SpatialState::from_config(cfg);
    let mut link = LinkState::default();
    let mut framer = Framer::new();
    let mut port: Option<Box<dyn serialport::SerialPort>> = None;
    let mut queried = false;
    let mut command_error: Option<String> = None;

    loop {
        let start_ms = now_ms();
        let deadline = Instant::now() + window_len;
        let mut win = Window::default();
        let mut bytes_read = 0u64;
        let mut device_error = None;
        let source_name;

        if !cfg.simulate && port.is_none() {
            match open_port(cfg) {
                Ok(mut p) => {
                    eprintln!("{TAG}: opened {} at {} baud", cfg.device, cfg.baud);
                    if cfg.command == Command::QueryFirmware && !queried {
                        queried = true;
                        if let Err(e) = p.write_all(frame::VENDOR_FW_QUERY) {
                            command_error = Some(e.to_string());
                        }
                    }
                    framer = Framer::new();
                    port = Some(p);
                }
                Err(e) => device_error = Some(e),
            }
        }
        let before = framer.stats();
        if cfg.simulate {
            let burst = decode::simulated_burst();
            bytes_read = burst.len() as u64;
            feed(
                &mut framer,
                &mut win,
                &mut link,
                &mut spatial,
                &ring,
                &burst,
            );
            source_name = "simulate";
            eprintln!("{TAG}: simulated burst, not the sensor");
        } else {
            if let Some(p) = port.as_mut() {
                if let Some(e) = read_window(
                    p.as_mut(),
                    &mut framer,
                    &mut win,
                    &mut link,
                    &mut spatial,
                    &ring,
                    deadline,
                    &mut bytes_read,
                ) {
                    eprintln!("{TAG}: read error on {}: {e}", cfg.device);
                    device_error = Some(e);
                    port = None;
                    queried = false;
                }
            }
            source_name = "uart";
        }

        let end_ms = now_ms();
        finish_window(
            win,
            WindowContext {
                start_ms,
                end_ms,
                device: cfg.device.clone(),
                stats: framer.stats().since(&before),
                bytes_read,
                device_error,
                simulated: cfg.simulate,
                link: link.clone(),
                writes: cfg.command.as_str(),
                source_id: cfg.source_id.clone(),
                command_error: command_error.take(),
            },
            &mut spatial,
            &ring,
            source_name,
        );
        if cfg.once {
            return;
        }
        if cfg.simulate {
            std::thread::sleep(window_len);
        } else if port.is_none() {
            std::thread::sleep(deadline.saturating_duration_since(Instant::now()));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    cog_sensor_sources::handle_help(
        &args,
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION"),
        include_str!("../cog.toml"),
    );
    match parse_args(&args) {
        Ok(cfg) => run(&cfg),
        Err(e) => {
            eprintln!("{TAG}: {e}");
            std::process::exit(2);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn defaults_send_nothing_and_stay_on_loopback() {
        let c = parse_args(&[]).unwrap();
        assert_eq!(c.command, Command::Keep);
        assert_eq!(c.command.as_str(), "none");
        assert!(!c.simulate);
        assert_eq!(c.api_bind, "127.0.0.1:8055");
        assert_eq!(c.source_id, "ld6004-radar");
        assert_eq!(c.baud, 115_200);
    }

    #[test]
    fn every_console_command_parses() {
        let toml = include_str!("../cog.toml");
        let line = toml
            .lines()
            .find(|l| l.starts_with("allowed_commands"))
            .unwrap();
        let cmds: Vec<&str> = line.split('"').skip(1).step_by(2).collect();
        assert!(cmds.len() >= 4);
        for c in cmds.iter().filter(|c| **c != "--help") {
            parse_args(&args(c)).unwrap_or_else(|e| panic!("{c}: {e}"));
        }
    }

    #[test]
    fn unknown_bind_and_command_are_refused() {
        assert!(parse_args(&args("--api-bind 0.0.0.0:8055")).is_err());
        assert!(parse_args(&args("--command enable")).is_err());
        assert!(parse_args(&args("--device /etc/passwd")).is_err());
        assert!(parse_args(&args("--baud 100")).is_err());
        assert!(parse_args(&args("--simulate --command query-firmware")).is_err());
        let id = parse_args(&args("--source-id ld6004-radar/")).unwrap();
        assert_eq!(id.source_id, "ld6004-radar");
    }

    #[cfg(not(feature = "spatial-evidence"))]
    #[test]
    fn spatial_flags_are_unknown_without_the_feature() {
        let err = parse_args(&args("--once --spatial-out export")).unwrap_err();
        assert!(err.contains("unknown"), "{err}");
    }

    #[cfg(feature = "spatial-evidence")]
    #[test]
    fn spatial_export_parses_and_stays_off_without_the_flag() {
        let c = parse_args(&[]).unwrap();
        assert!(c.spatial.is_none());
        let c = parse_args(&args(
            "--spatial-out export --radar-pose 0,0,0,90,1 --spatial-region region/urth/meso/lab",
        ))
        .unwrap();
        let s = c.spatial.unwrap();
        assert!(s.refusal().is_none());
        assert_eq!(s.uncertainty_m, 0.3);
        assert_eq!(s.source_id, "ld6004-radar");
        assert!(parse_args(&args("--spatial-out /dev/null")).is_err());
    }
}
