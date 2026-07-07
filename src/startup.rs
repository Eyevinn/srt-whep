use crate::routes::*;
use crate::signal::SignalHandle;
use actix_cors::Cors;
use actix_web::dev::Server;
use actix_web::{guard, web, App, HttpServer};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

pub fn run(listener: TcpListener, signal: SignalHandle) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .route("/list", web::get().to(list))
            .route("/channel", web::post().to(whep_handler))
            .route("/channel", web::route().guard(guard::Options()).to(options))
            .route("/channel/{id}", web::patch().to(whep_patch_handler))
            .route("/channel/{id}", web::delete().to(remove_connection))
            .route("/whip_sink/{id}", web::post().to(whip_handler))
            .app_data(web::Data::new(signal.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
