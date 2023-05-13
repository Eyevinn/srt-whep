use crate::domain::*;
use actix_web::http::StatusCode;
use actix_web::{web, HttpResponse, ResponseError};
use anyhow::Context;
use chrono::Utc;
use std::convert::{TryFrom, TryInto};

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
#[tracing::instrument(name = "Receive an offer from a client", skip(form, app_state))]
pub async fn subscribe(
    form: String,
    app_state: web::Data<SharableAppState>,
) -> Result<HttpResponse, SubscribeError> {
    let sdp: SessionDescription = form.try_into().map_err(SubscribeError::ValidationError)?;
    tracing::info!("Received SDP at time: {:?}", Utc::now());

    if sdp.is_sendonly() {
        app_state
            .save_whip_offer(sdp)
            .await
            .context("Failed to save whip offer")?;

        let resouce_id = app_state
            .create_resource()
            .await
            .context("Failed to create resource")?;

        let whip_answer = app_state
            .wait_on_whep_offer()
            .await
            .context("Failed to receive a whep offer")?;

        return Ok(HttpResponse::Ok()
            .append_header(("Location", format!("/resources/{}", resouce_id)))
            .content_type("application/sdp")
            .body(whip_answer.as_ref().to_string()));
    } else {
        app_state
            .save_whep_offer(sdp)
            .await
            .context("Failed to save whep offer")?;

        let whep_answer = app_state
            .wait_on_whip_offer()
            .await
            .context("Failed to receive a whip offer")?;

        let resouce_id = app_state
            .get_resource()
            .await
            .context("Failed to find resource")?;

        return Ok(HttpResponse::Created()
            .append_header(("Location", format!("/resources/{}", resouce_id)))
            .content_type("application/sdp")
            .body(whep_answer.as_ref().to_string()));
    }
}
