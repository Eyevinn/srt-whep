use crate::errors::error_chain_fmt;
use std::fmt::Debug;
use thiserror::Error;
use timed_locks::Error as TimedLockError;

/// The `BranchControl` seam's error language. Three variants carry the
/// whole policy — is a retry worthwhile? — while the detail stays a
/// string; callers decide retry-vs-fail, never parse messages.
#[derive(Error)]
pub enum PipelineError {
    /// The input stream cannot accept a branch yet (not demuxed, output
    /// tees not built, or the pipeline is between supervisor restarts).
    /// Retryable.
    #[error("Pipeline is not ready")]
    NotReady,
    /// A failure expected to clear on its own — e.g. the state lock timed
    /// out behind a slow branch operation. Retryable.
    #[error("Transient pipeline failure: {0}")]
    Transient(String),
    /// A failure that will not clear without intervention: a missing
    /// element, a failed GStreamer operation.
    #[error("Pipeline operation failed: {0}")]
    Fatal(String),
}

impl Debug for PipelineError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

/// The 1s timed state lock timing out is the canonical transient failure:
/// whatever held the lock will release it.
impl From<TimedLockError> for PipelineError {
    fn from(e: TimedLockError) -> Self {
        PipelineError::Transient(e.to_string())
    }
}
