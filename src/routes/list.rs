use crate::domain::*;
use actix_web::{web, HttpResponse};

#[allow(clippy::async_yields_async)]
pub async fn list(app_state: web::Data<SharableAppState>) -> Result<HttpResponse, SubscribeError> {
    match app_state.list_connections().await {
        Ok(list) => Ok(HttpResponse::Ok().body(serde_json::to_string(&list).unwrap())),
        Err(error) => Err(SubscribeError::UnexpectedError(error.into())),
    }
}
