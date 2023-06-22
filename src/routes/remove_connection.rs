use crate::domain::*;
use crate::pipeline::SharablePipeline;

use actix_web::{web, HttpResponse};
use anyhow::Context;

#[allow(clippy::async_yields_async)]
pub async fn remove_connection(
    path: web::Path<String>,
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<SharablePipeline>,
) -> Result<HttpResponse, SubscribeError> {
    let id = path.into_inner();

    pipeline_state
        .remove_connection(id.clone())
        .context("Failed to remove connection from pipeline")?;

    app_state
        .remove_connection(id)
        .context("Failed to remove connection from app")?;

    Ok(HttpResponse::Ok().finish())
}
