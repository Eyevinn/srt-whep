use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};

/// DELETE is idempotent. Tearing down a live connection returns `200 OK`
/// (the WHEP/WHIP-spec termination confirmation); a connection that is
/// already gone — a client retry, or a session the coordinator already
/// reaped — is a no-op that returns `204 No Content` instead of `404`.
/// Retryable teardown failures (`503`) and fatal errors (`500`) are
/// propagated unchanged.
#[tracing::instrument(name = "REMOVE", skip(signal))]
pub async fn remove_connection(
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    match signal.remove_connection(id).await {
        Ok(()) => Ok(HttpResponse::Ok().finish()),
        Err(SignalError::NotFound(_)) => Ok(HttpResponse::NoContent().finish()),
        Err(e) => Err(e),
    }
}
