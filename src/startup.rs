use crate::domain::SharableAppState;
use crate::webrtc_internal::SharableWebrtcState;
use crate::routes::{health_check, subscribe, get_connection, get_status, get_answer, get_ice_candidate};
use actix_web::dev::Server;
use actix_web::{web, App, HttpServer};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

pub fn run(listener: TcpListener, app_state: SharableAppState, webrtc_state: SharableWebrtcState) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        App::new()
            .wrap(TracingLogger::default())
            .route("/health_check", web::get().to(health_check))
            .route("/subscriptions", web::post().to(subscribe))
            .route("broadcaster/channel/{id}", web::get().to(get_status))
            .route("/broadcaster/channel/{id}", web::post().to(get_connection))
            .route("broadcaster/channel/{id}/{sessionId}", web::put().to(get_answer))
            .route("broadcaster/channel/{id}/{sessionId}", web::patch().to(get_ice_candidate))
            .app_data(web::Data::new(app_state.clone()))
            .app_data(web::Data::new(webrtc_state.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
 