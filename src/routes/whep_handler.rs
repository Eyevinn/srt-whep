use crate::domain::*;
use crate::stream::SharablePipeline;
use actix_web::{web, HttpResponse};
use anyhow::Context;
use chrono::Utc;
use uuid::Uuid;

#[allow(clippy::async_yields_async)]
#[tracing::instrument(
    name = "Receive an offer from a client",
    skip(form, app_state, pipeline_state)
)]
pub async fn subscribe(
    form: String,
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<SharablePipeline>,
) -> Result<HttpResponse, SubscribeError> {
    if !form.is_empty() {
        return Err(SubscribeError::ValidationError(MyError::InvalidSDP(
            "Client initialization not supported.".to_string(),
        )));
    }

    let connection_id = Uuid::new_v4().to_string();
    tracing::info!(
        "Create connection {} at time: {:?}",
        connection_id.clone(),
        Utc::now()
    );

    pipeline_state
        .add_client(connection_id.clone())
        .context("Failed to add client")?;

    app_state
        .add_resource(connection_id.clone())
        .context("Failed to add resource")?;

    let sdp = app_state
        .wait_on_whip_offer(connection_id.clone())
        .await
        .context("Failed to receive a whip offer")?;

    let url = format!("/channel/{}", connection_id);
    tracing::info!("Receiving streaming from: {}", url);

    Ok(HttpResponse::Created()
        .append_header(("Location", url))
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

    app_state
        .save_whep_offer(sdp, id)
        .context("Failed to save whep offer")?;

    Ok(HttpResponse::NoContent().into())
}
