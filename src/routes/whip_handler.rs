use crate::domain::SessionDescription;
use crate::signal::{SignalError, SignalHandle};
use crate::stream::whip_sink_path;
use actix_web::{web, HttpResponse};

#[tracing::instrument(name = "WHIP SINK", skip(form, signal))]
pub async fn whip_handler(
    form: String,
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let conn_id = path.into_inner();
    let sdp = SessionDescription::parse(form).map_err(SignalError::from)?;
    if !sdp.is_sendonly() {
        return Err(SignalError::InvalidSdp(
            "Received a recv-only SDP from whipsink; expected sendonly.".to_string(),
        ));
    }

    tracing::info!("Received SDP offer for connection {}", conn_id);
    let answer = signal.offer_received(conn_id.clone(), sdp).await?;

    Ok(HttpResponse::Created()
        .append_header(("Location", whip_sink_path(&conn_id)))
        .content_type("application/sdp")
        .body(answer.as_ref().to_string()))
}
