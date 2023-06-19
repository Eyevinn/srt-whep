use actix_web::ResponseError;
use actix_web::http::StatusCode;

#[derive(thiserror::Error)]
pub enum MyError {
    #[error("Invalid SDP")]
    InvalidSDP,
    #[error("Repeated WHIP offer exists")]
    RepeatedWhipOffer,
    #[error("Repeated WHEP offer exists")]
    RepeatedWhepError,
    #[error("Repeated resource id")]
    RepeatedResourceIdError,
    #[error("Resource not found")]
    ResourceNotFound,
}

// We are still using a bespoke implementation of `Debug` // to get a nice report using the error source chain
impl std::fmt::Debug for MyError {
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


#[derive(thiserror::Error)]
pub enum SubscribeError {
    #[error("{0}")]
    ValidationError(String),
    #[error(transparent)]
    UnexpectedError(#[from] anyhow::Error),
}

impl std::fmt::Debug for SubscribeError {
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
