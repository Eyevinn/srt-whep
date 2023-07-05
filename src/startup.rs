use crate::domain::SharableAppState;
use crate::routes::*;
use crate::stream::PipelineBase;
use actix_cors::Cors;
use actix_web::dev::Server;
use actix_web::{web, App, HttpServer};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

pub fn run<T: PipelineBase + 'static>(
    listener: TcpListener,
    app_state: SharableAppState,
    pipeline_state: T,
) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .route("/health_check", web::get().to(health_check))
            .route("/list", web::get().to(list))
            .route("/channel", web::post().to(whep_handler::<T>))
            .route("/channel/{id}", web::patch().to(patch_handler))
            .route("/channel/{id}", web::delete().to(remove_connection::<T>))
            .route("/whip_sink", web::post().to(whip_handler::<T>))
            .app_data(web::Data::new(app_state.clone()))
            .app_data(web::Data::new(pipeline_state.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
