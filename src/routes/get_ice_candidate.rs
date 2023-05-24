use actix_web::{web, web::Bytes,HttpResponse};
use crate::webrtc_internal::SharableWebrtcState;
use actix_web::http::Error;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct IceCandidate {
    candidate :String
}

pub async fn get_ice_candidate(bytes: Bytes, _webrtc_state: web::Data<SharableWebrtcState>) -> Result<HttpResponse, Error> {

    let ice_str : String = match String::from_utf8(bytes.to_vec()) {
        Ok(s) => s.to_owned(),
        Err(err) => panic!("{}", err),
    };

    let candidate = match serde_json::from_str::<IceCandidate>(&ice_str)
    {
        Ok(s) => s,
        Err(err) => panic!("{}", err),
    };

    _webrtc_state.set_ice_candidate(candidate.candidate).await.unwrap();


   return Ok(HttpResponse::NoContent().into());
}