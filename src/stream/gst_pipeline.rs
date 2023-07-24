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
    /// Add connection to pipeline
    /// # Arguments
    /// * `id` - Connection id
    ///
    /// Based on the stream type (audio or video) of the connection, the corresponding branch is created
    /// For whipsink to work, the branch must be linked to the output tee element and synced in state
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

        if !demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("video") || pad.name().starts_with("audio"))
        {
            tracing::error!("No pad's name starts with 'video' or 'audio'");
        }

        Ok(())
    }

    /// Remove connection from pipeline
    /// # Arguments
    /// * `id` - Connection id (must exist in the pipeline)
    ///
    /// Based on the id, we find the corresponding branch (video or audio) and this branch could
    /// be linked to the output tee element or not (audio queue is not linked when stream contains no audio)
    /// If the branch is linked, we block the tee's source pad with a pad probe and remove the branch in the callback
    /// If the branch is not linked, we remove the branch directly
    /// After removing the branch, we remove the whip sink from the pipeline
    async fn remove_connection(&self, id: String) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();
        tracing::debug!("Remove connection {} from pipeline", id);

        let video_tee_name = "output_tee_video";
        let audio_tee_name = "output_tee_audio";
        let video_queue_name = "video-queue-".to_string() + &id;
        let audio_queue_name = "audio-queue-".to_string() + &id;
        let whip_sink_name = "whip-sink-".to_string() + &id;

        // Try to remove branch from pipeline
        // Return Ok if branch is removed
        let try_remove_branch_from_pipeline =
            |pipeline: &Pipeline, tee_name: &str, queue_name: &str| -> Result<(), Error> {
                let tee = pipeline.by_name(tee_name).expect("Failed to get tee");
                let queue = pipeline.by_name(queue_name).expect("Failed to get queue");
                let queue_sink_pad = queue
                    .static_pad("sink")
                    .expect("Failed to get queue sink pad");
                tracing::debug!("Removing queue {} from pipeline", queue_name);

                // Remove src pad from tee if queue is linked
                let name = queue_name.to_string();
                if queue_sink_pad.is_linked() {
                    let tee_src_pad = queue_sink_pad.peer().expect("Failed to get tee src pad");

                    let pipeline_weak = pipeline.downgrade();
                    // Block tee's source pad with a pad probe.
                    // the probe callback is called as soon as the pad becomes idle
                    tee_src_pad.add_probe(gst::PadProbeType::IDLE, move |pad, _info| {
                        let pipeline = match pipeline_weak.upgrade() {
                            Some(pipeline) => pipeline,
                            // drop src pad if pipeline is already dropped
                            None => return gst::PadProbeReturn::Drop,
                        };

                        queue
                            .set_state(gst::State::Null)
                            .expect("Failed to clean queue");
                        pipeline
                            .remove(&queue)
                            .expect("Failed to remove queue from pipeline");
                        tracing::debug!("Queue {} was removed from pipeline", name);

                        tee.release_request_pad(pad);

                        gst::PadProbeReturn::Ok
                    });
                } else {
                    tracing::debug!("Queue {} is not linked", name);

                    // Audio queue is not linked to tee if stream contains no audio
                    // Remove them directly from pipeline
                    pipeline
                        .remove(&queue)
                        .expect("Failed to remove queue from pipeline");
                }

                Ok(())
            };

        try_remove_branch_from_pipeline(pipeline, video_tee_name, &video_queue_name)?;
        try_remove_branch_from_pipeline(pipeline, audio_tee_name, &audio_queue_name)?;

        // Remove whip sink from pipeline
        // If whip sink fails to send offer, it is removed from
        // pipeline automatically (so no need to remove it again)
        if let Some(whip_sink) = pipeline.by_name(&whip_sink_name) {
            whip_sink
                .set_state(gst::State::Null)
                .expect("Failed to pause clean whip sink");
            pipeline
                .remove(&whip_sink)
                .expect("Failed to remove whip sink from pipeline");
        }

        Ok(())
    }

    /// Setup pipeline
    /// # Arguments
    /// * `args` - Pipeline arguments
    ///
    /// Create a pipeline with the all needed elements and register callbacks for dynamic pads
    /// Link them together when the demux element generates all dynamic pads
    /// Start playing and wait until the message bus receives an EOS or error message
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

        // Run discoverer if the source stream is in listener mode (we are the caller)
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
                tracing::error!("Failed to link: {}", err);
            } else {
                tracing::info!("Successfully linked stream. Ready to play.");
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

    /// Close pipeline by sending EOS message
    async fn close_pipeline(&self) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();

        let eos_message = Eos::new();
        let bus = pipeline.bus().unwrap();
        bus.post(eos_message)?;

        pipeline.set_state(gst::State::Null)?;

        Ok(())
    }

    /// Print pipeline to dot file
    /// Warning: this function should not be called when the pipeline is in locked state
    async fn print_pipeline(&self) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();
        pipeline.debug_to_dot_file(DebugGraphDetails::all(), "pipeline");

        Ok(())
    }
}
