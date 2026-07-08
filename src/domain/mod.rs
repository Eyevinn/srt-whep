mod errors;
mod session_description;

pub use errors::SdpError;
pub use session_description::{
    SdpAnswer, SdpOffer, SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER,
};
