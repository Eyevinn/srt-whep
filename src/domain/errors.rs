use actix_web::http::StatusCode;
use actix_web::ResponseError;
use std::fmt::Debug;
use thiserror::Error;
use timed_locks::Error as TimedLockError;

#[derive(Error)]
pub enum MyError {
    #[error("Invalid SDP: {0}")]
    InvalidSDP(String),
    #[error("Repeated resource id: {0}")]
    RepeatedResourceIdError(String),
    #[error("Resource not found")]
    ResourceNotFound,
    #[error("Lock is timeout")]
    LockTimeout(#[from] TimedLockError),
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

#[derive(Error)]
pub enum SubscribeError {
    #[error("Failed to process request: {0}")]
    ValidationError(MyError),
    #[error(transparent)]
    UnexpectedError(#[from] anyhow::Error),
}

impl Debug for SubscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

impl ResponseError for SubscribeError {
    fn status_code(&self) -> StatusCode {
        match self {
            SubscribeError::ValidationError(_) => StatusCode::BAD_REQUEST,
            SubscribeError::UnexpectedError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
