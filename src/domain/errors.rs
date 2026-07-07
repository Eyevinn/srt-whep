use std::fmt::Debug;
use thiserror::Error;
use timed_locks::Error as TimedLockError;

#[derive(Error)]
pub enum MyError {
    #[error("Invalid SDP: {0}")]
    InvalidSDP(String),
    #[error("Lock is timeout")]
    LockTimeout(#[from] TimedLockError),
    #[error("Failed to find element: {0}")]
    MissingElement(String),
    #[error("Failed Operation: {0}")]
    FailedOperation(String),
}

// We are still using a bespoke implementation of `Debug` to get a nice report using the error source chain
impl Debug for MyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

pub fn error_chain_fmt(
    e: &impl std::error::Error,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    writeln!(f, "{}\n", e)?;
    let mut current = e.source();
    while let Some(cause) = current {
        writeln!(f, "Caused by:\n\t{}", cause)?;
        current = cause.source();
    }
    Ok(())
}
