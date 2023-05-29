use actix_web::{web,HttpResponse};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use crate::webrtc_internal::SharableWebrtcState;
use actix_web::http::Error;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Sdp {
    answer: String
}

pub async fn get_answer(form: String, _webrtc_state: web::Data<SharableWebrtcState>) -> Result<HttpResponse, Error> {
    
    let sdp = RTCSessionDescription::answer(form).unwrap();

    _webrtc_state.set_remote_sdp(sdp).await.unwrap();

   return Ok(HttpResponse::NoContent().into());
}