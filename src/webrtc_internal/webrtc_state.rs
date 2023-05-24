use std::sync::{Arc, Mutex};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::{MediaEngine, MIME_TYPE_VP8};
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::track_local_static_rtp::TrackLocalStaticRTP;
use webrtc::track::track_local::TrackLocal;
use webrtc::Error;
use crate::domain::MyError;

struct WebrtcState {
  sdp: Option<RTCSessionDescription>,
  peer_connection: Option<Arc<RTCPeerConnection>>
}

impl WebrtcState {
  fn new() -> Self {
      Self {
          sdp: None,
          peer_connection: None,
      }
  }
}

#[derive(Clone)]
pub struct SharableWebrtcState(Arc<Mutex<WebrtcState>>);

impl SharableWebrtcState {
  pub fn new() -> Self {
    Self(Arc::new(Mutex::new(WebrtcState::new())))
}

pub async fn get_offer(&self) -> Result<RTCSessionDescription, MyError> {
  let mut webrtc_state = self.0.lock().unwrap();
  if let Some(sdp) = &webrtc_state.sdp {
      return Ok(sdp.clone());
  } else {
    if let Some(peer_connection) = &webrtc_state.peer_connection {
    let offer = peer_connection.create_offer(None).await.unwrap();
    peer_connection.set_local_description(offer.clone()).await.unwrap();
    webrtc_state.sdp = Some(offer.clone());
    return Ok(offer);
    } 
    return Err(MyError::ResourceNotFound);
  }
}

pub async fn set_remote_sdp(&self, sdp: RTCSessionDescription) -> Result<(), Error> {
  let webrtc_state = self.0.lock().unwrap();
  if let Some(peer_connection) = &webrtc_state.peer_connection {
    peer_connection.set_remote_description(sdp).await?;
  }
  return Ok(())
}

pub async fn set_ice_candidate(&self, candidate: String) -> Result<(), Error> {
  let webrtc_state = self.0.lock().unwrap();
   if let Some(peer_connection) = &webrtc_state.peer_connection {
      if let Err(err) = peer_connection.add_ice_candidate(RTCIceCandidateInit {
        candidate,
        ..Default::default()
    }).await
    {
      panic!("{}", err);
    }
  }
  return Ok(())
}

pub async fn set_up_peer(&self) -> Result<(), Error> {
  let mut webrtc_state = self.0.lock().unwrap();
  let mut m = MediaEngine::default();

  m.register_default_codecs()?;
  let mut registry = Registry::new();

  // Use the default set of Interceptors
  registry = register_default_interceptors(registry, &mut m)?;

  // Create the API object with the MediaEngine
  let api = APIBuilder::new()
      .with_media_engine(m)
      .with_interceptor_registry(registry)
      .build();

  // Prepare the configuration
  let config = RTCConfiguration {
      ice_servers: vec![RTCIceServer {
          urls: vec!["stun:stun.l.google.com:19302".to_owned()],
          ..Default::default()
      }],
      ..Default::default()
  };

  let peer_connection = Arc::new(api.new_peer_connection(config).await?);

  // Create Track that we send video back to browser on
  let video_track = Arc::new(TrackLocalStaticRTP::new(
      RTCRtpCodecCapability {
          mime_type: MIME_TYPE_VP8.to_owned(),
          ..Default::default()
      },
      "video".to_owned(),
      "webrtc-rs".to_owned(),
  ));

  let rtp_sender = peer_connection
    .add_track(Arc::clone(&video_track) as Arc<dyn TrackLocal + Send + Sync>)
    .await?;

  

  tokio::spawn(async move {
    let mut rtcp_buf = vec![0u8; 1500];
    while let Ok((_, _)) = rtp_sender.read(&mut rtcp_buf).await {}
    Ok::<(), Error>(())
  });


  webrtc_state.peer_connection = Some(peer_connection);
  return Ok(())
}

}