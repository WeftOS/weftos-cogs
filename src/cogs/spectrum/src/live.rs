//! The live loop: subscribe to the owner's IQ ring on the node's cogbus, read records, feed the
//! collector and publish what it produces. It survives the owner and the broker restarting: it
//! reconnects, resubscribes and takes the new ring when the broker announces one.

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::bus::{Bus, RingGrant};
use crate::collect::{Collector, Config, Event};
use crate::iq::Header;
use crate::report::{Reporter, MONO};
use crate::ring::RingReader;
use crate::shared::Shared;

pub const IQ_KIND: &str = "iq.cs16";
pub const IQ_SCHEMA: &str = "iq.cs16.v1";
pub const PSD_KIND: &str = "spectrum.psd";
pub const PSD_SCHEMA: &str = "spectrum.psd.v1";
pub const OCC_KIND: &str = "band.occupancy";
pub const OCC_SCHEMA: &str = "telemetry.v1";
pub const SVC_KIND: &str = "svc.sdr";
pub const SVC_SCHEMA: &str = "sdr.v1";

pub struct LiveOpts {
    pub path: String,
    pub token: String,
    /// Source id of the owner's streams.
    pub source: String,
    /// Source id of this cog's readings.
    pub service: String,
    /// A `plan` call body to send to the owner after subscribing.
    pub ask_plan: Option<Value>,
}

/// Where readings go.
pub trait Publish {
    fn publish(&mut self, kind: &str, schema: &str, t_ns: u64, body: Value);
}

struct BusPub<'a>(&'a Bus, &'a str);

impl Publish for BusPub<'_> {
    fn publish(&mut self, kind: &str, schema: &str, t_ns: u64, body: Value) {
        if let Err(e) = self.0.publish(kind, self.1, schema, t_ns, body) {
            eprintln!("[cog-spectrum] publish failed: {e}");
        }
    }
}

/// Send each event out as its reading and let the reporter keep the numbers.
pub fn emit(
    events: Vec<Event>,
    node: &str,
    source: &str,
    rep: &mut Reporter,
    out: &mut dyn Publish,
    shared: &Shared,
) {
    for e in events {
        match e {
            Event::Psd(p) => {
                let body = rep.psd(&p, source, node);
                if let Ok(mut s) = shared.psd.lock() {
                    *s = body.clone();
                }
                out.publish(PSD_KIND, PSD_SCHEMA, p.t_ns, body);
            }
            Event::Occupancy(o) => {
                let body = rep.occupancy(&o, source, node);
                out.publish(OCC_KIND, OCC_SCHEMA, o.t_ns, body);
            }
        }
    }
}

fn handle_record(data: &[u8], t_ns: u64, col: &mut Collector, rep: &mut Reporter) -> Vec<Event> {
    match Header::decode(data) {
        Ok((h, samples)) => {
            rep.records += 1;
            rep.last_record_ns = MONO.now();
            col.on_record(&h, &samples, t_ns, MONO.now())
        }
        Err(e) => {
            rep.decode_errors += 1;
            if rep.decode_errors <= 3 {
                eprintln!("[cog-spectrum] bad record: {e}");
            }
            Vec::new()
        }
    }
}

/// One connection's worth of reading; returns when the connection ends or `shared.stop` is set.
fn session(
    o: &LiveOpts,
    cfg: &Config,
    shared: &Arc<Shared>,
    rep: &mut Reporter,
    col: &mut Collector,
) -> Result<(), String> {
    let (bus, notices) = Bus::connect(&o.path, &o.token)?;
    let topic = format!("{IQ_KIND}/{}", o.source);
    let (sub, grant) = bus.stream_subscribe(&topic, IQ_SCHEMA)?;
    eprintln!(
        "[cog-spectrum] subscribed to {topic} on node {}",
        bus.node_id
    );
    rep.bus_node = Some(bus.node_id.clone());
    let mut ring: Option<(RingReader, u64)> = None;
    if let Some(g) = grant {
        ring = Some((g.map()?, g.head + 1));
    }
    if let Some(body) = &o.ask_plan {
        match bus.call(
            &format!("{SVC_KIND}/{}", o.source),
            SVC_SCHEMA,
            body.clone(),
            Duration::from_secs(20),
        ) {
            Ok(r) => eprintln!("[cog-spectrum] the owner took our plan: {r}"),
            Err(e) => eprintln!("[cog-spectrum] the owner refused our plan: {e}"),
        }
    }
    let mut out = BusPub(&bus, &o.service);
    let mut last_tick = Instant::now();
    let mut last_status = Instant::now() - Duration::from_secs(10);
    while !shared.stop.load(Ordering::Acquire) && !bus.is_closed() {
        while let Ok((f, fds)) = notices.try_recv() {
            if let Some((s, g)) = RingGrant::from_notice(&f, fds) {
                if s == sub {
                    ring = Some((g.map()?, g.head + 1));
                    rep.ring_changes += 1;
                }
            }
        }
        let mut got = 0;
        if let Some((r, next)) = ring.as_ref().map(|(r, n)| (r, *n)) {
            let mut next = next;
            while got < 512 {
                let Some(rec) = r.read(&mut next) else { break };
                got += 1;
                let ev = handle_record(&rec.data, rec.t_ns, col, rep);
                emit(ev, &bus.node_id, &o.service, rep, &mut out, shared);
            }
            if let Some((_, n)) = ring.as_mut() {
                *n = next;
            }
        }
        if got == 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
        if last_tick.elapsed() >= Duration::from_millis(250) {
            last_tick = Instant::now();
            let ev = col.tick(MONO.now());
            emit(ev, &bus.node_id, &o.service, rep, &mut out, shared);
        }
        if last_status.elapsed() >= Duration::from_secs(1) {
            last_status = Instant::now();
            rep.publish_status(shared, col, "streaming", None, cfg);
        }
    }
    Ok(())
}

/// Run until `shared.stop`: connect, read, reconnect.
pub fn run(o: LiveOpts, cfg: Config, shared: Arc<Shared>, mut rep: Reporter, mut col: Collector) {
    let mut backoff = 1u64;
    while !shared.stop.load(Ordering::Acquire) {
        rep.publish_status(&shared, &col, "waiting", None, &cfg);
        match session(&o, &cfg, &shared, &mut rep, &mut col) {
            Ok(()) => backoff = 1,
            Err(e) => {
                rep.publish_status(&shared, &col, "no_source", Some(e.clone()), &cfg);
                eprintln!("[cog-spectrum] {e}; retrying in {backoff} s");
            }
        }
        for _ in 0..backoff * 10 {
            if shared.stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        backoff = (backoff * 2).min(15);
    }
}
