use crate::domain::*;
use crate::stream::PipelineBase;
use actix_web::{web, HttpResponse};
use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

#[allow(clippy::async_yields_async)]
#[tracing::instrument(name = "WHIP SINK", skip(form, app_state, pipeline_state))]
pub async fn whip_handler<T: PipelineBase>(
    form: String,
    path: web::Path<String>,
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<T>,
) -> Result<HttpResponse, SubscribeError> {
    let conn_id = path.into_inner();
    if conn_id.is_empty() {
        return Err(SubscribeError::ValidationError(MyError::EmptyConnection));
    }
    let sdp_offer: SessionDescription = form.try_into().map_err(SubscribeError::ValidationError)?;
    if !sdp_offer.is_sendonly() {
        return Err(SubscribeError::ValidationError(MyError::InvalidSDP(
            "Received a recv-only SDP from whipsink, ignoring it.".to_string(),
        )));
    }

    tracing::info!(
        "Received SDP offer for connection {} at time: {:?}",
        conn_id,
        Utc::now()
    );
    let resource_id = Uuid::new_v4().to_string();

    app_state
        .save_whip_offer(conn_id.clone(), sdp_offer)
        .await
        .context("Failed to save WHIP SDP offer")?;

    let sdp_answer = app_state.wait_on_whep_answer(conn_id.clone()).await;

    match sdp_answer {
        Ok(sdp_answer) => {
            tracing::info!("Recevied WHEP SDP answer from client to be used as WHIP SDP answer");

            let relative_url = format!("/whip_sink/{}/{}", conn_id, resource_id);
            tracing::info!("WHIP connection resource: {}", relative_url);

            Ok(HttpResponse::Created()
                .append_header(("Location", relative_url))
                .content_type("application/sdp")
                .body(sdp_answer.as_ref().to_string()))
        }
        Err(err) => {
            tracing::error!("No WHEP SDP answer received from client: {}", err);

            // Reset pipeline and app state if SDP answer is not received
            pipeline_state
                .quit()
                .await
                .context("Failed to stop pipeline")?;

            app_state
                .reset()
                .await
                .context("Failed to reset app state")?;

            Err(SubscribeError::ValidationError(MyError::AnswerMissing))
        }
    }
}
