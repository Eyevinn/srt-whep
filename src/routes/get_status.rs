use actix_web::{HttpResponse, http::header::ContentType};
use serde::{Deserialize, Serialize};
use actix_web::http::Error;

#[derive(Serialize, Deserialize)]
struct StatusResponse {
    channel_id: String,
    viewers: i64,
}

pub async fn get_status(_form: String) -> Result<HttpResponse, Error> {

    let response = { StatusResponse{
        channel_id: "1".to_string(),
        viewers: 2
    }};
    let body = serde_json::to_string(&response).unwrap();

    return Ok(HttpResponse::Ok().content_type(ContentType::json()).body(body));
}