//! One worker thread runs the calls, one at a time, highest priority first:
//! the NPU is a single resource, and running two models at once is what
//! pushed the board out of memory. For each call the worker makes the model
//! resident (evicting least recently used ones to stay in budget), waits
//! for the image service to be idle, generates, and answers through the
//! call's reply function. A model that fails to load, or a server that has
//! died, is restarted through the wrapper and tried once more.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::b64;
use crate::backend::{Backend, BlobSource, Gate, Output, Request};
use crate::catalog::Entry;
use crate::queue::PriorityQueue;
use crate::residency::{Plan, Residency};

pub type ReplyFn = Box<dyn FnOnce(Result<Value, String>) + Send>;

pub struct Job {
    pub entry: Entry,
    pub req: Request,
    pub priority: u8,
    pub enq: Instant,
    /// After this the caller has been told "timed out"; do not start it.
    pub deadline: Instant,
    pub reply: ReplyFn,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Counters {
    pub served: u64,
    pub failed: u64,
    /// Calls turned away because the queue was full.
    pub rejected: u64,
    /// Calls dropped unrun because their deadline had passed.
    pub expired: u64,
    pub loads: u64,
    pub evictions: u64,
    pub restarts: u64,
}

#[derive(Debug, Clone)]
pub struct ResidentInfo {
    pub alias: String,
    pub cost_mib: u64,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub depth: usize,
    /// Alias being run now.
    pub running: Option<String>,
    pub oldest_wait_ms: u64,
    pub resident: Vec<ResidentInfo>,
    pub used_mib: u64,
    pub budget_mib: u64,
    /// Measured cost of every model that has loaded, for the next start.
    pub measured: Vec<(String, u64)>,
    pub counters: Counters,
}

struct State {
    queue: PriorityQueue<Job>,
    running: Option<String>,
    resident: Vec<ResidentInfo>,
    used_mib: u64,
    measured: Vec<(String, u64)>,
    counters: Counters,
    shutdown: bool,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
    budget_mib: u64,
}

#[derive(Clone)]
pub struct Scheduler {
    shared: Arc<Shared>,
}

impl Scheduler {
    pub fn new(queue_cap: usize, budget_mib: u64) -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    queue: PriorityQueue::new(queue_cap),
                    running: None,
                    resident: Vec::new(),
                    used_mib: 0,
                    measured: Vec::new(),
                    counters: Counters::default(),
                    shutdown: false,
                }),
                cv: Condvar::new(),
                budget_mib,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Queue `job`. When the queue is full the job is handed back unrun.
    pub fn submit(&self, job: Job) -> Result<(), Box<Job>> {
        let mut g = self.lock();
        let (p, now) = (job.priority, Instant::now());
        match g.queue.push(p, job, now) {
            Ok(()) => {
                drop(g);
                self.shared.cv.notify_one();
                Ok(())
            }
            Err(job) => {
                g.counters.rejected += 1;
                Err(Box::new(job))
            }
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = self.lock();
        Snapshot {
            depth: g.queue.len(),
            running: g.running.clone(),
            oldest_wait_ms: g.queue.oldest_wait(Instant::now()).as_millis() as u64,
            resident: g.resident.clone(),
            used_mib: g.used_mib,
            budget_mib: self.shared.budget_mib,
            measured: g.measured.clone(),
            counters: g.counters.clone(),
        }
    }

    pub fn shutdown(&self) {
        self.lock().shutdown = true;
        self.shared.cv.notify_all();
    }

    /// Start the worker. It owns the backend and the residency book.
    pub fn run_worker(
        &self,
        backend: Box<dyn Backend>,
        gate: Box<dyn Gate>,
        blobs: Box<dyn BlobSource>,
        residency: Residency,
    ) -> JoinHandle<()> {
        let s = self.clone();
        std::thread::spawn(move || s.work(backend, gate, blobs, residency))
    }

    fn work(
        &self,
        mut backend: Box<dyn Backend>,
        gate: Box<dyn Gate>,
        blobs: Box<dyn BlobSource>,
        mut res: Residency,
    ) {
        loop {
            let job = {
                let mut g = self.lock();
                loop {
                    if g.shutdown {
                        return;
                    }
                    let warm: Vec<String> = g.resident.iter().map(|r| r.alias.clone()).collect();
                    if let Some(j) = g
                        .queue
                        .pop_preferring(Instant::now(), |j| warm.contains(&j.entry.alias))
                    {
                        g.running = Some(j.entry.alias.clone());
                        break j;
                    }
                    g = self.shared.cv.wait(g).unwrap_or_else(|p| p.into_inner());
                }
            };
            let outcome = self.run(
                job,
                backend.as_mut(),
                gate.as_ref(),
                blobs.as_ref(),
                &mut res,
            );
            let mut g = self.lock();
            g.running = None;
            match outcome {
                Outcome::Served => g.counters.served += 1,
                Outcome::Failed => g.counters.failed += 1,
                Outcome::Expired => g.counters.expired += 1,
            }
            drop(g);
            self.sync(&res);
        }
    }

    fn sync(&self, res: &Residency) {
        let mut g = self.lock();
        g.resident = res
            .resident()
            .iter()
            .map(|r| ResidentInfo {
                alias: r.alias.clone(),
                cost_mib: r.cost_mib,
            })
            .collect();
        g.used_mib = res.used_mib();
        let mut m: Vec<_> = res
            .measured()
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect();
        m.sort();
        g.measured = m;
    }

    fn bump(&self, f: impl FnOnce(&mut Counters)) {
        f(&mut self.lock().counters);
    }

    /// Make `e` resident. `Ok(true)` when it had to load.
    fn ensure(&self, e: &Entry, be: &mut dyn Backend, res: &mut Residency) -> Result<bool, String> {
        match res.plan(e, be.mem_available_mib()) {
            Plan::Hit => {
                res.touch(&e.alias);
                Ok(false)
            }
            Plan::TooBig {
                need_mib,
                limit_mib,
            } => Err(format!(
                "{} needs about {need_mib} MiB and this node can give a model {limit_mib} MiB",
                e.alias
            )),
            Plan::Load { evict, .. } => {
                for a in evict {
                    be.unload(&a);
                    res.remove(&a);
                    self.bump(|c| c.evictions += 1);
                }
                self.sync(res);
                let cost = match be.load(e) {
                    Ok(c) => c,
                    Err(first) if first.restartable() => {
                        // Restart through the wrapper and try once more.
                        be.reset(&e.alias);
                        self.bump(|c| c.restarts += 1);
                        be.load(e).map_err(|second| {
                            format!(
                                "{} failed to load twice: {}; then {}",
                                e.alias, first.msg, second.msg
                            )
                        })?
                    }
                    Err(other) => return Err(format!("{} failed to load: {}", e.alias, other.msg)),
                };
                res.add(&e.alias, cost);
                // Measured above the estimate: make the book honest again.
                for a in res.over_budget(&e.alias) {
                    be.unload(&a);
                    res.remove(&a);
                    self.bump(|c| c.evictions += 1);
                }
                self.bump(|c| c.loads += 1);
                self.sync(res);
                Ok(true)
            }
        }
    }

    fn run(
        &self,
        job: Job,
        be: &mut dyn Backend,
        gate: &dyn Gate,
        blobs: &dyn BlobSource,
        res: &mut Residency,
    ) -> Outcome {
        let Job {
            entry,
            mut req,
            enq,
            deadline,
            reply,
            ..
        } = job;
        let started = Instant::now();
        if started >= deadline {
            reply(Err("expired in the queue".into()));
            return Outcome::Expired;
        }
        let queue_ms = ms(started - enq);
        // An image held by the broker is read now, not when queued: a queue of
        // waiting calls holds handles, not megabytes. A blob that cannot be
        // read fails the call before any model is loaded for it.
        if let Some(b) = req.image_blob.take() {
            match blobs.fetch(&b) {
                Ok(bytes) => req.image = Some((b.media_type, b64::encode(&bytes))),
                Err(m) => {
                    reply(Err(format!(
                        "image blob {}: {m}",
                        &b.hash[..12.min(b.hash.len())]
                    )));
                    return Outcome::Failed;
                }
            }
        }
        let mut loaded = match self.ensure(&entry, be, res) {
            Ok(l) => l,
            Err(m) => {
                reply(Err(m));
                return Outcome::Failed;
            }
        };
        let mut load_ms = ms(started.elapsed());
        if let Err(m) = gate.wait_idle(deadline) {
            reply(Err(m));
            return Outcome::Failed;
        }
        if Instant::now() >= deadline {
            reply(Err("expired while loading".into()));
            return Outcome::Expired;
        }
        let mut t = Instant::now();
        let mut out = be.infer(&entry, &req);
        if out.as_ref().is_err_and(|e| e.restartable()) {
            // The server died or lost its model: restart it and run again.
            be.reset(&entry.alias);
            res.remove(&entry.alias);
            self.bump(|c| c.restarts += 1);
            let reload = Instant::now();
            out = match self.ensure(&entry, be, res) {
                Ok(l) => {
                    loaded |= l;
                    load_ms += ms(reload.elapsed());
                    t = Instant::now();
                    be.infer(&entry, &req)
                }
                Err(m) => {
                    reply(Err(m));
                    return Outcome::Failed;
                }
            };
        }
        match out {
            Ok(o) => {
                res.touch(&entry.alias);
                reply(Ok(answer(
                    &entry,
                    &o,
                    queue_ms,
                    load_ms,
                    ms(t.elapsed()),
                    loaded,
                )));
                Outcome::Served
            }
            Err(e) => {
                reply(Err(e.msg));
                Outcome::Failed
            }
        }
    }
}

enum Outcome {
    Served,
    Failed,
    Expired,
}

fn ms(d: Duration) -> u64 {
    d.as_millis() as u64
}

fn answer(e: &Entry, o: &Output, queue_ms: u64, load_ms: u64, run_ms: u64, loaded: bool) -> Value {
    json!({
        "model": e.alias,
        "text": o.text,
        "finish": o.finish,
        "usage": {"prompt_tokens": o.prompt_tokens, "completion_tokens": o.completion_tokens},
        "queue_ms": queue_ms,
        "load_ms": if loaded { load_ms } else { 0 },
        "run_ms": run_ms,
        "loaded": loaded,
    })
}

#[cfg(test)]
mod tests;
