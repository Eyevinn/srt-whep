use crate::domain::*;
use crate::stream::PipelineBase;
use actix_web::{web, HttpResponse};
use anyhow::Context;

#[allow(clippy::async_yields_async)]
pub async fn list<T: PipelineBase>(
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<T>,
) -> Result<HttpResponse, SubscribeError> {
    // print pipeline for debugging
    pipeline_state
        .print()
        .await
        .context("Failed to print pipeline")?;

    match app_state.list_connections().await {
        Ok(list) => Ok(HttpResponse::Ok().body(serde_json::to_string(&list).unwrap())),
        Err(error) => Err(SubscribeError::UnexpectedError(error.into())),
    }
}
