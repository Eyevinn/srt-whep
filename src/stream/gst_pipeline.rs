use anyhow::{Error, Ok};
use async_trait::async_trait;
use gst::{message::Eos, prelude::*, DebugGraphDetails, Pipeline};
use gstreamer as gst;
use gstwebrtchttp;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Duration;
use timed_locks::Mutex;

use crate::stream::pipeline::{Args, PipelineBase, SRTMode};
use crate::stream::utils::run_discoverer;

#[derive(Clone)]
pub struct PipelineWrapper {
    pipeline: Option<Pipeline>,
    port: u32,
}

impl PipelineWrapper {
    fn new(args: Args) -> Self {
        Self {
            pipeline: None,
            port: args.port,
        }
    }
}

#[derive(Clone)]
pub struct SharablePipeline(Arc<Mutex<PipelineWrapper>>);

impl SharablePipeline {
    pub fn new(args: Args) -> Self {
        Self(Arc::new(Mutex::new_with_timeout(
            PipelineWrapper::new(args),
            Duration::from_secs(1),
        )))
    }
}

impl Deref for SharablePipeline {
    type Target = Arc<Mutex<PipelineWrapper>>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for SharablePipeline {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

#[async_trait]
impl PipelineBase for SharablePipeline {
    async fn add_connection(&self, id: String) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();
        tracing::debug!("Add connection {} to pipeline", id);

        let demux = pipeline
            .by_name("demux")
            .expect("pipeline has no element with name demux");
        let queue_video: gst::Element = gst::ElementFactory::make("queue")
            .name("video-queue-".to_string() + &id)
            .build()?;
        let queue_audio: gst::Element = gst::ElementFactory::make("queue")
            .name("audio-queue-".to_string() + &id)
            .build()?;
        let whipsink = gst::ElementFactory::make("whipsink")
            .name("whip-sink-".to_string() + &id)
            .property(
                "whip-endpoint",
                format!("http://localhost:{}/whip_sink/{}", pipeline_state.port, id),
            )
            .build()?;
        pipeline.add_many(&[&queue_video, &queue_audio, &whipsink])?;

        if demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("video"))
        {
            let output_tee_video = pipeline
                .by_name("output_tee_video")
                .expect("pipeline has no element with name output_tee_video");
            gst::Element::link_many(&[&output_tee_video, &queue_video, &whipsink])?;

            let video_elements = &[&output_tee_video, &queue_video, &whipsink];
            for e in video_elements {
                e.sync_state_with_parent()?;
            }

            tracing::debug!("Successfully linked video sink");
        }

        if demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("audio"))
        {
            let output_tee_audio = pipeline
                .by_name("output_tee_audio")
                .expect("pipeline has no element with name output_tee_audio");
            gst::Element::link_many(&[&output_tee_audio, &queue_audio, &whipsink])?;

            let audio_elements = &[&output_tee_audio, &queue_audio, &whipsink];
            for e in audio_elements {
                e.sync_state_with_parent()?;
            }

            tracing::debug!("Successfully linked audio sink");
        }

