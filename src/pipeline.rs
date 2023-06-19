use anyhow::{Error, Ok};
use clap::{Parser, ValueEnum};
use gst::{prelude::*, DebugGraphDetails, Pipeline};
use gstreamer::message::Eos;
use gstreamer as gst;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// SRT source stream address(ip:port)
    #[arg(short, long)]
    pub input_address: String,

    #[arg(short, long)]
    #[clap(value_enum)]
    pub srt_mode: SRTMode,

    /// SRT output stream address(ip:port)
    #[arg(short, long)]
    pub output_address: String,

    /// Port for whep client
    #[arg(short, long, default_value_t = 8000)]
    pub port: u32,
}

#[derive(ValueEnum, Clone, Debug)]
pub enum SRTMode {
    Caller,
    Listener
}

impl SRTMode {
    fn to_str(&self) -> &str {
        match self {
            SRTMode::Caller => "caller",
            SRTMode::Listener => "listener",
        }
    }

    fn reverse(&self) -> Self {
        match self {
            SRTMode::Caller => SRTMode::Listener,
            SRTMode::Listener => SRTMode::Caller,
        }
    }
}

#[derive(Clone)]
struct PipelineStruct {
    pipeline: Option<Pipeline>,
    port: u32,
}

impl PipelineStruct {
    fn new(_args: Args) -> Self {
        Self {
            pipeline: None,
            port: _args.port,
        }
    }
}

#[derive(Clone)]
pub struct SharablePipeline(Arc<Mutex<PipelineStruct>>);

impl SharablePipeline {
    pub fn new(_args: Args) -> Self {
        Self(Arc::new(Mutex::new(PipelineStruct::new(_args))))
    }

    pub fn add_client(&self, resource_id: String) -> Result<(), Error> {
        let pipeline_state = self.0.lock().unwrap();
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();

        let queue_video: gst::Element = gst::ElementFactory::make("queue").name("video-queue-".to_string() + &resource_id.to_owned()).build()?;
        let queue_audio: gst::Element = gst::ElementFactory::make("queue").name("audio-queue-".to_string() + &resource_id.to_owned()).build()?;
        let whipsink = gst::ElementFactory::make("whipsink").name(("whip-sink-".to_owned() +  &resource_id.clone()).as_str(),)
            .property(
                "whip-endpoint",
                format!("http://localhost:{}/whip_sink", pipeline_state.port),
            )
            .build()?;
        let output_tee_video = pipeline
            .by_name("output_tee_video")
            .expect("pipeline has no element with name output_tee_video");

            let output_tee_audio = pipeline
            .by_name("output_tee_audio")
            .expect("pipeline has no element with name output_tee_audio");

        pipeline.add_many(&[&queue_video, &queue_audio, &whipsink]).unwrap();
        gst::Element::link_many(&[&output_tee_video, &queue_video, &whipsink]).unwrap();
        gst::Element::link_many(&[&output_tee_audio, &queue_audio, &whipsink]).unwrap();

        let video_elements = &[&output_tee_video, &queue_video, &whipsink];
        for e in video_elements {
            e.sync_state_with_parent()?;
        }

        let video_elements = &[&output_tee_audio, &queue_audio, &whipsink];
        for e in video_elements {
            e.sync_state_with_parent()?;
        }

        pipeline.debug_to_dot_file(DebugGraphDetails::all(), "add-client");
        Ok(())
    }

    pub fn remove_connection(&self, id: String) -> Result<(), Error> {
        let pipeline_state = self.0.lock().unwrap();
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();
        
        let video_queue = pipeline
            .by_name(("video-queue-".to_string() + &id.clone()).as_str())
            .expect(("pipeline has no element with name video-queue-".to_owned() + &id.clone()).as_str());
        let audio_queue = pipeline
            .by_name(("audio-queue-".to_string() + &id.clone()).as_str())
            .expect(("pipeline has no element with name output_tee_video".to_owned() + &id.clone()).as_str());
        let whip_sink = pipeline
            .by_name(("whip-sink-".to_owned() + &id.clone()).as_str())
            .expect(("pipeline has no element with name output_tee_video".to_owned() + &id.clone()).as_str());

        video_queue.set_state(gst::State::Null).unwrap();
        audio_queue.set_state(gst::State::Null).unwrap();
        whip_sink.set_state(gst::State::Null).unwrap();

        pipeline.remove(&whip_sink).expect("Failed to remove element");
        pipeline.remove(&video_queue).expect("Failed to remove element");
        pipeline.remove(&audio_queue).expect("Failed to remove element");
        
        pipeline.debug_to_dot_file(DebugGraphDetails::all(), "after-remove");

        return Ok(());
    }

