use crate::domain::*;
use crate::stream::PipelineBase;
use actix_web::{web, HttpResponse};
use anyhow::Context;
use chrono::Utc;

#[allow(clippy::async_yields_async)]
#[tracing::instrument(
    name = "WHIP SINK",
    skip(form, app_state, pipeline_state)
)]
pub async fn whip_handler<T: PipelineBase>(
    form: String,
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<T>,
) -> Result<HttpResponse, SubscribeError> {
    tracing::info!("Received SDP offer at time: {:?}", Utc::now());
    let sdp_offer: SessionDescription = form.try_into().map_err(SubscribeError::ValidationError)?;
    if !sdp_offer.is_sendonly() {
        return Err(SubscribeError::ValidationError(MyError::InvalidSDP(
            "Received a recv-only SDP from whipsink, ignoring it.".to_string(),
        )));
    }

    let id = app_state
        .save_whip_offer(sdp_offer)
        .await
        .context("Failed to save WHIP SDP offer")?;

    let sdp_answer = app_state.wait_on_whep_answer(id.clone()).await;

    match sdp_answer {
        Ok(sdp_answer) => {
            tracing::info!("Recevied WHEP SDP answer from client to be used as WHIP SDP answer");

            let relative_url = format!("/whip_sink/{}", id);
            tracing::info!("WHIP connection resource: {}", relative_url);
        
            Ok(HttpResponse::Created() 
                .append_header(("Location", relative_url))
                .content_type("application/sdp")
                .body(sdp_answer.as_ref().to_string()))
        }
        Err(err) => {
            tracing::error!("No WHEP SDP answer received from client: {}", err);

            pipeline_state
                .remove_connection(id.clone())
                .await
                .context("Failed to remove client")?;

            app_state
                .remove_connection(id.clone())
                .await
                .context("Failed to remove connection")?;

            Err(SubscribeError::ValidationError(MyError::AnswerMissing))
        }
    }
}
