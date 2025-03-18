use crate::domain::*;
use crate::stream::PipelineBase;
use actix_web::{web, HttpResponse};
use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

#[allow(clippy::async_yields_async)]
#[tracing::instrument(name = "WHEP", skip(form, app_state, pipeline_state))]
pub async fn whep_handler<T: PipelineBase>(
    form: String,
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<T>,
) -> Result<HttpResponse, SubscribeError> {
    if !form.is_empty() {
        tracing::debug!("WHIP Offer: \n {}", form);
        return Err(SubscribeError::ValidationError(MyError::InvalidSDP(
            "Empty body expected. Client initialization not supported.".to_string(),
        )));
    }

    // Check if input stream is available
    let ready = pipeline_state
        .ready()
        .await
        .context("Failed to check pipeline state")?;
    if !ready {
        return Err(SubscribeError::MissingInputStream);
    }

    let id = Uuid::new_v4().to_string();
    tracing::info!("Create connection {} at time: {:?}", id.clone(), Utc::now());

    pipeline_state
        .add_connection(id.clone())
        .await
        .context("Failed to add connection to pipeline")?;

    app_state
        .add_connection(id.clone())
        .await
        .context("Failed to add connection to app state")?;

    tracing::debug!("Waiting for SDP offer from WHIP sink");
    let sdp_offer = app_state.wait_on_whip_offer(id.clone()).await;

    match sdp_offer {
        Ok(sdp_offer) => {
            let relative_url = format!("/channel/{}", id);
            tracing::info!("WHEP connection resource: {}", relative_url);

            Ok(HttpResponse::Created()
                .append_header(("Location", relative_url))
                .content_type("application/sdp")
                .body(sdp_offer.as_ref().to_string()))
        }
        Err(err) => {
            tracing::error!("Failed to receive SDP offer from WHIP sink: {}", err);

            // Reset pipeline and app state if SDP offer is not received
            pipeline_state
                .quit()
                .await
                .context("Failed to stop pipeline")?;

            app_state
                .reset()
                .await
                .context("Failed to reset app state")?;

            Err(SubscribeError::ValidationError(MyError::OfferMissing))
        }
    }
}

#[allow(clippy::async_yields_async)]
#[tracing::instrument(name = "WHEP PATCH", skip(form, app_state))]
pub async fn whep_patch_handler(
    form: String,
    path: web::Path<String>,
    app_state: web::Data<SharableAppState>,
) -> Result<HttpResponse, SubscribeError> {
    let id = path.into_inner();
    if id.is_empty() {
        return Err(SubscribeError::ValidationError(MyError::EmptyConnection));
    }
    let sdp_answer: SessionDescription =
        form.try_into().map_err(SubscribeError::ValidationError)?;
    if sdp_answer.is_sendonly() {
        return Err(SubscribeError::ValidationError(MyError::InvalidSDP(
            "Received a send-only SDP from client, ignoring it.".to_string(),
        )));
    }

    // TODO: check content type for trickle-ice and return not supported

    tracing::debug!("Saving WHEP SDP answer to app");
    app_state
        .save_whep_answer(id, sdp_answer)
        .await
        .context("Failed to save WHEP SDP answer")?;

    Ok(HttpResponse::NoContent().into())
}
