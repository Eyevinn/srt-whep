use anyhow::{Error, Ok, Result};
use async_trait::async_trait;
use gst::{prelude::*, DebugGraphDetails, Pipeline};
use gstreamer as gst;
use gstrswebrtc::signaller::Signallable;
use gstrswebrtc::webrtcsink::WhipWebRTCSink;
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
    main_loop: Option<glib::MainLoop>,
    port: u32,
}

impl PipelineWrapper {
    fn new(args: Args) -> Self {
        Self {
            pipeline: None,
            main_loop: None,
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
    /// Check if SRT input stream is available
    async fn ready(&self) -> Result<bool, Error> {
        let pipeline_state = self.lock_err().await.inspect_err(|e| {
            tracing::error!("Failed to lock pipeline: {}", e);
        })?;
        let pipeline = pipeline_state.pipeline.as_ref();
        if pipeline.is_none() {
            tracing::error!("Pipeline is not missing");
            return Ok(false);
        }
        let pipeline = pipeline.unwrap();

        let demux = pipeline
            .by_name("demux")
            .ok_or(MyError::MissingElement("demux".to_string()))?;

        Ok(demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("video") || pad.name().starts_with("audio")))
    }

    /// Add connection to pipeline
    /// # Arguments
    /// * `id` - Connection id
    ///
    /// Based on the stream type (audio or video) of the connection, the corresponding branch is created
    /// For whipsink to work, the branch must be linked to the output tee element and synced in state
    /// Return NoSRTStream error if no input stream is available
    async fn add_connection(&self, id: String) -> Result<(), Error> {
        let ready = self.ready().await?;
        if !ready {
            tracing::error!("Demux has no pad available. No connection can be added.");
            return Err(
                MyError::FailedOperation("Pipeline not ready for connection".to_string()).into(),
            );
        }

        let pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state.pipeline.as_ref().unwrap();
        tracing::debug!("Add connection {} to pipeline", id);

        let demux = pipeline
            .by_name("demux")
            .ok_or(MyError::MissingElement("demux".to_string()))?;
        // WhipWebRTCSink is renamed as 'whipclientsink' since gst-plugin-webrtc version 0.13.0
        let whipsink = gst::ElementFactory::make("whipclientsink")
            .name("whip-sink-".to_string() + &id)
            .build()?;
        pipeline.add_many([&whipsink])?;
        if let Some(whipsink) = whipsink.dynamic_cast_ref::<WhipWebRTCSink>() {
            let signaller = whipsink.property::<Signallable>("signaller");
            signaller.set_property_from_str(
                "whip-endpoint",
                &format!("http://localhost:{}/whip_sink/{}", pipeline_state.port, id),
            );
        }

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

        // Remove video/audio branch from pipeline
        Self::remove_branch_from_pipeline(pipeline, &video_queue_name).await?;
        Self::remove_branch_from_pipeline(pipeline, &audio_queue_name).await?;

        // Remove whip sink from pipeline
        // If whip sink fails to send offer, it is removed from
        // pipeline automatically (so no need to remove it again)
        if let Some(whip_sink) = pipeline.by_name(&whip_sink_name) {
            tracing::debug!("Removing {} from pipeline", whip_sink_name);
            Self::remove_element_from_pipeline(pipeline, &whip_sink).await?;
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
        // Load webrtcsink plugin
        gstrswebrtc::plugin_register_static()?;
        tracing::debug!("Setting up pipeline");

        // Create a pipeline
        let pipeline = gst::Pipeline::default();

        let uri = format!(
            "srt://{}?mode={}",
            args.input_address,
            args.srt_mode.to_str()
        );
        let srt_mode = args.srt_mode.clone();
        tracing::info!("SRT Input uri: {}", uri);

        // Run discoverer if the source stream is in listener mode (we are the caller)
        if srt_mode == SRTMode::Caller && args.run_discoverer {
            tracing::info!("Running discoverer...");
            // Swallow error if discoverer fails (This could happen When SRT client is running in Docker container)
            let _ = run_discoverer(&uri, args.discoverer_timeout_sec);
        }

        let src = gst::ElementFactory::make("srtsrc")
            .name("srt_source")
            .property("uri", uri)
            .property("latency", 0)
            .build()?;
        let input_tee = gst::ElementFactory::make("tee").name("input_tee").build()?;

        let whep_queue = Self::create_custom_queue("whep-queue", "0", "0", "no")?;
        let typefind = gst::ElementFactory::make("typefind")
            .name("typefind")
            .build()?;
        let tsdemux = gst::ElementFactory::make("tsdemux")
            .name("demux")
            .property("latency", 0)
            .build()?;

        let video_queue = Self::create_custom_queue("video-queue", "0", "0", "no")?;
        let audio_queue = Self::create_custom_queue("audio-queue", "0", "0", "no")?;
        let srt_queue = Self::create_custom_queue("srt-queue", "0", "0", "downstream")?;

        let output_uri = format!(
            "srt://{}?mode={}",
            args.output_address,
            args.srt_mode.reverse().to_str()
        );
        tracing::info!("SRT Output uri: {}", output_uri);
        let srtsink = gst::ElementFactory::make("srtsink")
            .property("uri", output_uri)
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
        // will not generate more dynamic pads. This usually happens when the stream
        // is fully received and decoded.
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
                        let output_tee_video = gst::ElementFactory::make("tee")
                            .name("output_tee_video")
                            .build()?;
                        // Add a fakesink to the end of pipeline to consume buffers
                        // it receives and pops EOS to message bus when the SRT input stream is closed
                        let fakesink = gst::ElementFactory::make("fakesink")
                            .property("can-activate-pull", true)
                            .build()?;

                        let video_elements =
                            &[&video_queue, &h264parse, &output_tee_video, &fakesink];
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
                        tracing::warn!(
                            "H.265(HEVC) streams can be linked but are not fully supported yet"
                        );
                        // Create h265 video elements
                        let h265parse = gst::ElementFactory::make("h265parse").build()?;
                        let output_tee_video = gst::ElementFactory::make("tee")
                            .name("output_tee_video")
                            .build()?;
                        let fakesink = gst::ElementFactory::make("fakesink")
                            .property("can-activate-pull", true)
                            .build()?;

                        let video_elements =
                            &[&video_queue, &h265parse, &output_tee_video, &fakesink];
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
                        let output_tee_audio = gst::ElementFactory::make("tee")
                            .name("output_tee_audio")
                            .build()?;
                        let fakesink = gst::ElementFactory::make("fakesink")
                            .property("can-activate-pull", true)
                            .build()?;

                        let audio_elements = &[
                            &audio_queue,
                            &aacparse,
                            &avdec_aac,
                            &audioconvert,
                            &audioresample,
                            &opusenc,
                            &output_tee_audio,
                            &fakesink,
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
                    tracing::error!("Failed to link media: {}", err);
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
        let mut pipeline_state = self.lock_err().await?;
        let pipeline = pipeline_state
            .pipeline
            .as_ref()
            .ok_or(MyError::FailedOperation(
                "Pipeline called before initialization".to_string(),
            ))?;
        let bus = pipeline.bus().unwrap();
        let main_loop = glib::MainLoop::new(None, false);
        pipeline_state.main_loop = Some(main_loop.clone());
        drop(pipeline_state);

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
            tracing::debug!("Stopping pipeline");
            let result = pipeline.send_event(gst::event::Eos::new());
            if !result {
                tracing::error!("Failed to send EOS to pipeline");
            }
        } else {
            tracing::error!("Pipeline is missing");
        }

        Ok(())
    }

    /// Quit pipeline by sending a quit message to the main loop
    /// This function is used to restart the pipeline in case of
    /// unrecoverable errors
    async fn quit(&self) -> Result<(), Error> {
        let pipeline_state = self.lock_err().await?;
        if let Some(main_loop) = pipeline_state.main_loop.as_ref() {
            tracing::debug!("Force-quit pipeline");
            main_loop.quit();
        }

        Ok(())
    }

    /// Clean up all elements in the pipeline and reset state
    async fn clean_up(&self) -> Result<(), Error> {
        let mut pipeline_state = self.lock_err().await?;
        if let Some(pipeline) = pipeline_state.pipeline.as_ref() {
            pipeline
                .call_async_future(move |pipeline| {
                    let _ = pipeline.set_state(gst::State::Null).inspect_err(|e| {
                        tracing::error!("Failed to clean pipeline up: {}", e);
                    });
                })
                .await;
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
        } else {
            tracing::error!("Pipeline is missing");
        }

        Ok(())
    }
}

// Helper functions
impl SharablePipeline {
    /// Create a queue element with given name and properties
    /// To check if the queue is blocking, we connect to the overrun and underrun signals
    fn create_custom_queue(
        queue_name: &str,
        max_size_buffers: &str,
        max_size_time: &str,
        leaky: &str,
    ) -> Result<gst::Element, Error> {
        let queue = gst::ElementFactory::make("queue")
            .name(queue_name)
            .property_from_str("max-size-buffers", max_size_buffers)
            .property_from_str("max-size-time", max_size_time)
            .property_from_str("leaky", leaky)
            .build()?;

        queue.connect("overrun", false, {
            move |values: &[glib::Value]| {
                let queue = values[0].get::<gst::Element>().unwrap();
                tracing::debug!("{} is overrun", queue.name());
                None
            }
        });

        Ok(queue)
    }

    /// Remove element from a locked pipeline
    /// # Arguments
    /// * `pipeline` - Locked pipeline
    /// * `element` - Element to be removed
    ///
    /// To remove an element from a pipeline, one has to set the state of the element to NULL
    /// and remove it from the pipeline. This function MUST be called when the pipeline is in locked state
    async fn remove_element_from_pipeline(
        pipeline: &Pipeline,
        element: &gst::Element,
    ) -> Result<(), Error> {
        let pipeline_weak = pipeline.downgrade();
        // To set state to NULL from an async tokio context, one has to make use of gst::Element::call_async
        // and set the state to NULL from there, without blocking the runtime
        element
            .call_async_future(move |element| {
                // Here we temporarily retrieve a strong reference on the pipeline from the weak one
                // we moved into this callback.
                let pipeline = match pipeline_weak.upgrade() {
                    Some(pipeline) => pipeline,
                    None => return,
                };
                let _ = element.set_state(gst::State::Null).inspect_err(|e| {
                    tracing::error!("Failed to set {} to NULL: {}", element.name(), e)
                });

                if pipeline.remove(element).is_ok() {
                    tracing::debug!("{} is removed from pipeline", element.name());
                } else {
                    tracing::error!("Failed to remove {} from pipeline", element.name());
                }
            })
            .await;

        Ok(())
    }

    /// Remove branch from a locked pipeline
    /// # Arguments
    /// * `pipeline` - Locked pipeline
    /// * `queue_name` - Name of the queue element to be removed
    ///
    /// To remove a branch from a pipeline, one has to remove the src pad from the tee element
    /// and remove the queue element from the pipeline. This function MUST be called when the pipeline is in locked state
    async fn remove_branch_from_pipeline(
        pipeline: &Pipeline,
        queue_name: &str,
    ) -> Result<(), Error> {
        tracing::debug!("Removing {} from pipeline", queue_name);
        // Check if queue exists
        let queue = pipeline.by_name(queue_name);
        if queue.is_none() {
            tracing::warn!("{} does not exist", queue_name);
            return Ok(());
        }

        let queue = queue.unwrap();
        let queue_sink_pad = queue
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
            let tee = tee_src_pad
                .parent_element()
                .ok_or(MyError::MissingElement("output_tee".to_string()))?;

            // Pause tee before removing pad and resume afterward
            tee.call_async_future(move |tee| {
                let _ = tee.set_state(gst::State::Paused).inspect_err(|e| {
                    tracing::error!("Failed to pause tee: {}", e);
                });
                if tee.remove_pad(&tee_src_pad).is_ok() {
                    tracing::debug!("Pad is removed from tee");
                } else {
                    tracing::error!("Failed to remove Pad from tee");
                }
                let _ = tee.set_state(gst::State::Playing).inspect_err(|e| {
                    tracing::error!("Failed to resume tee: {}", e);
                });
            })
            .await;

            Self::remove_element_from_pipeline(pipeline, &queue).await?;
        } else {
            return Err(MyError::FailedOperation(format!(
                "Queue {} is not linked and can not be removed.",
                name
            ))
            .into());
        }

        Ok(())
    }
}
