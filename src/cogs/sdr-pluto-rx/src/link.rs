//! The cogbus side: the connection thread that serves `svc.sdr` and keeps the link alive, and
//! the output the acquisition thread writes IQ records and readings to.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::acquire::{Envelope, Out, Shared};
use crate::bus::Bus;
use crate::iq;
use crate::ring::RingWriter;
use crate::svc;

pub const IQ_KIND: &str = "iq.cs16";
pub const IQ_SCHEMA: &str = "iq.cs16.v1";
pub const STATUS_KIND: &str = "sdr.status";
pub const STATUS_SCHEMA: &str = "telemetry.v1";
pub const SVC_KIND: &str = "svc.sdr";
pub const SVC_SCHEMA: &str = "sdr.v1";

/// The current connection, shared between the connection thread and the acquisition thread.
#[derive(Default)]
pub struct Link {
    bus: Mutex<Option<Arc<Bus>>>,
    gen: AtomicU64,
    connects: AtomicU64,
    /// Last ring seq the acquisition thread wrote; a reopened ring continues after it.
    last_seq: AtomicU64,
    /// A ring the connection thread opened for the connection with this generation.
    ring: Mutex<Option<(u64, RingWriter)>>,
}

impl Link {
    fn set(&self, b: Option<Arc<Bus>>) -> u64 {
        if b.is_some() {
            self.connects.fetch_add(1, Ordering::Relaxed);
        }
        *self.ring.lock().unwrap_or_else(|p| p.into_inner()) = None;
        *self.bus.lock().unwrap_or_else(|p| p.into_inner()) = b;
        self.gen.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn get(&self) -> (u64, Option<Arc<Bus>>) {
        let g = self.bus.lock().unwrap_or_else(|p| p.into_inner());
        (self.gen.load(Ordering::Acquire), g.clone())
    }
}

pub struct LinkOpts {
    pub path: String,
    pub token: String,
    pub service: String,
    /// Largest ring record (a base64 message of one record's samples).
    pub max_record: u64,
    pub ring_bytes: u64,
}

/// Largest message for `samples` complex samples: base64 of the bytes plus the fields.
pub fn max_record_for(samples: usize) -> u64 {
    (samples * 4).div_ceil(3) as u64 * 4 + 1024
}

/// Connect, serve `svc.sdr/<service>`, route calls to the acquisition thread, reconnect with
/// backoff when the broker goes away.
pub fn spawn(o: LinkOpts, link: Arc<Link>, cmds: Sender<Envelope>, shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("link".into())
        .spawn(move || {
            let mut backoff = 1u64;
            let mut last_log = Instant::now() - Duration::from_secs(60);
            while !shared.stop.load(Ordering::Acquire) {
                match connect_and_serve(&o) {
                    Ok((bus, calls)) => {
                        backoff = 1;
                        let gen = link.set(Some(bus.clone()));
                        eprintln!(
                            "[cog-sdr-pluto-rx] cogbus: serving {SVC_KIND}/{} on node {}",
                            o.service, bus.node_id
                        );
                        let mut ring_due = Instant::now();
                        let mut ring_open = false;
                        while !bus.is_closed() && !shared.stop.load(Ordering::Acquire) {
                            // The ring is opened here, never on the acquisition thread: the
                            // broker's answer must not hold up the radio's reads.
                            if !ring_open && Instant::now() >= ring_due {
                                let seq = link.last_seq.load(Ordering::Relaxed);
                                match bus.stream_writer(IQ_KIND, &o.service, IQ_SCHEMA, o.max_record, Some(o.ring_bytes), Some(seq)) {
                                    Ok(w) => {
                                        *link.ring.lock().unwrap_or_else(|p| p.into_inner()) = Some((gen, w));
                                        ring_open = true;
                                    }
                                    Err(e) => {
                                        eprintln!("[cog-sdr-pluto-rx] cogbus: cannot open the IQ ring: {e}");
                                        ring_due = Instant::now() + Duration::from_secs(2);
                                    }
                                }
                            }
                            match calls.recv_timeout(Duration::from_millis(200)) {
                                Ok(f) => handle_call(&bus, &cmds, &f),
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                                Err(_) => break,
                            }
                        }
                        link.set(None);
                        eprintln!("[cog-sdr-pluto-rx] cogbus: connection ended");
                    }
                    Err(e) => {
                        if last_log.elapsed() > Duration::from_secs(30) {
                            eprintln!("[cog-sdr-pluto-rx] cogbus: {e}; retrying");
                            last_log = Instant::now();
                        }
                    }
                }
                for _ in 0..backoff * 10 {
                    if shared.stop.load(Ordering::Acquire) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                backoff = (backoff * 2).min(30);
            }
        })
        .ok();
}

fn connect_and_serve(o: &LinkOpts) -> Result<(Arc<Bus>, crate::bus::Calls), String> {
    let (bus, calls) = Bus::connect(&o.path, &o.token)?;
    bus.serve(SVC_KIND, &o.service, SVC_SCHEMA)?;
    Ok((bus, calls))
}

fn handle_call(bus: &Bus, cmds: &Sender<Envelope>, f: &Value) {
    let Some(call) = f["call"].as_u64() else {
        return;
    };
    let from = f["from"].as_str().unwrap_or("?");
    let wait = Duration::from_millis(
        f["timeout_ms"]
            .as_u64()
            .unwrap_or(30_000)
            .clamp(100, 600_000),
    );
    let r = svc::parse(from, &f["body"]).and_then(|cmd| {
        let (tx, rx) = channel();
        cmds.send((cmd, tx))
            .map_err(|_| "the tuner is shutting down".to_string())?;
        rx.recv_timeout(wait)
            .map_err(|_| "the tuner did not answer in time".to_string())?
    });
    if let Err(e) = bus.reply(call, r) {
        eprintln!("[cog-sdr-pluto-rx] cogbus: reply failed: {e}");
    }
}

/// The `sdr.status` reading body: `telemetry.v1`-style items so generic consumers can use it.
pub fn status_body(source: &str, node: &str, t_ns: u64, seq: u64, doc: &Value) -> Value {
    let mut items = Vec::new();
    let mut add = |name: &str, v: &Value, unit: &str| {
        if let Some(x) = v.as_f64() {
            items.push(json!({"name": name, "value": x, "unit": unit}));
        } else if let Some(b) = v.as_bool() {
            items.push(json!({"name": name, "value": if b { 1.0 } else { 0.0 }, "unit": ""}));
        }
    };
    add("healthy", &doc["healthy"], "");
    add("lo", &doc["hw"]["lo_hz"], "Hz");
    add("rate", &doc["hw"]["rate_sps"], "S/s");
    add("bandwidth", &doc["hw"]["bw_hz"], "Hz");
    add("gain", &doc["hw"]["gain_db"], "dB");
    add("rssi", &doc["hw"]["rssi_db"], "dB");
    add("temp.phy", &doc["hw"]["temp_phy_c"], "Cel");
    add("temp.zynq", &doc["hw"]["temp_zynq_c"], "Cel");
    add("power", &doc["power_dbfs"], "dBFS");
    add("samples.delivered", &doc["samples"]["delivered"], "");
    add("samples.expected", &doc["samples"]["expected"], "");
    add("samples.lost", &doc["samples"]["lost"], "");
    add(
        "samples.pending",
        &doc["samples"]["pending_unconfirmed"],
        "",
    );
    add("loss.fraction", &doc["samples"]["loss_fraction"], "");
    add("gap.events", &doc["counters"]["gap_events"], "");
    add("overflows", &doc["counters"]["overflows"], "");
    add("zero.blocks", &doc["counters"]["zero_blocks"], "");
    add(
        "dead.receive.path",
        &doc["counters"]["dead_receive_path"],
        "",
    );
    add("retunes", &doc["counters"]["retunes"], "");
    json!({"source_id": source, "node_id": node, "t_ns": t_ns, "seq": seq, "items": items})
}

/// A record on its way to the ring writer thread.
struct Job {
    t_ns: u64,
    head: iq::Header,
    samples: Vec<u8>,
}

#[derive(Default)]
struct RingStats {
    seq: AtomicU64,
    dropped: AtomicU64,
    errors: AtomicU64,
}

/// Encodes and writes records to the ring. It runs on its own thread so that building a 44 KB
/// JSON message (base64 and all) never delays a read from the radio: the acquisition thread only
/// copies the samples into a bounded queue, and a record that does not fit the queue is dropped
/// and counted (`ring_dropped`) rather than letting the radio's kernel buffer overflow.
struct RingTask {
    link: Arc<Link>,
    source: String,
    ring: Option<(u64, RingWriter)>,
    stats: Arc<RingStats>,
}

impl RingTask {
    /// The ring of the current connection, taken from the slot the connection thread fills.
    fn ensure_ring(&mut self) -> Option<&mut RingWriter> {
        let gen = self.link.gen.load(Ordering::Acquire);
        if self.ring.as_ref().is_some_and(|(g, _)| *g != gen) {
            self.ring = None;
        }
        if self.ring.is_none() {
            let mut slot = self.link.ring.try_lock().ok()?;
            if slot.as_ref().is_some_and(|(g, _)| *g == gen) {
                self.ring = slot.take();
            }
        }
        self.ring.as_mut().map(|(_, w)| w)
    }

    fn write(&mut self, job: &Job) {
        let node = self
            .link
            .get()
            .1
            .map(|b| b.node_id.clone())
            .filter(|n| n.len() == 32)
            .unwrap_or_else(|| "0".repeat(32));
        let source = self.source.clone();
        let Some(w) = self.ensure_ring() else {
            return;
        };
        let meta = iq::Meta {
            source_id: &source,
            node_id: &node,
        };
        let msg = job.head.encode(&job.samples, &meta, job.t_ns, w.head() + 1);
        match w.write(job.t_ns, wall_ns(), &msg) {
            Ok(seq) => {
                self.stats.seq.store(seq, Ordering::Relaxed);
                self.link.last_seq.store(seq, Ordering::Relaxed);
            }
            Err(e) => {
                self.stats.errors.fetch_add(1, Ordering::Relaxed);
                eprintln!("[cog-sdr-pluto-rx] cogbus: ring write: {e}");
            }
        }
    }
}

/// Writes the IQ stream to its ring and the status reading to the bus; does nothing while there
/// is no connection.
pub struct BusOut {
    link: Arc<Link>,
    source: String,
    jobs: SyncSender<Job>,
    stats: Arc<RingStats>,
    reading_seq: u64,
}

/// Records the queue holds: 256 of 8192 samples is 0.4 s at 5 MS/s.
const QUEUE: usize = 256;

impl BusOut {
    pub fn new(link: Arc<Link>, source: &str) -> Self {
        let (jobs, rx) = sync_channel::<Job>(QUEUE);
        let stats = Arc::new(RingStats::default());
        let mut task = RingTask {
            link: link.clone(),
            source: source.to_string(),
            ring: None,
            stats: stats.clone(),
        };
        std::thread::Builder::new()
            .name("ring".into())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    task.write(&job);
                }
            })
            .ok();
        Self {
            link,
            source: source.to_string(),
            jobs,
            stats,
            reading_seq: 0,
        }
    }
}

