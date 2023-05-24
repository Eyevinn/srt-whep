use actix_web::{HttpResponse};
use actix_web::http::Error;

pub async fn no_content_return() -> Result<HttpResponse, Error> {

   return Ok(HttpResponse::NoContent().into());
}