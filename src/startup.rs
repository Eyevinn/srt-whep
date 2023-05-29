use crate::domain::SharableAppState;
use crate::routes::{health_check, patch, subscribe, srt_request};
use actix_cors::Cors;
use actix_web::dev::Server;
use actix_web::{web, App, HttpServer};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

pub fn run(listener: TcpListener, app_state: SharableAppState) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .route("/health_check", web::get().to(health_check))
            .route("/channel/{id}", web::patch().to(patch))
            .route("/channel", web::post().to(subscribe))
            .route("/srt_sink", web::post().to(srt_request))
            .app_data(web::Data::new(app_state.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