impl Out for BusOut {
    fn record(&mut self, t_ns: u64, h: &iq::Header, samples: &[u8]) -> Result<u64, String> {
        if self.link.get().1.is_none() {
            return Ok(0);
        }
        let job = Job {
            t_ns,
            head: h.clone(),
            samples: samples.to_vec(),
        };
        match self.jobs.try_send(job) {
            Ok(()) => Ok(self.stats.seq.load(Ordering::Relaxed)),
            Err(_) => {
                self.stats.dropped.fetch_add(1, Ordering::Relaxed);
                Err("the ring writer is behind: record dropped".into())
            }
        }
    }

    fn status(&mut self, t_ns: u64, doc: &Value) {
        let (_, bus) = self.link.get();
        let Some(bus) = bus else { return };
        self.reading_seq += 1;
        let body = status_body(&self.source, &bus.node_id, t_ns, self.reading_seq, doc);
        if let Err(e) = bus.publish(STATUS_KIND, &self.source, STATUS_SCHEMA, t_ns, body) {
            eprintln!("[cog-sdr-pluto-rx] cogbus: publish failed: {e}");
        }
    }

    fn info(&self) -> Value {
        let (_, bus) = self.link.get();
        json!({
            "connected": bus.is_some(),
            "node_id": bus.map(|b| b.node_id.clone()),
            "connects": self.link.connects.load(Ordering::Relaxed),
            "ring_seq": self.stats.seq.load(Ordering::Relaxed),
            "ring_dropped": self.stats.dropped.load(Ordering::Relaxed),
            "ring_errors": self.stats.errors.load(Ordering::Relaxed),
        })
    }
}

fn wall_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests;
