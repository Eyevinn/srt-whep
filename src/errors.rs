//! Cross-cutting error utilities shared by every layer's error type.

/// Format an error together with its `source()` chain, e.g. for a bespoke
/// `Debug` impl that reports the whole causal chain rather than one line.
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
