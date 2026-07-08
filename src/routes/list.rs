use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};

#[tracing::instrument(name = "LIST", skip(signal))]
pub async fn list(signal: web::Data<SignalHandle>) -> Result<HttpResponse, SignalError> {
    let connections = signal.list_connections().await?;
    Ok(HttpResponse::Ok().json(connections))
}
