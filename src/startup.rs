use crate::domain::SharableAppState;
use crate::routes::*;
use crate::stream::PipelineBase;
use actix_cors::Cors;
use actix_web::dev::Server;
use actix_web::{guard, web, App, HttpServer};
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
            .route("/list", web::get().to(list::<T>))
            .route("/channel", web::post().to(whep_handler::<T>))
            .route("/channel", web::route().guard(guard::Options()).to(options))
            .route("/channel/{id}", web::patch().to(whep_patch_handler))
            .route("/channel/{id}", web::delete().to(remove_connection::<T>))
            .route("/whip_sink/{id}", web::post().to(whip_handler::<T>))
            .app_data(web::Data::new(app_state.clone()))
            .app_data(web::Data::new(pipeline_state.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
