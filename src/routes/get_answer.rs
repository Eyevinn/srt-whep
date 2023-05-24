use actix_web::{web, web::Bytes,HttpResponse};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use crate::webrtc_internal::SharableWebrtcState;
use actix_web::http::Error;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Sdp {
    answer: String
}

pub async fn get_answer(bytes: Bytes, _webrtc_state: web::Data<SharableWebrtcState>) -> Result<HttpResponse, Error> {

    //let sdp = serde_json::from_str::<RTCSessionDescription>(&_form).unwrap();


    let sdp_str:String = match String::from_utf8(bytes.to_vec()) {
        Ok(s) => s.to_owned(),
        Err(err) => panic!("{}", err),
    };
    
    //let sdp = RTCSessionDescription::answer(sdp_str).unwrap();
    let sdp = match serde_json::from_str::<Sdp>(&sdp_str)
    {
        Ok(s) => s,
        Err(err) => panic!("{}", err),
    };
    let sdp = RTCSessionDescription::answer(sdp.answer).unwrap();

    _webrtc_state.set_remote_sdp(sdp).await.unwrap();

   return Ok(HttpResponse::NoContent().into());
}