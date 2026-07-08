use crate::errors::error_chain_fmt;
use std::fmt::Debug;
use thiserror::Error;

/// SDP validation failures — the domain's only error language.
#[derive(Error)]
pub enum SdpError {
    #[error("Invalid SDP: {0}")]
    InvalidSdp(String),
}

// Bespoke `Debug` to report the error source chain.
impl Debug for SdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}
