use anyhow::Error;
use clap::Parser;

use gst::prelude::*;
use gstreamer as gst;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// SRT source stream address(ip:port)
    #[arg(short, long)]
    input_address: String,

    /// SRT output stream address(ip:port)
    #[arg(short, long)]
    output_address: String,

    /// Port for whep client
    #[arg(short, long, default_value_t = 8000)]
    pub port: u32,
}

pub fn setup_pipeline(args: &Args) -> Result<(), Error> {
    gst::init()?;

    // Create a pipeline
    // gst-launch-1.0 srtsrc uri="srt://127.0.0.1:1234" ! tee name=t \
    //     t. ! queue ! typefind ! rtpmp2tpay ! whipsink whip-endpoint="http://localhost:8000/subscriptions" \
    //     t. ! queue ! srtserversink uri="srt://:8888" wait-for-connection=false
    let pipeline = gst::Pipeline::default();
    let src = gst::ElementFactory::make("srtsrc")
        .property("uri", format!("srt://{}", args.input_address))
        .build()?;
    let tee = gst::ElementFactory::make("tee").name("tee").build()?;
    let whep_queue = gst::ElementFactory::make("queue")
        .name("whep_queue")
        .build()
        .unwrap();
    let srt_queue = gst::ElementFactory::make("queue")
        .name("srt_queue")
        .build()
        .unwrap();
    let typefind = gst::ElementFactory::make("typefind").build()?;
    let rtpmp2tpay = gst::ElementFactory::make("rtpmp2tpay").build()?;
    let whipsink = gst::ElementFactory::make("whipsink")
        .property("whip-endpoint", format!("http://localhost:{}", args.port))
        .build()?;
    let srtserversink = gst::ElementFactory::make("srtserversink")
        .property("uri", format!("srt://{}", args.output_address))
        .property("wait-for-connection", false)
        .build()?;

    pipeline.add_many(&[
        &src,
        &tee,
        &whep_queue,
        &srt_queue,
        &typefind,
        &rtpmp2tpay,
        &whipsink,
        &srtserversink,
    ])?;
    gst::Element::link_many(&[&src, &tee])?;
    gst::Element::link_many(&[&tee, &whep_queue, &typefind, &rtpmp2tpay, &whipsink])?;
    gst::Element::link_many(&[&tee, &srt_queue, &srtserversink])?;

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
