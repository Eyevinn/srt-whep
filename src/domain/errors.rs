use std::fmt::Debug;
use thiserror::Error;

/// SDP validation failures — the domain's only error language.
#[derive(Error)]
pub enum SdpError {
    #[error("Invalid SDP: {0}")]
    InvalidSdp(String),
}

// We are still using a bespoke implementation of `Debug` to get a nice report using the error source chain
impl Debug for SdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}

pub(crate) fn error_chain_fmt(
    e: &impl std::error::Error,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    writeln!(f, "{}\n", e)?;
    let mut current = e.source();
    while let Some(cause) = current {
        writeln!(f, "Caused by:\n\t{}", cause)?;
        current = cause.source();
    }
    Ok(())
}
