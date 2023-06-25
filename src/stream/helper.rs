use anyhow::Error;
use derive_more::{Display, Error};
use gst_pbutils::{prelude::*, DiscovererInfo, DiscovererStreamInfo};
use gstreamer as gst;
use gstreamer_pbutils as gst_pbutils;

#[derive(Debug, Display, Error)]
#[display(fmt = "Discoverer error {_0}")]
struct DiscovererError(#[error(not(source))] &'static str);

fn print_tags(info: &DiscovererInfo) {
    tracing::info!("Tags:");

    let tags = info.tags();
    match tags {
        Some(taglist) => {
            tracing::info!("  {taglist}"); // FIXME use an iterator
        }
        None => {
            tracing::info!("  no tags");
        }
    }
}

fn print_stream_info(stream: &DiscovererStreamInfo) {
    tracing::info!("Stream: ");

    let caps_str = match stream.caps() {
        Some(caps) => caps.to_string(),
        None => String::from("--"),
    };
    tracing::info!("  Format: {caps_str}");
}

fn print_discoverer_info(info: &DiscovererInfo) -> Result<(), Error> {
    tracing::info!("Duration: {}", info.duration().display());

    print_tags(info);
    print_stream_info(
        &info
            .stream_info()
            .ok_or(DiscovererError("Error while obtaining stream info"))?,
    );

    let children = info.stream_list();
    tracing::info!("Children streams:");
    for child in children {
        print_stream_info(&child);
    }

    Ok(())
}

pub fn run_discoverer(uri: &str, timeout_sec: u64) -> Result<(), Error> {
    // gst::init()?;

    let timeout: gst::ClockTime = gst::ClockTime::from_seconds(timeout_sec);
    let discoverer = gst_pbutils::Discoverer::new(timeout)?;
    let info = discoverer.discover_uri(uri)?;
    print_discoverer_info(&info)?;

    Ok(())
}
