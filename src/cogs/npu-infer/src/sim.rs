//! A backend that runs no model, for `--once --simulate` and the smoke test.

use crate::backend::{Backend, BackendError, Output, Request};
use crate::catalog::Entry;

#[derive(Default)]
pub struct Sim;

impl Backend for Sim {
    fn load(&mut self, e: &Entry) -> Result<u64, BackendError> {
        Ok(e.est_mib)
    }

    fn unload(&mut self, _: &str) {}

    fn infer(&mut self, e: &Entry, req: &Request) -> Result<Output, BackendError> {
        Ok(Output {
            text: format!(
                "[SYNTHETIC {}] {}",
                e.alias,
                req.text.clone().unwrap_or_default()
            ),
            finish: "stop".into(),
            prompt_tokens: 1,
            completion_tokens: 1,
        })
    }

    fn reset(&mut self, _: &str) {}

    fn mem_available_mib(&self) -> Option<u64> {
        None
    }
}
