use crate::domain::SharableAppState;
use crate::routes::{health_check, subscribe};
use actix_web::dev::Server;
use actix_web::{web, App, HttpServer};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

pub fn run(listener: TcpListener, app_state: SharableAppState) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger::default())
            .route("/health_check", web::get().to(health_check))
            .route("/", web::post().to(subscribe))
            .app_data(web::Data::new(app_state.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
