use crate::domain::*;
use actix_web::{web, HttpResponse};
use anyhow::Context;
use chrono::Utc;

#[allow(clippy::async_yields_async)]
#[tracing::instrument(
    name = "Receive the offer from GStreamer pipeline",
    skip(form, app_state)
)]
pub async fn whip_request(
    form: String,
    app_state: web::Data<SharableAppState>,
) -> Result<HttpResponse, SubscribeError> {
    tracing::info!("Received SDP at time: {:?}", Utc::now());
    let sdp: SessionDescription = form.try_into().map_err(SubscribeError::ValidationError)?;
    if !sdp.is_sendonly() {
        return Err(SubscribeError::ValidationError(MyError::InvalidSDP(
            "Received a recv-only SDP from whipsink, ignoring it.".to_string(),
        )));
    }

    let id = app_state
        .save_whip_offer(sdp)
        .await
        .context("Failed to save whip offer")?;

    let relative_url = format!("/channel/{}", id);
    tracing::info!("Prepare streaming at: {}", relative_url);

    let whip_answer = app_state
        .wait_on_whep_offer(id.clone())
        .await
        .context("Failed to find a whep offer")?;

    Ok(HttpResponse::Ok()
        .append_header(("Location", relative_url))
        .content_type("application/sdp")
        .body(whip_answer.as_ref().to_string()))
}
