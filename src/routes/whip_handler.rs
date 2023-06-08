
use uuid::Uuid;
use crate::domain::*;
use actix_web::{web, HttpResponse,};
use anyhow::Context;
use chrono::Utc;
use std::convert::TryInto;

#[allow(clippy::async_yields_async)]
#[tracing::instrument(name = "Receive an offer from a server", skip(form, app_state))]
pub async fn whip_request(
    form: String,
    app_state: web::Data<SharableAppState>,
) -> Result<HttpResponse, SubscribeError> {
    tracing::info!("Received SDP at time: {:?}", Utc::now());
    let sdp: SessionDescription = form.try_into().map_err(SubscribeError::ValidationError)?;
    if sdp.is_sendonly() {
        let resource_id = Uuid::new_v4().to_string();
        app_state
            .save_whip_offer(sdp, Some(resource_id.clone()))
            .await
            .context("Failed to save whip offer")?;

        let whip_answer = app_state
            .wait_on_whep_offer(resource_id.clone())
            .await
            .context("Failed to receive a whep offer")?;
        return Ok(HttpResponse::Ok()
            .append_header((
                "Location",
                format!("http://0.0.0.0:8000/channel/{}", resource_id),
            ))
            .content_type("application/sdp")
            .body(whip_answer.as_ref().to_string()));
    } else {
        return Ok(HttpResponse::BadRequest().into());
    }
}