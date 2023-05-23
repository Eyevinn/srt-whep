use anyhow::Error;

use gst::prelude::*;
use gstreamer as gst;

pub fn setup_pipeline() -> Result<(), Error> {
    gst::init()?;

    // Create a pipeline
    // gst-launch-1.0 srtsrc uri="srt://127.0.0.1:1234"  ! typefind ! queue !  rtpmp2tpay ! whipsink whip-endpoint="http://localhost:8000/subscriptions"
    let pipeline = gst::Pipeline::default();
    let src = gst::ElementFactory::make("srtsrc")
        .property("uri", "srt://127.0.0.1:1234")
        .build()?;
    let typefind = gst::ElementFactory::make("typefind").build()?;
    let queue = gst::ElementFactory::make("queue").build()?;
    let rtpmp2tpay = gst::ElementFactory::make("rtpmp2tpay").build()?;
    let whipsink = gst::ElementFactory::make("whipsink")
        .property("whip-endpoint", "http://localhost:8000/subscriptions")
        .build()?;

    pipeline.add_many(&[&src, &typefind, &queue, &rtpmp2tpay, &whipsink])?;
    gst::Element::link_many(&[&src, &typefind, &queue, &rtpmp2tpay, &whipsink])?;

    // Start playing
    pipeline.set_state(gst::State::Playing)?;

    // Wait until an EOS or error message appears
    let bus = pipeline.bus().unwrap();
    let _msg = bus.timed_pop_filtered(
        gst::ClockTime::NONE,
        &[gst::MessageType::Error, gst::MessageType::Eos],
    );

    // Clean up
    pipeline.set_state(gst::State::Null)?;

    Ok(())
}
