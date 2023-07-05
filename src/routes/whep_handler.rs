use crate::domain::*;
use crate::stream::PipelineBase;
use actix_web::{web, HttpResponse};
use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

#[allow(clippy::async_yields_async)]
#[tracing::instrument(
    name = "Receive an offer from a client",
    skip(form, app_state, pipeline_state)
)]
pub async fn subscribe<T: PipelineBase>(
    form: String,
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<T>,
) -> Result<HttpResponse, SubscribeError> {
    if !form.is_empty() {
        return Err(SubscribeError::ValidationError(MyError::InvalidSDP(
            "Client initialization not supported.".to_string(),
        )));
    }

    let id = Uuid::new_v4().to_string();
    tracing::info!("Create connection {} at time: {:?}", id.clone(), Utc::now());

    tracing::debug!("Adding client to pipeline");
    pipeline_state
        .add_client(id.clone())
        .context("Failed to add client")?;

    tracing::debug!("Adding connection to app");
    app_state
        .add_connection(id.clone())
        .await
        .context("Failed to add connection")?;

    tracing::debug!("Waiting for a whip offer");
    let sdp = app_state
        .wait_on_whip_offer(id.clone())
        .await
        .context("Failed to receive a whip offer")?;

    let relative_url = format!("/channel/{}", id);
    tracing::info!("Receiving streaming from: {}", relative_url);

    Ok(HttpResponse::Created()
        .append_header(("Location", relative_url))
        .content_type("application/sdp")
        .body(sdp.as_ref().to_string()))
}

#[allow(clippy::async_yields_async)]
#[tracing::instrument(name = "Receive an answer from a client", skip(form, app_state))]
pub async fn patch(
    form: String,
    path: web::Path<String>,
    app_state: web::Data<SharableAppState>,
) -> Result<HttpResponse, SubscribeError> {
    let sdp: SessionDescription = form.try_into().map_err(SubscribeError::ValidationError)?;
    let id = path.into_inner();
    if id.is_empty() {
        return Err(SubscribeError::ValidationError(MyError::ResourceNotFound));
    }

    tracing::debug!("Saving whep offer to app");
    app_state
        .save_whep_offer(sdp, id)
        .await
        .context("Failed to save whep offer")?;

    Ok(HttpResponse::NoContent().into())
}
