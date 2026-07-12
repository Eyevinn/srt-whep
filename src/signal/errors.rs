use crate::domain::SdpError;
use crate::stream::PipelineError;
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SignalError {
    #[error("Invalid SDP: {0}")]
    InvalidSdp(String),
    // Parse failures from the domain carry the domain error as their source;
    // its Display is already "Invalid SDP: …", so this does not double-prefix.
    #[error(transparent)]
    Sdp(#[from] SdpError),
    #[error("Connection {0} not found")]
    NotFound(String),
    #[error("Connection {0} is in the wrong state for this operation")]
    WrongState(String),
    #[error("Timed out waiting for the {0}")]
    Timeout(&'static str),
    #[error("Input stream is not ready")]
    NotReady,
    #[error("Pipeline is busy: {0}")]
    PipelineBusy(String),
    #[error("Signaling coordinator is unavailable")]
    Unavailable,
    #[error("Pipeline operation failed: {0}")]
    Pipeline(String),
}

impl SignalError {
    /// The HTTP contract in one match: the status a failure gets, and the
    /// Retry-After to attach when a retry is worthwhile. One arm decides
    /// both for each variant, so the retryable set (503 + Retry-After) is
    /// spelled exactly once and the two can never drift.
    fn http_contract(&self) -> (StatusCode, Option<&'static str>) {
        match self {
            SignalError::InvalidSdp(_) | SignalError::Sdp(_) => (StatusCode::BAD_REQUEST, None),
            SignalError::NotFound(_) => (StatusCode::NOT_FOUND, None),
            SignalError::WrongState(_) => (StatusCode::CONFLICT, None),
            SignalError::Timeout(_) | SignalError::NotReady | SignalError::PipelineBusy(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, Some("3"))
            }
            SignalError::Unavailable | SignalError::Pipeline(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, None)
            }
        }
    }
}

impl ResponseError for SignalError {
    fn status_code(&self) -> StatusCode {
        self.http_contract().0
    }

    fn error_response(&self) -> HttpResponse {
        let (status, retry_after) = self.http_contract();
        let mut builder = HttpResponse::build(status);
        if let Some(seconds) = retry_after {
            builder.append_header(("Retry-After", seconds));
        }
        builder.body(self.to_string())
    }
}

/// The stream→signal seam: retry policy survives the crossing instead of
/// flattening into an opaque 500.
impl From<PipelineError> for SignalError {
    fn from(e: PipelineError) -> Self {
        match e {
            PipelineError::NotReady => SignalError::NotReady,
            PipelineError::Transient(msg) => SignalError::PipelineBusy(msg),
            PipelineError::Fatal(msg) => SignalError::Pipeline(msg),
        }
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

    #[test]
    fn pipeline_errors_map_to_retryable_or_fatal_statuses() {
        use crate::stream::PipelineError;

        let not_ready = SignalError::from(PipelineError::NotReady);
        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, not_ready.status_code());
        assert_eq!(
            "3",
            not_ready
                .error_response()
                .headers()
                .get("Retry-After")
                .unwrap()
        );

        // Retryable failures keep their retry semantics end-to-end instead
        // of collapsing into an opaque 500 — and keep their detail.
        let transient = SignalError::from(PipelineError::Transient("lock timed out".into()));
        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, transient.status_code());
        assert_eq!(
            "3",
            transient
                .error_response()
                .headers()
                .get("Retry-After")
                .unwrap()
        );
        assert!(transient.to_string().contains("lock timed out"));

        let fatal = SignalError::from(PipelineError::Fatal("demux vanished".into()));
        assert_eq!(StatusCode::INTERNAL_SERVER_ERROR, fatal.status_code());
        assert!(fatal
            .error_response()
            .headers()
            .get("Retry-After")
            .is_none());
        assert!(fatal.to_string().contains("demux vanished"));
    }

    #[test]
    fn sdp_parse_errors_map_without_double_prefix() {
        use crate::domain::SdpError;

        let err = SignalError::from(SdpError::InvalidSdp("v=1 unsupported".into()));
        assert_eq!(StatusCode::BAD_REQUEST, err.status_code());
        assert_eq!("Invalid SDP: v=1 unsupported", err.to_string());
    }
}
