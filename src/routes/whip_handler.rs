use crate::domain::*;
use crate::stream::PipelineBase;
use actix_web::{web, HttpResponse};
use anyhow::Context;
use chrono::Utc;

#[allow(clippy::async_yields_async)]
#[tracing::instrument(
    name = "Receive the offer from GStreamer pipeline",
    skip(form, app_state, pipeline_state)
)]
pub async fn whip_handler<T: PipelineBase>(
    form: String,
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<T>,
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

    let sdp = app_state.wait_on_whep_offer(id.clone()).await;

    match sdp {
        Ok(sdp) => Ok(HttpResponse::Ok()
            .append_header(("Location", relative_url))
            .content_type("application/sdp")
            .body(sdp.as_ref().to_string())),
        Err(err) => {
            tracing::error!("Failed to receive a whep offer: {}", err);

            pipeline_state
                .remove_connection(id.clone())
                .await
                .context("Failed to remove client")?;

            app_state
                .remove_connection(id.clone())
                .await
                .context("Failed to remove connection")?;

            Err(SubscribeError::ValidationError(MyError::OfferMissing))
        }
    }
}
