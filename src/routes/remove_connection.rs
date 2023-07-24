use crate::domain::*;
use crate::stream::PipelineBase;

use actix_web::{web, HttpResponse};
use anyhow::Context;

#[allow(clippy::async_yields_async)]
pub async fn remove_connection<T: PipelineBase>(
    path: web::Path<String>,
    app_state: web::Data<SharableAppState>,
    pipeline_state: web::Data<T>,
) -> Result<HttpResponse, SubscribeError> {
    let id = path.into_inner();

    // Check if connection exists in app state
    if !app_state
        .has_connection(id.clone())
        .await
        .context("Failed to check connection in app")?
    {
        return Err(SubscribeError::ValidationError(MyError::ResourceNotFound));
    }

    pipeline_state
        .remove_connection(id.clone())
        .await
        .context("Failed to remove connection from pipeline")?;

    app_state
        .remove_connection(id)
        .await
        .context("Failed to remove connection from app")?;

    Ok(HttpResponse::Ok().finish())
}
