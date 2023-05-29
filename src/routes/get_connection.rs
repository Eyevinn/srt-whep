use actix_web::{web,HttpResponse};
use crate::webrtc_internal::*;
use serde::{Deserialize, Serialize};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use actix_web::http::Error;
use crate::domain::SharableAppState;

#[derive(Serialize, Deserialize)]
struct SdpResponse {
    media_streams: String,
    offer: RTCSessionDescription,
}

#[derive(Serialize, Deserialize)]
struct Sdp {
    r#type: String,
    sdp: String,
}

#[derive(Serialize, Deserialize)]
struct SdpReq {
    offer: String
}
#[tracing::instrument(name = "Receive an offer from a client", skip(form, app_state, _webrtc_state))]
pub async fn get_connection(form: String, app_state: web::Data<SharableAppState>, _webrtc_state: web::Data<SharableWebrtcState>) -> Result<HttpResponse, Error> {
    
    if form.is_empty() {
        let offer = _webrtc_state.get_offer().await.unwrap();

    
        let sdp_str = serde_json::to_string(&offer).unwrap();
        let sdp : Sdp = serde_json::from_str::<Sdp>(&sdp_str).unwrap();
        let body = serde_json::to_string(&sdp.sdp).unwrap();
    
    
        return Ok(HttpResponse::Created().append_header((
            "Location",
            format!("http://0.0.0.0:8000/channel/{}", "hej"),
        ))
        .content_type("application/sdp")
        .body(sdp.sdp));
    }
    return Ok(HttpResponse::BadRequest().into());
    
}