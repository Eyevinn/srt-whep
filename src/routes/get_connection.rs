use actix_web::{web, web::Bytes,HttpResponse, http::header::ContentType};
use crate::webrtc_internal::*;
use serde::{Deserialize, Serialize};
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use actix_web::http::Error;
use std::convert::{TryFrom, TryInto};
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
    
    // let sdp_str:String = match String::from_utf8(bytes.to_vec()) {
    //     Ok(s) => s.to_owned(),
    //     Err(err) => panic!("{}", err),
    // };
    
    let sdp = match serde_json::from_str::<SdpReq>(&form)
    {
        Ok(s) => s,
        Err(err) => panic!("{}", err),
    };
    
    let sdp = RTCSessionDescription::answer(sdp.offer).unwrap();

    _webrtc_state.set_remote_sdp(sdp).await.unwrap();
    
    
    
    
    let offer = _webrtc_state.get_offer().await.unwrap();

    // let response = { SdpResponse{
    //     media_streams: "hejhej".to_string(),
    //     offer: offer
    // }};
    let sdp_str = serde_json::to_string(&offer).unwrap();
    let sdp : Sdp = serde_json::from_str::<Sdp>(&sdp_str).unwrap();
    let body = serde_json::to_string(&sdp.sdp).unwrap();


    return Ok(HttpResponse::Created().content_type("application/sdp").body(sdp.sdp));
}