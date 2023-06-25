use crate::domain::SharableAppState;
use crate::routes::{health_check, list, patch, remove_connection, subscribe, whip_request};
use crate::stream::SharablePipeline;
use actix_cors::Cors;
use actix_web::dev::Server;
use actix_web::{web, App, HttpServer};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

pub fn run(
    listener: TcpListener,
    app_state: SharableAppState,
    pipeline_state: SharablePipeline,
) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .route("/health_check", web::get().to(health_check))
            .route("/list", web::get().to(list))
            .route("/channel", web::post().to(subscribe))
            .route("/channel/{id}", web::patch().to(patch))
            .route("/channel/{id}", web::delete().to(remove_connection))
            .route("/whip_sink", web::post().to(whip_request))
            .app_data(web::Data::new(app_state.clone()))
            .app_data(web::Data::new(pipeline_state.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
