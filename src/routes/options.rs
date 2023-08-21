use actix_web::{http::header, HttpResponse};

#[tracing::instrument(name = "OPTIONS")]
pub async fn options() -> HttpResponse {
    HttpResponse::NoContent()
        .append_header((header::VARY, "Origin, Access-Control-Request-Headers"))
        .append_header((header::ACCESS_CONTROL_ALLOW_ORIGIN, "*"))
        .append_header((
            header::ACCESS_CONTROL_EXPOSE_HEADERS,
            "Location, Accept, Allow, Accept-POST",
        ))
        .append_header((
            header::ACCESS_CONTROL_ALLOW_METHODS,
            "POST, GET, OPTIONS, PATCH, PUT",
        ))
        .append_header(("ACCEPT-POST", "application/sdp"))
        .finish()
}
