use crate::domain::SharableAppState;
use crate::webrtc_internal::SharableWebrtcState;
use crate::routes::{health_check, subscribe, get_connection, get_status, get_answer};
use actix_web::dev::Server;
use actix_web::{web, App, HttpServer};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;
use actix_cors::Cors;

pub fn run(listener: TcpListener, app_state: SharableAppState, webrtc_state: SharableWebrtcState) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .route("/health_check", web::get().to(health_check))
            .route("/subscriptions", web::post().to(subscribe))
            .route("/channel", web::post().to(get_connection))
            .route("/channel/{id}", web::patch().to(get_answer))
            .app_data(web::Data::new(app_state.clone()))
            .app_data(web::Data::new(webrtc_state.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
 