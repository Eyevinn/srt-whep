use crate::domain::*;
use crate::pipeline::setup_pipeline;
use actix_web::http::StatusCode;
use actix_web::{web, HttpResponse, ResponseError};
use anyhow::Context;
use chrono::Utc;
use std::convert::{TryFrom, TryInto};
use uuid::Uuid;
use tokio::task;

impl TryFrom<String> for SessionDescription {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let sdp = SessionDescription::parse(value)?;
        Ok(sdp)
    }
}

#[derive(thiserror::Error)]
pub enum SubscribeError {
    #[error("{0}")]
    ValidationError(String),
    #[error(transparent)]
    UnexpectedError(#[from] anyhow::Error),
}

impl std::fmt::Debug for SubscribeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

impl ResponseError for SubscribeError {
    fn status_code(&self) -> StatusCode {
        match self {
            SubscribeError::ValidationError(_) => StatusCode::BAD_REQUEST,
            SubscribeError::UnexpectedError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[allow(clippy::async_yields_async)]
#[tracing::instrument(name = "Receive an offer from a server", skip(form, app_state))]
pub async fn srt_request(
    form: String,
    app_state: web::Data<SharableAppState>,
) -> Result<HttpResponse, SubscribeError> {
    tracing::info!("Received SDP at time: {:?}", Utc::now());
    let sdp: SessionDescription = form.try_into().map_err(SubscribeError::ValidationError)?;
    if sdp.is_sendonly() {
        println!("whip inc");
        let resource_id = Uuid::new_v4().to_string();
        app_state
        .save_whip_offer(sdp, Some(resource_id.clone()))
            .await
            .context("Failed to save whip offer")?;

        let whip_answer = app_state
            .wait_on_whep_offer(resource_id.clone())
            .await
            .context("Failed to receive a whep offer")?;
        println!("sending whep");
        return Ok(HttpResponse::Ok()
        .append_header(("Location", format!("http://0.0.0.0:8000/channel/{}", resource_id)))
        .content_type("application/sdp")
        .body(whip_answer.as_ref().to_string()));
    } else {
        return Ok(HttpResponse::BadRequest().into());
    }
}

#[allow(clippy::async_yields_async)]
#[tracing::instrument(name = "Receive an offer from a client", skip(form, app_state))]
pub async fn subscribe(
    form: String,
    app_state: web::Data<SharableAppState>,
) -> Result<HttpResponse, SubscribeError> {
    tracing::info!("Received SDP at time: {:?}", Utc::now());

    if form.is_empty() {
        println!("bad req");
        let args = app_state.get_args().await.context("Failed to find resource")?;
        println!("why1");
        let t = task::spawn(async move {
            println!("why");
            setup_pipeline(&args.clone()).unwrap();
        });

        println!("working");

        let result = app_state
            .wait_on_whip_offer()
            .await
            .context("Failed to receive a whip offer")?;

        return Ok(HttpResponse::Created()
            .append_header((
                "Location",
                format!("http://0.0.0.0:8000/channel/{}", result.resource_id),
            ))
            .content_type("application/sdp")
            .body(result.sdp.as_ref().to_string()));
    }
    
    return Ok(HttpResponse::BadRequest().into());
}

#[allow(clippy::async_yields_async)]
//#[tracing::instrument(name = "Receive an answer from a client", skip(form, app_state))]
pub async fn patch(
    form: String, path: web::Path<String>,
    app_state: web::Data<SharableAppState>,
) -> Result<HttpResponse, SubscribeError> {
    let sdp: SessionDescription = form.try_into().map_err(SubscribeError::ValidationError)?;
    let id = path.into_inner();

    app_state
        .save_whep_offer(sdp, Some(id))
        .await
        .context("Failed to save whep offer")?;

    return Ok(HttpResponse::NoContent().into());
}
