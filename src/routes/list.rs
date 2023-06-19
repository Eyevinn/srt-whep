use actix_web::{HttpResponse, web};
use crate::domain::SharableAppState;

#[allow(clippy::async_yields_async)]
pub async fn list(app_state: web::Data<SharableAppState>) -> HttpResponse {
  match app_state.list_connections() {
    Ok(list) => {HttpResponse::Ok().body(serde_json::to_string(&list).unwrap())},
    Err(error) => {HttpResponse::InternalServerError().body(error.to_string())}
  }
}