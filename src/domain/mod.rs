mod errors;
mod session_description;

pub use errors::{error_chain_fmt, SdpError};
pub use session_description::{SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
