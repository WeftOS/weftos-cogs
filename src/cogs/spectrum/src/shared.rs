//! State the HTTP server and the reporter read.

use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use serde_json::Value;

#[derive(Default)]
pub struct Shared {
    pub status: Mutex<Value>,
    /// The newest spectrum.psd.v1 reading as published.
    pub psd: Mutex<Value>,
    pub healthy: AtomicBool,
    pub stop: AtomicBool,
}
