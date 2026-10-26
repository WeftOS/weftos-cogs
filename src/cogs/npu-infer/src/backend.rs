//! What the scheduler needs from the thing that runs models. The real one is
//! [`crate::geniex::GeniexBackend`]; the tests use a fake.

use crate::catalog::Entry;

/// One generation request, already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub system: Option<String>,
    pub text: Option<String>,
    /// `(mime, base64)`.
    pub image: Option<(String, String)>,
    /// An image held by the broker as a blob (ADR-122 section 13). The worker
    /// fetches it just before the call runs and turns it into `image`.
    pub image_blob: Option<BlobRef>,
    pub max_tokens: u32,
}

/// A `blob.v1` handle's essentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobRef {
    pub hash: String,
    pub size: u64,
    pub media_type: String,
}

/// Where blob bytes come from (the broker).
pub trait BlobSource: Send {
    /// The bytes of `b`, `Err` with the reason when they cannot be read.
    fn fetch(&self, b: &BlobRef) -> Result<Vec<u8>, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Output {
    pub text: String,
    pub finish: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// How a call failed, which decides what the scheduler does about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The serving process is gone or unreachable. Restart and retry once.
    Dead,
    /// GenieX could not load the model. Restart and retry once.
    LoadFailed,
    /// The model ran and failed (bad input, context too long). Report it.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendError {
    pub fault: Fault,
    pub msg: String,
}

impl BackendError {
    pub fn new(fault: Fault, msg: impl Into<String>) -> Self {
        Self {
            fault,
            msg: msg.into(),
        }
    }

    pub fn restartable(&self) -> bool {
        matches!(self.fault, Fault::Dead | Fault::LoadFailed)
    }
}

pub trait Backend: Send {
    /// Load `e` and return what it costs in memory, MiB, as measured.
    fn load(&mut self, e: &Entry) -> Result<u64, BackendError>;
    /// Free `alias`. Missing is not an error.
    fn unload(&mut self, alias: &str);
    fn infer(&mut self, e: &Entry, req: &Request) -> Result<Output, BackendError>;
    /// Throw away whatever serves `alias` (a hung or half-loaded process) so
    /// the next `load` starts a new one through the wrapper.
    fn reset(&mut self, alias: &str);
    /// System `MemAvailable`, MiB.
    fn mem_available_mib(&self) -> Option<u64>;
}

/// Something else that uses the NPU and has to be waited for.
pub trait Gate: Send {
    /// Return once the other user is idle, or `Err` at `deadline`.
    fn wait_idle(&self, deadline: std::time::Instant) -> Result<(), String>;
    /// Is it busy right now (for the status report)? `None`: not running.
    fn busy(&self) -> Option<bool>;
}
