use anyhow::{Error, Ok, Result};
use async_trait::async_trait;
use gst::{prelude::*, DebugGraphDetails, Pipeline};
use gstreamer as gst;
use gstwebrtchttp;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::time::Duration;
use timed_locks::Mutex;

use crate::domain::MyError;
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
            .ok_or(MyError::MissingElement("demux".to_string()))?;
        if !demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("video") || pad.name().starts_with("audio"))
        {
            tracing::error!("Demux has no pad available. No connection can be added.");
            return Err(MyError::FailedOperation("No available stream".to_string()).into());
        }

        // Create whip sink when demux has at least one video or audio pad
        let whipsink = gst::ElementFactory::make("whipsink")
            .name("whip-sink-".to_string() + &id)
            .property(
                "whip-endpoint",
                format!("http://localhost:{}/whip_sink/{}", pipeline_state.port, id),
            )
            .build()?;
        pipeline.add_many([&whipsink])?;

        if demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("video"))
        {
            let output_tee_video = pipeline
                .by_name("output_tee_video")
                .ok_or(MyError::MissingElement("output_tee_video".to_string()))?;
            let queue_video: gst::Element = gst::ElementFactory::make("queue")
                .name("video-queue-".to_string() + &id)
                .build()?;
            pipeline.add_many([&queue_video])?;
            gst::Element::link_many([&output_tee_video, &queue_video, &whipsink])?;

            let video_elements = &[&output_tee_video, &queue_video];
            for e in video_elements {
                e.sync_state_with_parent()?;
            }

            tracing::debug!("Successfully linked video to whip sink");
        }

        if demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("audio"))
        {
            let output_tee_audio = pipeline
                .by_name("output_tee_audio")
                .ok_or(MyError::MissingElement("output_tee_audio".to_string()))?;
            let queue_audio: gst::Element = gst::ElementFactory::make("queue")
                .name("audio-queue-".to_string() + &id)
                .build()?;
            pipeline.add_many([&queue_audio])?;
            gst::Element::link_many([&output_tee_audio, &queue_audio, &whipsink])?;

            let audio_elements = &[&output_tee_audio, &queue_audio];
            for e in audio_elements {
                e.sync_state_with_parent()?;
            }

            tracing::debug!("Successfully linked audio to whip sink");
        }

        whipsink.sync_state_with_parent()?;
        demux.sync_state_with_parent()?;

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

        let video_queue_name = "video-queue-".to_string() + &id;
        let audio_queue_name = "audio-queue-".to_string() + &id;
        let whip_sink_name = "whip-sink-".to_string() + &id;

        // Try to remove branch from pipeline
        // Return Ok if branch is removed (or not exist)
        let remove_branch_from_pipeline = |pipeline: &Pipeline,
                                           queue_name: &str|
         -> Result<(), Error> {
            tracing::debug!("Removing queue {} from pipeline", queue_name);
            // Check if queue exists
            let queue = pipeline.by_name(queue_name);
            if queue.is_none() {
                tracing::warn!("Queue {} does not exist", queue_name);
                return Ok(());
            }

            let queue = queue.unwrap();
            let queue_sink_pad =
                queue
                    .static_pad("sink")
                    .ok_or(MyError::MissingElement(format!(
                        "{}'s sink pad",
                        queue_name
                    )))?;

            // Remove src pad from tee if queue is linked
            let name = queue_name.to_string();
            if queue_sink_pad.is_linked() {
                let tee_src_pad = queue_sink_pad
                    .peer()
                    .ok_or(MyError::MissingElement("tee's src pad".to_string()))?;

                let pipeline_weak = pipeline.downgrade();
                // Block tee's source pad with a pad probe.
                // the probe callback is called as soon as the pad becomes idle
                tee_src_pad.add_probe(gst::PadProbeType::IDLE, move |_pad, _info| {
                    let pipeline = match pipeline_weak.upgrade() {
                        Some(pipeline) => pipeline,
                        // drop pad if pipeline is already dropped
                        None => return gst::PadProbeReturn::Drop,
                    };

                    if queue.set_state(gst::State::Null).is_ok() && pipeline.remove(&queue).is_ok()
                    {
                        tracing::debug!("Queue {} is removed from pipeline", name);
                    } else {
                        tracing::error!("Failed to remove queue {} from pipeline", name);
                    }

                    // remove src pad afterwards
                    gst::PadProbeReturn::Remove
                });
            } else {
                return Err(MyError::FailedOperation(format!(
                    "Queue {} is not linked and can not be removed.",
                    name
                ))
                .into());
            }

            Ok(())
        };

        // TODO: check if the order would cause blocking issues when removing branches
        // TODO: pause the demux and resume it after removing branches
        remove_branch_from_pipeline(pipeline, &video_queue_name)?;
        remove_branch_from_pipeline(pipeline, &audio_queue_name)?;

        // Remove whip sink from pipeline
        // If whip sink fails to send offer, it is removed from
        // pipeline automatically (so no need to remove it again)
        if let Some(whip_sink) = pipeline.by_name(&whip_sink_name) {
            whip_sink.set_state(gst::State::Null).map_err(|_| {
                MyError::FailedOperation(format!("Failed to set {} to Null state", whip_sink_name))
            })?;

            pipeline.remove(&whip_sink).map_err(|_| {
                MyError::FailedOperation(format!(
                    "Failed to remove {} from pipeline",
                    whip_sink_name
                ))
            })?;
        }

        Ok(())
    }

    /// Setup pipeline
    /// # Arguments
    /// * `args` - Pipeline arguments
    ///
    /// Create a pipeline with the all needed elements and register callbacks for dynamic pads
    /// Link them together when the demux element generates all dynamic pads and start playing
    async fn init(&mut self, args: &Args) -> Result<(), Error> {
        // Initialize GStreamer (only once)
        gst::init()?;
        // Load whipsink
        gstwebrtchttp::plugin_register_static()?;
        tracing::debug!("Setting up pipeline");

        // Create a pipeline
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
        let audio_queue: gst::Element = gst::ElementFactory::make("queue")
            .name("audio-queue")
            .build()?;

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

        pipeline.add_many([
            &src,
            &input_tee,
            &whep_queue,
            &srt_queue,
            &typefind,
            &tsdemux,
            &video_queue,
            &audio_queue,
            &srtsink,
        ])?;
        gst::Element::link_many([&src, &input_tee])?;
        gst::Element::link_many([&input_tee, &whep_queue, &typefind, &tsdemux])?;
        gst::Element::link_many([&input_tee, &srt_queue, &srtsink])?;

        let pipeline_weak = pipeline.downgrade();
        // Connect to tsdemux's no-more-pads signal, that is emitted when the element
        // will not generate more dynamic pads.
        tsdemux.connect_no_more_pads(move |dbin| {
            tracing::info!("No more pads from the stream. Ready to link.");
            // Here we temporarily retrieve a strong reference on the pipeline from the weak one
            // we moved into this callback.
            let pipeline = match pipeline_weak.upgrade() {
                Some(pipeline) => pipeline,
                None => return,
            };

            let all_linked = dbin.foreach_src_pad(|_, pad| {
                let media_type = pad
                    .current_caps()
                    .and_then(|caps| caps.structure(0).map(|s| s.name()));
                if media_type.is_none() {
                    tracing::warn!("Failed to get media type from demux pad");
                    return false;
                }

                let media_type = media_type.unwrap().as_str();
                tracing::debug!("linking to media {:?}", media_type);

                let link_media = |pipeline: &Pipeline, media_type: &str| -> Result<(), Error> {
                    if media_type.starts_with("video/x-h264") {
                        // Create h264 video elements
                        let h264parse = gst::ElementFactory::make("h264parse").build()?;
                        let rtph264pay = gst::ElementFactory::make("rtph264pay").build()?;
                        let output_tee_video = gst::ElementFactory::make("tee")
                            .name("output_tee_video")
                            .build()?;

                        let video_elements =
                            &[&video_queue, &h264parse, &rtph264pay, &output_tee_video];
                        // 'video_queue' has been added to the pipeline already, so we don't add it again.
                        pipeline.add_many(&video_elements[1..])?;
                        gst::Element::link_many(&video_elements[..])?;
                        // This is quite important and people forget it often. Without making sure that
                        // the new elements have the same state as the pipeline, things will fail later.
                        // They would still be in Null state and can't process data.
                        for e in video_elements {
                            e.sync_state_with_parent()?;
                        }

                        Ok(())
                    } else if media_type.starts_with("video/x-h265") {
                        // Create h265 video elements
                        let h265parse = gst::ElementFactory::make("h265parse").build()?;
                        let rtph265pay = gst::ElementFactory::make("rtph265pay").build()?;
                        let output_tee_video = gst::ElementFactory::make("tee")
                            .name("output_tee_video")
                            .build()?;

                        let video_elements =
                            &[&video_queue, &h265parse, &rtph265pay, &output_tee_video];
                        // 'video_queue' has been added to the pipeline already, so we don't add it again.
                        pipeline.add_many(&video_elements[1..])?;
                        gst::Element::link_many(&video_elements[..])?;
                        for e in video_elements {
                            e.sync_state_with_parent()?;
                        }

                        Ok(())
                    } else if media_type.starts_with("audio") {
                        let aacparse = gst::ElementFactory::make("aacparse").build()?;
                        let avdec_aac = gst::ElementFactory::make("avdec_aac").build()?;
                        let audioconvert = gst::ElementFactory::make("audioconvert").build()?;
                        let audioresample = gst::ElementFactory::make("audioresample").build()?;
                        let opusenc = gst::ElementFactory::make("opusenc").build()?;
                        let rtpopuspay = gst::ElementFactory::make("rtpopuspay").build()?;
                        let output_tee_audio = gst::ElementFactory::make("tee")
                            .name("output_tee_audio")
                            .build()?;

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

                        // 'audio_queue' has been added to the pipeline already, so we don't add it again.
                        pipeline.add_many(&audio_elements[1..])?;
                        gst::Element::link_many(&audio_elements[..])?;
                        for e in audio_elements {
                            e.sync_state_with_parent()?;
                        }

                        Ok(())
                    } else {
                        Err(
                            MyError::FailedOperation(format!("Unknown media type {}", media_type))
                                .into(),
                        )
                    }
                };

                let linked = link_media(&pipeline, media_type).map_err(|err| {
                    tracing::error!("Failed to link media. {}", err);
                    err
                });

                linked.is_ok()
            });

            if all_linked {
                tracing::info!("Successfully linked stream. Ready to play.");
            } else {
                tracing::error!("Failed to link stream");
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
                        .ok_or(MyError::MissingElement("audio-queue".to_string()))?;
                    let sink_pad =
                        audio_queue
                            .static_pad("sink")
                            .ok_or(MyError::MissingElement(
                                "audio-queue's sink pad".to_string(),
                            ))?;
                    src_pad.link(&sink_pad)?;

                    tracing::info!("Successfully inserted audio sink");
                }
                if is_video {
                    // Get the queue element's sink pad and link the decodebin's newly created
                    // src pad for the video stream to it.
                    let video_queue = pipeline
                        .by_name("video-queue")
                        .ok_or(MyError::MissingElement("video-queue".to_string()))?;
                    let sink_pad =
                        video_queue
                            .static_pad("sink")
                            .ok_or(MyError::MissingElement(
                                "video-queue's sink pad".to_string(),
                            ))?;
                    src_pad.link(&sink_pad)?;

                    tracing::info!("Successfully inserted video sink");
                }

                Ok(())
            };

            if let Err(err) = insert_sink(is_audio, is_video) {
                tracing::error!("Failed to insert sink: {}", err);
            }
        });

        // Set to playing
        pipeline.set_state(gst::State::Playing)?;
        {
            self.lock_err().await?.pipeline = Some(pipeline);
        }

        Ok(())
    }

    /// Run pipeline and wait until the message bus receives an EOS or error message
    async fn run(&self) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state
            .pipeline
            .as_ref()
            .ok_or(MyError::FailedOperation(
                "Pipeline called before initialization".to_string(),
            ))?;
        let bus = pipeline.bus().unwrap();
        drop(pipeline_state);

        let main_loop = glib::MainLoop::new(None, false);
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
                    main_loop.quit();
                }
                MessageView::Error(err) => {
                    tracing::error!(
                        "{:?} runs into error : {} ({:?})",
                        err.src().map(|s| s.path_string()),
                        err.error(),
                        err.debug()
                    );
                    main_loop.quit();
                }
                _ => (),
            };

            // Tell the mainloop to continue executing this callback.
            glib::ControlFlow::Continue
        })?;

        // Operate GStreamer's bus, facilliating GLib's mainloop here.
        // This function call will block until you tell the mainloop to quit
        main_loop.run();

        Ok(())
    }

    /// Close pipeline by sending EOS message
    async fn end(&self) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        if let Some(pipeline) = pipeline_state.pipeline.as_ref() {
            pipeline.send_event(gst::event::Eos::new());
        }

        Ok(())
    }

    /// Clean up all elements in the pipeline and reset state
    async fn clean_up(&self) -> Result<(), Error> {
        let mut pipeline_state = self.lock_err().await?;
        if let Some(pipeline) = pipeline_state.pipeline.as_ref() {
            pipeline.set_state(gst::State::Null)?;
            pipeline_state.pipeline = None;
        }

        Ok(())
    }

    /// Helper function for debugging
    /// Print pipeline to dot file
    /// Warning: this function should not be called when the pipeline is in locked state
    async fn print(&self) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        if let Some(pipeline) = pipeline_state.pipeline.as_ref() {
            pipeline.debug_to_dot_file(DebugGraphDetails::all(), "pipeline");
        }

        Ok(())
    }
}