    pub fn setup_pipeline(&self, args: &Args) -> Result<(), Error> {
        // Initialize GStreamer (only once)
        gst::init()?;

        // Create a pipeline (WebRTC branch)
        let pipeline = gst::Pipeline::default();

        let src = gst::ElementFactory::make("srtsrc")
            .property("uri", format!("srt://{}?mode={}", args.input_address, args.srt_mode.to_str()))
            .build()?;
        let input_tee = gst::ElementFactory::make("tee").name("input_tee").build()?;

        let whep_queue = gst::ElementFactory::make("queue")
            .name("whep_queue")
            .build()?;
        let srt_queue = gst::ElementFactory::make("queue")
            .name("srt_queue")
            .build()?;
        let typefind = gst::ElementFactory::make("typefind")
            .name("typefind")
            .build()?;
        let tsdemux = gst::ElementFactory::make("tsdemux").name("demux").build()?;

        let video_queue: gst::Element = gst::ElementFactory::make("queue")
            .name("video-queue")
            .build()?;
        let h264parse = gst::ElementFactory::make("h264parse").build()?;
        let rtph264pay = gst::ElementFactory::make("rtph264pay").build()?;
        let output_tee_video = gst::ElementFactory::make("tee")
            .name("output_tee_video")
            .build()?;

        let output_tee_audio = gst::ElementFactory::make("tee")
            .name("output_tee_audio")
            .build()?;

        let audio_queue: gst::Element = gst::ElementFactory::make("queue")
            .name("audio-queue")
            .build()?;
        let aacparse = gst::ElementFactory::make("aacparse").build()?;
        let avdec_aac = gst::ElementFactory::make("avdec_aac").build()?;
        let audioconvert = gst::ElementFactory::make("audioconvert").build()?;
        let audioresample = gst::ElementFactory::make("audioresample").build()?;
        let opusenc = gst::ElementFactory::make("opusenc").build()?;
        let rtpopuspay = gst::ElementFactory::make("rtpopuspay").build()?;
        let srtsink = gst::ElementFactory::make("srtsink")
            .property("uri", format!("srt://{}?mode={}", args.output_address, args.srt_mode.reverse().to_str()))
            .property("async", false) // to not block tee
            .property("wait-for-connection", false)
            .build()?;

        pipeline.add_many(&[
            &src,
            &input_tee,
            &whep_queue,
            &srt_queue,
            &typefind,
            &tsdemux,
            &video_queue,
            &audio_queue,
            &h264parse,
            &aacparse,
            &avdec_aac,
            &audioconvert,
            &audioresample,
            &rtph264pay,
            &output_tee_video,
            &output_tee_audio,
            &opusenc,
            &rtpopuspay,
            &srtsink,
        ])?;
        gst::Element::link_many(&[&src, &input_tee])?;
        gst::Element::link_many(&[&input_tee, &whep_queue, &typefind, &tsdemux])?;
        gst::Element::link_many(&[&video_queue, &h264parse, &rtph264pay, &output_tee_video])?;
        gst::Element::link_many(&[&input_tee, &srt_queue, &srtsink])?;

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

                let queue = gst::ElementFactory::make("queue").build()?;
                let fakesink = gst::ElementFactory::make("fakesink").build()?;
                let output_tee_video = pipeline
                    .by_name("output_tee_video")
                    .expect("pipeline has no element with name output_tee_video");
        
                pipeline.add_many(&[&queue, &fakesink])?;
                gst::Element::link_many(&[&output_tee_video, &queue, &fakesink])?;
                
                let video_elements = &[
                    &video_queue,
                    &h264parse, 
                    &rtph264pay, 
                    &output_tee_video,
                    &queue,
                    &fakesink,
                ];

                let output_tee_audio = pipeline
                .by_name("output_tee_audio")
                .expect("pipeline has no element with name output_tee_audio");

                let audio_elements = &[
                    &audio_queue,
                    &aacparse,
                    &avdec_aac,
                    &audioconvert,
                    &audioresample,
                    &opusenc,
                    &rtpopuspay,
                    &output_tee_audio
                ];
                gst::Element::link_many(audio_elements).expect("Failed to link audio elements");

                // This is quite important and people forget it often. Without making sure that
                // the new elements have the same state as the pipeline, things will fail later.
                // They would still be in Null state and can't process data.
                for e in video_elements {
                    e.sync_state_with_parent()?;
                }

                for e in audio_elements {
                    e.sync_state_with_parent()?;
                }

                Ok(())
            };

            if let Err(err) = link_sink() {
                // The following sends a message of type Error on the bus, containing our detailed
                // error information.
                println!("Failed to link demux: {}", err);
            } else {
                println!("Successfully linked demux");

                // export GST_DEBUG_DUMP_DOT_DIR=/tmp
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
                if is_audio {
                    // Get the queue element's sink pad and link the decodebin's newly created
                    // src pad for the audio stream to it.
                    let audio_queue = pipeline
                        .by_name("audio-queue")
                        .expect("pipeline has no element with name audio-queue");
                    let sink_pad = audio_queue
                        .static_pad("sink")
                        .expect("queue has no sinkpad");
                    src_pad.link(&sink_pad)?;
                }
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
        // Wait until an EOS or error message appears
        let bus = pipeline.bus().unwrap();
        pipeline.set_state(gst::State::Playing)?;

        {
            let mut pipeline_state = self.0.lock().unwrap();
            pipeline_state.pipeline = Some(pipeline);
        }

        let _msg = bus.timed_pop_filtered(
            gst::ClockTime::NONE,
            &[gst::MessageType::Error, gst::MessageType::Eos],
        );

        Ok(())
    }

    pub fn close_pipeline(&self) -> Result<(), Error> {
        let pipeline_state = self.0.lock().unwrap();
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();

        let eos_message = Eos::new();
        let bus = pipeline.bus().unwrap();
        bus.post(eos_message).unwrap();
        
        pipeline.set_state(gst::State::Null).unwrap();
        return Ok(());
    }
}
