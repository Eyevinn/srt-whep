use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};

#[tracing::instrument(name = "REMOVE", skip(signal))]
pub async fn remove_connection(
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    signal.remove_connection(id).await?;
    Ok(HttpResponse::Ok().finish())
}
