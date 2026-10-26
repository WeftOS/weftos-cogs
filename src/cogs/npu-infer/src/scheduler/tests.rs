//! The scheduler against a fake backend: order, eviction, restarts,
//! refusals and two consumers sharing the NPU.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use super::*;
use crate::backend::{BackendError, BlobRef, BlobSource, Fault};
use crate::catalog::{Catalog, Kind};
use crate::sd::NoGate;

type Log = Arc<Mutex<Vec<String>>>;

struct Fake {
    log: Log,
    costs: Vec<(&'static str, u64)>,
    /// Loads that fail (restartable) before one succeeds.
    fail_loads: usize,
    /// Infers that fail with `Dead` before one succeeds.
    dead_infers: usize,
    /// Every infer fails (not restartable).
    fail_all: bool,
    resident: Vec<String>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    delay: Duration,
    avail: Option<u64>,
}

impl Fake {
    fn new(log: &Log) -> Self {
        Self {
            log: log.clone(),
            costs: vec![],
            fail_loads: 0,
            dead_infers: 0,
            fail_all: false,
            resident: vec![],
            in_flight: Arc::new(AtomicUsize::new(0)),
            max_in_flight: Arc::new(AtomicUsize::new(0)),
            delay: Duration::ZERO,
            avail: None,
        }
    }
}

impl Backend for Fake {
    fn load(&mut self, e: &Entry) -> Result<u64, BackendError> {
        self.log.lock().unwrap().push(format!("load:{}", e.alias));
        if self.fail_loads > 0 {
            self.fail_loads -= 1;
            return Err(BackendError::new(
                Fault::LoadFailed,
                "SDKError(Model loading failed)",
            ));
        }
        self.resident.push(e.alias.clone());
        Ok(self
            .costs
            .iter()
            .find(|(a, _)| *a == e.alias)
            .map_or(e.est_mib, |c| c.1))
    }

    fn unload(&mut self, alias: &str) {
        self.log.lock().unwrap().push(format!("unload:{alias}"));
        self.resident.retain(|a| a != alias);
    }

    fn infer(&mut self, e: &Entry, req: &Request) -> Result<Output, BackendError> {
        let n = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(n, Ordering::SeqCst);
        std::thread::sleep(self.delay);
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.log.lock().unwrap().push(format!("infer:{}", e.alias));
        if self.dead_infers > 0 {
            self.dead_infers -= 1;
            return Err(BackendError::new(Fault::Dead, "geniex exited"));
        }
        if self.fail_all || !self.resident.contains(&e.alias) {
            return Err(BackendError::new(Fault::Failed, "context_length_exceeded"));
        }
        Ok(Output {
            text: format!(
                "{}:{}{}",
                e.alias,
                req.text.clone().unwrap_or_default(),
                req.image
                    .as_ref()
                    .map_or(String::new(), |(m, b)| format!("+img:{m}:{b}"))
            ),
            finish: "stop".into(),
            prompt_tokens: 1,
            completion_tokens: 1,
        })
    }

    fn reset(&mut self, alias: &str) {
        self.log.lock().unwrap().push(format!("reset:{alias}"));
        self.resident.retain(|a| a != alias);
    }