        pipeline.debug_to_dot_file(DebugGraphDetails::all(), "add-client");
        Ok(())
    }

    async fn remove_connection(&self, id: String) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();
        tracing::debug!("Remove connection {} from pipeline", id);

        std::process::exit(0);

        let remove_element = |name| -> Result<(), Error> {
            if let Some(_element) = pipeline.by_name(name) {
                // element.set_state(gst::State::Paused)?;
                // TODO: Find a way to remove element from pipeline without blocking pipeline
                //pipeline.remove(&element)?;
                tracing::warn!(
                    "We should have removed element with name {} but not yet",
                    name
                );
            } else {
                tracing::warn!("Element {} not found", name);
            }
            Ok(())
        };

        let video_element_name = "video-queue-".to_string() + &id;
        let audio_element_name = "audio-queue-".to_string() + &id;
        let whip_sink_name = "whip-sink-".to_string() + &id;

        remove_element(&video_element_name)?;
        remove_element(&audio_element_name)?;
        remove_element(&whip_sink_name)?;

        pipeline.debug_to_dot_file(DebugGraphDetails::all(), "after-remove");
        Ok(())
    }

    async fn setup_pipeline(&self, args: &Args) -> Result<(), Error> {
        // Initialize GStreamer (only once)
        gst::init()?;
        // Load whipsink
        gstwebrtchttp::plugin_register_static()?;
        tracing::debug!("Setting up pipeline");

        // Create a pipeline (WebRTC branch)
        let main_loop = glib::MainLoop::new(None, false);
        let pipeline = gst::Pipeline::default();

        let uri = format!(
            "srt://{}?mode={}",
            args.input_address,
            args.srt_mode.to_str()
        );
        tracing::info!("SRT Input uri: {}", uri);
        if args.srt_mode == SRTMode::Caller {
            tracing::info!("Running discoverer...");
            run_discoverer(&uri, args.discoverer_timeout_sec)?;
        }

        let src = gst::ElementFactory::make("srtsrc")
            .property("uri", uri)
            .build()?;
        let input_tee = gst::ElementFactory::make("tee").name("input_tee").build()?;

        let whep_queue = gst::ElementFactory::make("queue")
            .name("whep_queue")
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

        let srt_queue = gst::ElementFactory::make("queue")
            .name("srt_queue")
            .build()?;
        let output_uri = format!(
            "srt://{}?mode={}",
            args.output_address,
            args.srt_mode.reverse().to_str()
        );
        tracing::info!("SRT Output uri: {}", output_uri);
        let srtsink = gst::ElementFactory::make("srtsink")
            .property("uri", output_uri)
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
            tracing::info!("No more pads from the stream. Ready to link.");

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
                    &output_tee_audio,
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
                tracing::error!("Failed to link: {}", err);
            } else {
                tracing::info!("Successfully linked stream. Ready to play.");
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
                        tracing::error!("Unknown pad added {:?}", src_pad);
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

                    tracing::info!("Successfully inserted audio sink");
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

                    tracing::info!("Successfully inserted video sink");
                }

                Ok(())
            };

            if let Err(err) = insert_sink(is_audio, is_video) {
                // The following sends a message of type Error on the bus, containing our detailed
                // error information.
                tracing::error!("Failed to insert sink: {}", err);
            }
        });

        // Start playing
        let bus = pipeline.bus().unwrap();
        pipeline.set_state(gst::State::Playing)?;
        {
            // Store pipeline in state and drop lock
            let mut pipeline_state = self.lock_err().await?;
            pipeline_state.pipeline = Some(pipeline);
            drop(pipeline_state)
        }

        // Wait until an EOS or error message appears
        let main_loop_clone = main_loop.clone();
        let _bus_watch = bus.add_watch(move |_, msg| {
            use gst::MessageView;

            let main_loop = &main_loop_clone;
            match msg.view() {
                MessageView::Eos(..) => {
                    tracing::info!("received eos");
                    // An EndOfStream event was sent to the pipeline, so we tell our main loop
                    // to stop execution here.
                    main_loop.quit()
                }
                MessageView::Error(err) => {
                    tracing::error!(
                        "Error from {:?}: {} ({:?})",
                        err.src().map(|s| s.path_string()),
                        err.error(),
                        err.debug()
                    );
                    main_loop.quit();
                }
                _ => (),
            };

            // Tell the mainloop to continue executing this callback.
            glib::Continue(true)
        })?;

        // Operate GStreamer's bus, facilliating GLib's mainloop here.
        // This function call will block until you tell the mainloop to quit
        main_loop.run();

        Ok(())
    }

    async fn close_pipeline(&self) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();

        let eos_message = Eos::new();
        let bus = pipeline.bus().unwrap();
        bus.post(eos_message)?;

        pipeline.set_state(gst::State::Null)?;

        Ok(())
    }
}
