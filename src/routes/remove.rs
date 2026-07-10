use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};

/// The shared idempotent-DELETE mapping (single source of the PR #111 policy):
/// `Ok(())` → 200 (terminated a live session), `NotFound` → 204 (already gone,
/// a no-op), and every other error propagates unchanged (503 retryable / 500
/// fatal). Both DELETE routes go through here so the contract cannot diverge.
async fn delete(id: String, signal: &SignalHandle) -> Result<HttpResponse, SignalError> {
    match signal.remove_connection(id).await {
        Ok(()) => Ok(HttpResponse::Ok().finish()),
        Err(SignalError::NotFound(_)) => Ok(HttpResponse::NoContent().finish()),
        Err(e) => Err(e),
    }
}

/// A WHEP viewer terminating its playback session (`DELETE /channel/{id}`).
#[tracing::instrument(name = "WHEP DELETE", skip(signal))]
pub async fn terminate_session(
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    tracing::info!("WHEP client terminating session {}", id);
    delete(id, &signal).await
}

/// The internal loopback whipclientsink tearing down its leg
/// (`DELETE /whip_sink/{id}`).
#[tracing::instrument(name = "WHIP SINK DELETE", skip(signal))]
pub async fn remove_whip_sink(
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    tracing::info!("Removing WHIP sink for connection {}", id);
    delete(id, &signal).await
}
