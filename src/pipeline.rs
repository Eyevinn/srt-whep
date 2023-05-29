use anyhow::Error;
use clap::Parser;

use gst::{prelude::*, DebugGraphDetails};
use gstreamer as gst;

/// Simple program to greet a person
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
#[derive(Clone)]
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

    // Create a pipeline (WebRTC branch)
    // gst-launch-1.0 srtsrc uri="srt://127.0.0.1:1234"  ! decodebin name=d \
    //     d. ! queue ! x264enc tune=zerolatency ! rtph264pay ! whipsink whip-endpoint="http://localhost:8000/subscriptions" name=ws \
    //     d. ! queue ! audioconvert ! audioresample ! opusenc ! rtpopuspay ! ws.
    
    // gst-launch-1.0 srtsrc uri="srt://127.0.0.1:1234"  ! typefind ! queue ! tsdemux name=demux \
    //     demux. ! queue ! h264parse ! rtph264pay ! whipsink whip-endpoint="http://localhost:8000/subscriptions"
    let pipeline = gst::Pipeline::default();
    let src = gst::ElementFactory::make("srtsrc")
        .property("uri", format!("srt://{}", args.input_address))
        .build()?;
    let tee = gst::ElementFactory::make("tee").name("tee").build()?;
    let whep_queue = gst::ElementFactory::make("queue")
        .name("whep_queue")
        .build()?;
    let srt_queue = gst::ElementFactory::make("queue")
        .name("srt_queue")
        .build()?;
    let typefind = gst::ElementFactory::make("typefind").name("typefind").build()?;
    let tsdemux = gst::ElementFactory::make("tsdemux")
        .name("demux")
        .build()?;

    let video_queue: gst::Element = gst::ElementFactory::make("queue")
        .name("video-queue")
        .build()?;
    let h264parse = gst::ElementFactory::make("h264parse").build()?;
    let rtph264pay = gst::ElementFactory::make("rtph264pay").build()?;

    // let audio_queue: gst::Element = gst::ElementFactory::make("queue")
    //     .name("audio-queue")
    //     .build()?;
    // let aacparse = gst::ElementFactory::make("aacparse").build()?;
    // let avdec_aac = gst::ElementFactory::make("avdec_aac").build()?;
    // let audioconvert = gst::ElementFactory::make("audioconvert").build()?;
    // let audioresample = gst::ElementFactory::make("audioresample").build()?;
    // let opusenc = gst::ElementFactory::make("opusenc").build()?;
    // let rtpopuspay = gst::ElementFactory::make("rtpopuspay").build()?;
    let whipsink = gst::ElementFactory::make("whipsink")
        .property("whip-endpoint", format!("http://localhost:{}/srt_sink", args.port))
        .build()?;
    let srtserversink = gst::ElementFactory::make("srtserversink")
        .property("uri", format!("srt://{}", args.output_address))
        .property("async", false) // to not block tee
        .property("wait-for-connection", false)
        .build()?;

    pipeline.add_many(&[
        &src,
        &tee,
        &whep_queue,
        &srt_queue,
        &typefind,
        &tsdemux,
        &video_queue,
        // &audio_queue,
        &h264parse,
        // &aacparse,
        // &avdec_aac,
        // &audioconvert,
        // &audioresample,
        &rtph264pay,
        // &opusenc,
        // &rtpopuspay,
        &srtserversink,
    ])?;
    gst::Element::link_many(&[&src, &tee])?;
    gst::Element::link_many(&[&tee, &whep_queue, &typefind, &tsdemux])?;
    gst::Element::link_many(&[&tee, &srt_queue, &srtserversink])?;
    // gst::Element::link_many(&[&src, &typefind, &whep_queue, &tsdemux])?;

    let pipeline_weak = pipeline.downgrade();
    // Connect to tsdemux's no-more-pads signal, that is emitted when the element
    // will not generate more dynamic pads.
    tsdemux.connect_no_more_pads(move |_| {
        // Here we temporarily retrieve a strong reference on the pipeline from the weak one
        // we moved into this callback.
        let pipeline = match pipeline_weak.upgrade() {
            Some(pipeline) => pipeline,
            None => return,
        };

        let link_sink = || -> Result<(), Error> {
            let video_elements = &[&video_queue, &h264parse, &rtph264pay, &whipsink];
            gst::Element::link_many(video_elements).expect("Failed to link video elements");
            // Link only video elements for the moment

            // let audio_elements = &[
            //     &audio_queue,
            //     &aacparse,
            //     &avdec_aac,
            //     &audioconvert,
            //     &audioresample,
            //     &opusenc,
            //     &rtpopuspay,
            //     &whipsink,
            // ];
            // gst::Element::link_many(audio_elements).expect("Failed to link audio elements");

            // This is quite important and people forget it often. Without making sure that
            // the new elements have the same state as the pipeline, things will fail later.
            // They would still be in Null state and can't process data.
            for e in video_elements {
                e.sync_state_with_parent()?;
            }

            // for e in audio_elements {
            //     e.sync_state_with_parent()?;
            // }

            Ok(())
        };

        pipeline
            .add(&whipsink)
            .expect("Failed to add whipsink into pipeline");
        if let Err(err) = link_sink() {
            // The following sends a message of type Error on the bus, containing our detailed
            // error information.
            println!("Failed to link whip sink: {}", err);
        } else {
            println!("Successfully linked whip sink");
            pipeline.debug_to_dot_file(DebugGraphDetails::all(), "pipeline");
        }
    });

    let pipeline_weak = pipeline.downgrade();
    // Connect to decodebin's pad-added signal, that is emitted whenever
    // it found another stream from the input file and found a way to decode it to its raw format.
    // decodebin automatically adds a src-pad for this raw stream, which
    // we can use to build the follow-up pipeline.
    tsdemux.connect_pad_added(move |_dbin, src_pad| {
        // Here we temporarily retrieve a strong reference on the pipeline from the weak one
        // we moved into this callback.
        let pipeline = match pipeline_weak.upgrade() {
            Some(pipeline) => pipeline,
            None => return,
        };

        // Try to detect whether the raw stream decodebin provided us with
        // just now is either audio or video (or none of both, e.g. subtitles).
        let (is_audio, is_video) = {
            let media_type = src_pad.current_caps().and_then(|caps| {
                caps.structure(0).map(|s| {
                    let name = s.name();
                    (name.starts_with("audio/"), name.starts_with("video/"))
                })
            });

            match media_type {
                None => {
                    println!("Unknown pad added {:?}", src_pad);
                    return;
                }
                Some(media_type) => media_type,
            }
        };

        let insert_sink = |is_audio, is_video| -> Result<(), Error> {
            // if is_audio {
            //     // Get the queue element's sink pad and link the decodebin's newly created
            //     // src pad for the audio stream to it.
            //     let audio_queue = pipeline
            //         .by_name("audio-queue")
            //         .expect("pipeline has no element with name audio-queue");
            //     let sink_pad = audio_queue
            //         .static_pad("sink")
            //         .expect("queue has no sinkpad");
            //     src_pad.link(&sink_pad)?;
            // }
            if is_video {
                // Get the queue element's sink pad and link the decodebin's newly created
                // src pad for the video stream to it.
                let video_queue = pipeline
                    .by_name("video-queue")
                    .expect("pipeline has no element with name video-queue");
                let sink_pad = video_queue
                    .static_pad("sink")
                    .expect("queue has no sinkpad");
                src_pad.link(&sink_pad)?;
            }

            Ok(())
        };

        if let Err(err) = insert_sink(is_audio, is_video) {
            // The following sends a message of type Error on the bus, containing our detailed
            // error information.
            println!("Failed to insert sink: {}", err);
        } else {
            println!("Successfully inserted sink");
        }
    });

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
