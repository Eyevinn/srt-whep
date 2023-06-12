use actix_web::{HttpResponse, web};
use crate::{domain::SharableAppState, pipeline::SharablePipeline};

#[allow(clippy::async_yields_async)]
pub async fn remove_connection(
  path: web::Path<String>,
  app_state: web::Data<SharableAppState>,
  pipeline_state: web::Data<SharablePipeline>) -> HttpResponse {

    let id = path.into_inner();
    match pipeline_state.remove_connection(id.clone()) {
      Ok(()) => {},
      Err(error) =>  return HttpResponse::InternalServerError().body(error.to_string()),
    }
    match app_state.remove_connection(id.clone()).await {
      Ok(()) => {},
      Err(error) =>  return HttpResponse::InternalServerError().body(error.to_string()),
     
    };
    
    

    HttpResponse::Ok().finish()
}