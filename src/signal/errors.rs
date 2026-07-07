use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SignalError {
    #[error("Invalid SDP: {0}")]
    InvalidSdp(String),
    #[error("Connection {0} not found")]
    NotFound(String),
    #[error("Connection {0} is in the wrong state for this operation")]
    WrongState(String),
    #[error("Timed out waiting for the {0}")]
    Timeout(&'static str),
    #[error("Input stream is not ready")]
    NotReady,
    #[error("Signaling coordinator is unavailable")]
    Unavailable,
    #[error("Pipeline operation failed: {0}")]
    Pipeline(String),
}

impl ResponseError for SignalError {
    fn status_code(&self) -> StatusCode {
        match self {
            SignalError::InvalidSdp(_) => StatusCode::BAD_REQUEST,
            SignalError::NotFound(_) => StatusCode::NOT_FOUND,
            SignalError::WrongState(_) => StatusCode::CONFLICT,
            SignalError::Timeout(_) | SignalError::NotReady => StatusCode::SERVICE_UNAVAILABLE,
            SignalError::Unavailable | SignalError::Pipeline(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    fn error_response(&self) -> HttpResponse {
        let mut builder = HttpResponse::build(self.status_code());
        if matches!(self, SignalError::Timeout(_) | SignalError::NotReady) {
            builder.append_header(("Retry-After", "3"));
        }
        builder.body(self.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::SignalError;
    use actix_web::http::StatusCode;
    use actix_web::ResponseError;

    #[test]
    fn status_codes_match_the_api_contract() {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            SignalError::InvalidSdp("x".into()).status_code()
        );
        assert_eq!(
            StatusCode::NOT_FOUND,
            SignalError::NotFound("x".into()).status_code()
        );
        assert_eq!(
            StatusCode::CONFLICT,
            SignalError::WrongState("x".into()).status_code()
        );
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE,
            SignalError::Timeout("SDP offer").status_code()
        );
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE,
            SignalError::NotReady.status_code()
        );
        assert_eq!(
            StatusCode::INTERNAL_SERVER_ERROR,
            SignalError::Unavailable.status_code()
        );
        assert_eq!(
            StatusCode::INTERNAL_SERVER_ERROR,
            SignalError::Pipeline("x".into()).status_code()
        );
    }

    #[test]
    fn retriable_errors_carry_retry_after() {
        let resp = SignalError::NotReady.error_response();
        assert_eq!("3", resp.headers().get("Retry-After").unwrap());

        let resp = SignalError::Timeout("SDP answer").error_response();
        assert_eq!("3", resp.headers().get("Retry-After").unwrap());

        let resp = SignalError::NotFound("x".into()).error_response();
        assert!(resp.headers().get("Retry-After").is_none());
    }
}
