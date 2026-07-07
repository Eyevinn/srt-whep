use crate::domain::SessionDescription;
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};
use uuid::Uuid;

#[tracing::instrument(name = "WHIP SINK", skip(form, signal))]
pub async fn whip_handler(
    form: String,
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let conn_id = path.into_inner();
    let sdp =
        SessionDescription::parse(form).map_err(|e| SignalError::InvalidSdp(e.to_string()))?;
    if !sdp.is_sendonly() {
        return Err(SignalError::InvalidSdp(
            "Received a recv-only SDP from whipsink; expected sendonly.".to_string(),
        ));
    }

    tracing::info!("Received SDP offer for connection {}", conn_id);
    let answer = signal.offer_received(conn_id.clone(), sdp).await?;

    let resource_id = Uuid::new_v4().to_string();
    Ok(HttpResponse::Created()
        .append_header((
            "Location",
            format!("/whip_sink/{}/{}", conn_id, resource_id),
        ))
        .content_type("application/sdp")
        .body(answer.as_ref().to_string()))
}