    fn mem_available_mib(&self) -> Option<u64> {
        self.avail
    }
}

fn entry(alias: &str) -> Entry {
    let c = Catalog::ventuno();
    c.resolve(alias).unwrap().clone()
}

type Answer = Result<Value, String>;

/// Blob bytes from a fixed result, counting reads.
struct Blobs(Result<Vec<u8>, String>, Arc<AtomicUsize>);

impl Blobs {
    fn none() -> Self {
        Blobs(Err("no blobs".into()), Arc::default())
    }
}

impl BlobSource for Blobs {
    fn fetch(&self, _: &BlobRef) -> Result<Vec<u8>, String> {
        self.1.fetch_add(1, Ordering::SeqCst);
        self.0.clone()
    }
}

fn job(alias: &str, text: &str, priority: u8, tx: &mpsc::Sender<(String, Answer)>) -> Job {
    let tx = tx.clone();
    let tag = text.to_string();
    Job {
        entry: entry(alias),
        req: Request {
            system: None,
            text: Some(text.into()),
            image: None,
            image_blob: None,
            max_tokens: 8,
        },
        priority,
        enq: Instant::now(),
        deadline: Instant::now() + Duration::from_secs(30),
        reply: Box::new(move |r| {
            let _ = tx.send((tag, r));
        }),
    }
}

fn recv(rx: &mpsc::Receiver<(String, Answer)>) -> (String, Answer) {
    rx.recv_timeout(Duration::from_secs(10)).expect("an answer")
}

fn log_of(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

#[test]
fn calls_run_highest_priority_first_and_one_at_a_time() {
    let log: Log = Default::default();
    let mut fake = Fake::new(&log);
    fake.delay = Duration::from_millis(20);
    let max = fake.max_in_flight.clone();
    let s = Scheduler::new(32, 20_000);
    let (tx, rx) = mpsc::channel();
    for (name, p) in [("low", 1), ("mid", 4), ("high", 9), ("mid2", 4)] {
        assert!(s.submit(job("gemma3-1b", name, p, &tx)).is_ok());
    }
    assert_eq!(s.snapshot().depth, 4);
    let h = s.run_worker(
        Box::new(fake),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(20_000, 0, 3),
    );
    let order: Vec<String> = (0..4).map(|_| recv(&rx).0).collect();
    assert_eq!(order, ["high", "mid", "mid2", "low"]);
    assert_eq!(max.load(Ordering::SeqCst), 1);
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn within_a_priority_a_call_for_a_resident_model_runs_before_one_that_needs_a_load() {
    let log: Log = Default::default();
    let s = Scheduler::new(32, 20_000);
    let h = s.run_worker(
        Box::new(Fake::new(&log)),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(20_000, 0, 3),
    );
    let (tx, rx) = mpsc::channel();
    // make gemma3-1b resident first
    s.submit(job("gemma3-1b", "warmup", 4, &tx)).ok().unwrap();
    recv(&rx).1.unwrap();
    // hold the worker with a slow-to-start queue: stop it, queue, restart is not possible,
    // so queue behind a running job instead
    let mut slow = job("gemma3-1b", "slow", 4, &tx);
    let reply = std::mem::replace(&mut slow.reply, Box::new(|_| {}));
    let (gate_tx, gate_rx) = mpsc::channel::<()>();
    slow.reply = Box::new(move |r| {
        reply(r);
        let _ = gate_rx.recv_timeout(Duration::from_secs(5));
    });
    s.submit(slow).ok().unwrap();
    assert_eq!(recv(&rx).0, "slow");
    // the worker is now parked in slow's reply; queue a cold job first, then a warm one
    s.submit(job("qwen3.5-0.8b", "cold", 4, &tx)).ok().unwrap();
    s.submit(job("gemma3-1b", "warm", 4, &tx)).ok().unwrap();
    gate_tx.send(()).unwrap();
    let order: Vec<String> = (0..2).map(|_| recv(&rx).0).collect();
    assert_eq!(order, ["warm", "cold"]);
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_full_queue_hands_the_call_back_and_counts_it() {
    let s = Scheduler::new(1, 1000);
    let (tx, _rx) = mpsc::channel();
    assert!(s.submit(job("gemma3-1b", "a", 4, &tx)).is_ok());
    assert!(s.submit(job("gemma3-1b", "b", 4, &tx)).is_err());
    assert_eq!(s.snapshot().counters.rejected, 1);
}

#[test]
fn the_least_recently_used_model_is_evicted_to_stay_in_budget() {
    let log: Log = Default::default();
    let mut fake = Fake::new(&log);
    fake.costs = vec![
        ("gemma3-1b", 3000),
        ("qwen3.5-0.8b", 3000),
        ("qwen3-4b", 3000),
    ];
    // qwen3-4b is estimated at 4500 until it has loaded: 3000 + 4500 fits 8000 only
    // with one of the two resident models gone.
    let s = Scheduler::new(32, 8000);
    let h = s.run_worker(
        Box::new(fake),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(8000, 0, 8),
    );
    let (tx, rx) = mpsc::channel();
    for (m, t) in [
        ("gemma3-1b", "1"),
        ("qwen3.5-0.8b", "2"),
        ("gemma3-1b", "3"),
        ("qwen3-4b", "4"),
    ] {
        s.submit(job(m, t, 4, &tx)).ok().unwrap();
        assert!(recv(&rx).1.is_ok());
    }
    // after 1,2,3 the order of use is qwen3.5 then gemma3: loading qwen3-4b evicts qwen3.5
    let log = log_of(&log);
    let unloads: Vec<_> = log.iter().filter(|l| l.starts_with("unload:")).collect();
    assert_eq!(unloads, ["unload:qwen3.5-0.8b"], "{log:?}");
    let snap = s.snapshot();
    let mut have: Vec<_> = snap.resident.iter().map(|r| r.alias.as_str()).collect();
    have.sort();
    assert_eq!(have, ["gemma3-1b", "qwen3-4b"]);
    assert_eq!(snap.used_mib, 6000);
    assert!(snap.used_mib <= snap.budget_mib);
    assert_eq!(snap.counters.evictions, 1);
    assert_eq!(snap.counters.loads, 3);
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_model_that_measures_bigger_than_estimated_pushes_the_older_one_out_after_loading() {
    let log: Log = Default::default();
    let mut fake = Fake::new(&log);
    // qwen3-vl-4b is estimated at 6000 and measures 7300
    fake.costs = vec![("gemma3-1b", 1200), ("qwen3-vl-4b", 7300)];
    let s = Scheduler::new(8, 8000);
    let h = s.run_worker(
        Box::new(fake),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(8000, 0, 4),
    );
    let (tx, rx) = mpsc::channel();
    s.submit(job("gemma3-1b", "a", 4, &tx)).ok().unwrap();
    assert!(recv(&rx).1.is_ok());
    s.submit(job("qwen3-vl-4b", "b", 4, &tx)).ok().unwrap();
    assert!(recv(&rx).1.is_ok());
    let snap = s.snapshot();
    assert_eq!(snap.resident.len(), 1, "{snap:?}");
    assert!(snap.used_mib <= snap.budget_mib);
    assert!(log_of(&log).contains(&"unload:gemma3-1b".to_string()));
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_model_that_cannot_fit_is_refused_and_nothing_is_evicted() {
    let log: Log = Default::default();
    let s = Scheduler::new(8, 5000);
    let h = s.run_worker(
        Box::new(Fake::new(&log)),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(5000, 0, 4),
    );
    let (tx, rx) = mpsc::channel();
    s.submit(job("gemma3-1b", "a", 4, &tx)).ok().unwrap();
    assert!(recv(&rx).1.is_ok());
    s.submit(job("qwen2.5-vl-7b", "b", 4, &tx)).ok().unwrap();
    let e = recv(&rx).1.unwrap_err();
    assert!(e.contains("qwen2.5-vl-7b") && e.contains("MiB"), "{e}");
    assert!(!log_of(&log).iter().any(|l| l.starts_with("unload")));
    assert_eq!(s.snapshot().resident.len(), 1);
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_failed_load_restarts_through_the_wrapper_and_succeeds() {
    let log: Log = Default::default();
    let mut fake = Fake::new(&log);
    fake.fail_loads = 1;
    let s = Scheduler::new(8, 20_000);
    let h = s.run_worker(
        Box::new(fake),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(20_000, 0, 4),
    );
    let (tx, rx) = mpsc::channel();
    s.submit(job("gemma3-1b", "a", 4, &tx)).ok().unwrap();
    let v = recv(&rx).1.unwrap();
    assert_eq!(v["text"], "gemma3-1b:a");
    assert_eq!(v["loaded"], true);
    assert_eq!(
        log_of(&log),
        [
            "load:gemma3-1b",
            "reset:gemma3-1b",
            "load:gemma3-1b",
            "infer:gemma3-1b"
        ]
    );
    assert_eq!(s.snapshot().counters.restarts, 1);
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn two_failed_loads_report_the_failure_and_leave_nothing_resident() {
    let log: Log = Default::default();
    let mut fake = Fake::new(&log);
    fake.fail_loads = 2;
    let s = Scheduler::new(8, 20_000);
    let h = s.run_worker(
        Box::new(fake),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(20_000, 0, 4),
    );
    let (tx, rx) = mpsc::channel();
    s.submit(job("gemma3-1b", "a", 4, &tx)).ok().unwrap();
    let e = recv(&rx).1.unwrap_err();
    assert!(e.contains("failed to load twice"), "{e}");
    assert!(s.snapshot().resident.is_empty());
    // the next call gets a clean attempt
    s.submit(job("gemma3-1b", "b", 4, &tx)).ok().unwrap();
    assert!(recv(&rx).1.is_ok());
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_server_that_died_is_restarted_and_the_call_still_answers() {
    let log: Log = Default::default();
    let mut fake = Fake::new(&log);
    fake.dead_infers = 1;
    let s = Scheduler::new(8, 20_000);
    let h = s.run_worker(
        Box::new(fake),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(20_000, 0, 4),
    );
    let (tx, rx) = mpsc::channel();
    s.submit(job("gemma3-1b", "x", 4, &tx)).ok().unwrap();
    let v = recv(&rx).1.unwrap();
    assert_eq!(v["text"], "gemma3-1b:x");
    assert_eq!(
        log_of(&log),
        [
            "load:gemma3-1b",
            "infer:gemma3-1b",
            "reset:gemma3-1b",
            "load:gemma3-1b",
            "infer:gemma3-1b"
        ]
    );
    assert_eq!(s.snapshot().counters.restarts, 1);
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_model_error_is_reported_without_a_restart() {
    let log: Log = Default::default();
    let mut fake = Fake::new(&log);
    fake.fail_all = true;
    let s = Scheduler::new(8, 20_000);
    let h = s.run_worker(
        Box::new(fake),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(20_000, 0, 4),
    );
    let (tx, rx) = mpsc::channel();
    s.submit(job("gemma3-1b", "a", 4, &tx)).ok().unwrap();
    assert_eq!(recv(&rx).1.unwrap_err(), "context_length_exceeded");
    assert_eq!(s.snapshot().counters.restarts, 0);
    assert_eq!(s.snapshot().counters.failed, 1);
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_call_past_its_deadline_is_dropped_unrun() {
    let log: Log = Default::default();
    let s = Scheduler::new(8, 20_000);
    let (tx, rx) = mpsc::channel();
    let mut j = job("gemma3-1b", "late", 4, &tx);
    j.deadline = Instant::now() - Duration::from_millis(1);
    s.submit(j).ok().unwrap();
    let h = s.run_worker(
        Box::new(Fake::new(&log)),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(20_000, 0, 4),
    );
    assert_eq!(recv(&rx).1.unwrap_err(), "expired in the queue");
    assert!(log_of(&log).is_empty(), "no load, no infer");
    assert_eq!(s.snapshot().counters.expired, 1);
    s.shutdown();
    h.join().unwrap();
}

struct Busy;

impl Gate for Busy {
    fn wait_idle(&self, _: Instant) -> Result<(), String> {
        Err("the image service held the NPU until the deadline".into())
    }

    fn busy(&self) -> Option<bool> {
        Some(true)
    }
}

#[test]
fn a_call_does_not_run_while_the_image_service_holds_the_npu() {
    let log: Log = Default::default();
    let s = Scheduler::new(8, 20_000);
    let h = s.run_worker(
        Box::new(Fake::new(&log)),
        Box::new(Busy),
        Box::new(Blobs::none()),
        Residency::new(20_000, 0, 4),
    );
    let (tx, rx) = mpsc::channel();
    s.submit(job("gemma3-1b", "a", 4, &tx)).ok().unwrap();
    assert!(recv(&rx).1.unwrap_err().contains("image service"));
    assert!(!log_of(&log).iter().any(|l| l.starts_with("infer")));
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn two_consumers_with_different_models_share_the_npu_within_the_budget() {
    let log: Log = Default::default();
    let mut fake = Fake::new(&log);
    fake.delay = Duration::from_millis(5);
    fake.costs = vec![("qwen3-4b", 4500), ("qwen3-vl-4b", 6000)];
    let max = fake.max_in_flight.clone();
    // 4500 + 6000 does not fit 8000: each switch evicts, never two at once
    let s = Scheduler::new(64, 8000);
    let h = s.run_worker(
        Box::new(fake),
        Box::new(NoGate),
        Box::new(Blobs::none()),
        Residency::new(8000, 0, 4),
    );
    let (tx, rx) = mpsc::channel();
    let mut threads = Vec::new();
    for (who, model) in [("a", "qwen3-4b"), ("b", "qwen3-vl-4b")] {
        let (s, tx) = (s.clone(), tx.clone());
        threads.push(std::thread::spawn(move || {
            for i in 0..6 {
                let (t2, r2) = mpsc::channel();
                let mut j = job(
                    model,
                    &format!("{who}{i}"),
                    if who == "a" { 5 } else { 4 },
                    &tx,
                );
                // wait for each answer, as a blocking caller does
                let reply = std::mem::replace(&mut j.reply, Box::new(|_| {}));
                j.reply = Box::new(move |r| {
                    reply(r.clone());
                    let _ = t2.send(());
                });
                s.submit(j).ok().unwrap();
                r2.recv_timeout(Duration::from_secs(10)).unwrap();
            }
        }));
    }
    drop(tx);
    let answers: Vec<_> = rx.iter().collect();
    for t in threads {
        t.join().unwrap();
    }
    assert_eq!(answers.len(), 12);
    assert!(answers.iter().all(|(_, r)| r.is_ok()));
    assert_eq!(max.load(Ordering::SeqCst), 1);
    let snap = s.snapshot();
    assert!(snap.used_mib <= 8000, "{} MiB resident", snap.used_mib);
    assert!(snap.counters.evictions >= 1);
    assert_eq!(snap.counters.served, 12);
    // every load that was not the first was preceded by an unload: never both resident
    let mut resident = 0i32;
    for l in log_of(&log) {
        if l.starts_with("load:") {
            resident += 1;
        } else if l.starts_with("unload:") {
            resident -= 1;
        }
        assert!(resident <= 1, "two models resident at once");
    }
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_vlm_is_a_vlm() {
    assert_eq!(entry("qwen3-vl-4b").kind, Kind::Vlm);
}

fn blob_job(alias: &str, tx: &mpsc::Sender<(String, Answer)>) -> Job {
    let mut j = job(alias, "describe", 4, tx);
    j.req.image_blob = Some(BlobRef {
        hash: "ab".repeat(32),
        size: 4,
        media_type: "image/png".into(),
    });
    j
}

#[test]
fn an_image_blob_is_read_when_the_call_runs_and_reaches_the_model_as_an_image() {
    let log: Log = Default::default();
    let reads = Arc::new(AtomicUsize::new(0));
    let s = Scheduler::new(8, 20_000);
    let (tx, rx) = mpsc::channel();
    // queued before the worker runs: the handle is held, nothing is read yet
    s.submit(blob_job("qwen3-vl-4b", &tx)).ok().unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    let blobs = Blobs(Ok(vec![0xde, 0xad, 0xbe, 0xef]), reads.clone());
    let h = s.run_worker(
        Box::new(Fake::new(&log)),
        Box::new(NoGate),
        Box::new(blobs),
        Residency::new(20_000, 0, 3),
    );
    let v = recv(&rx).1.unwrap();
    assert_eq!(v["text"], "qwen3-vl-4b:describe+img:image/png:3q2+7w==");
    assert_eq!(reads.load(Ordering::SeqCst), 1);
    s.shutdown();
    h.join().unwrap();
}

#[test]
fn a_blob_that_cannot_be_read_fails_the_call_before_any_model_loads() {
    let log: Log = Default::default();
    let s = Scheduler::new(8, 20_000);
    let blobs = Blobs(Err("denied: not yours".into()), Arc::default());
    let h = s.run_worker(
        Box::new(Fake::new(&log)),
        Box::new(NoGate),
        Box::new(blobs),
        Residency::new(20_000, 0, 3),
    );
    let (tx, rx) = mpsc::channel();
    s.submit(blob_job("qwen3-vl-4b", &tx)).ok().unwrap();
    let e = recv(&rx).1.unwrap_err();
    assert!(
        e.starts_with("image blob abababababab") && e.contains("not yours"),
        "{e}"
    );
    assert!(
        log_of(&log).is_empty(),
        "no load for a call that cannot run"
    );
    assert_eq!(s.snapshot().counters.failed, 1);
    s.shutdown();
    h.join().unwrap();
}
