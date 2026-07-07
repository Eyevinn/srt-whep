use crate::domain::SessionDescription;
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};
use uuid::Uuid;

#[tracing::instrument(name = "WHEP", skip(form, signal))]
pub async fn whep_handler(
    form: String,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    if !form.is_empty() {
        return Err(SignalError::InvalidSdp(
            "Empty body expected. Client initialization not supported.".to_string(),
        ));
    }

    let id = Uuid::new_v4().to_string();
    tracing::info!("Creating connection {}", id);

    let offer = signal.create_connection(id.clone()).await?;

    Ok(HttpResponse::Created()
        .append_header(("Location", format!("/channel/{}", id)))
        .content_type("application/sdp")
        .body(offer.as_ref().to_string()))
}

#[tracing::instrument(name = "WHEP PATCH", skip(form, signal))]
pub async fn whep_patch_handler(
    form: String,
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    let sdp = SessionDescription::parse(form).map_err(SignalError::from)?;
    if sdp.is_sendonly() {
        return Err(SignalError::InvalidSdp(
            "Received a send-only SDP from client; expected recvonly.".to_string(),
        ));
    }

    signal.answer_received(id, sdp).await?;

    Ok(HttpResponse::NoContent().finish())
}
