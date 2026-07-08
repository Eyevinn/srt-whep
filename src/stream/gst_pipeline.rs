use anyhow::{Error, Result};
use async_trait::async_trait;
use gst::{prelude::*, Pipeline};
use gstreamer as gst;
use std::sync::Arc;
use std::time::Duration;
use timed_locks::Mutex;
use tokio::sync::mpsc;

use crate::stream::branch::{branch_id_from_name, Branch};
use crate::stream::errors::{PipelineError, StreamError};
use crate::stream::pipeline::{Args, BranchControl, PipelineLifecycle, SRTMode};
use crate::stream::utils::run_discoverer;

#[derive(Clone)]
struct PipelineWrapper {
    pipeline: Option<Pipeline>,
    main_loop: Option<glib::MainLoop>,
    args: Args,
}

impl PipelineWrapper {
    fn new(args: Args) -> Self {
        Self {
            pipeline: None,
            main_loop: None,
            args,
        }
    }
}

#[derive(Clone)]
pub struct SharablePipeline {
    state: Arc<Mutex<PipelineWrapper>>,
    /// Set once at wiring time (by the coordinator). The bus watch reports a
    /// per-viewer branch's runtime error here so the coordinator can reap
    /// that branch's connection instead of the error being merely logged.
    branch_failures: Arc<std::sync::Mutex<Option<mpsc::Sender<String>>>>,
}

impl SharablePipeline {
    pub fn new(args: Args) -> Self {
        Self {
            state: Arc::new(Mutex::new_with_timeout(
                PipelineWrapper::new(args),
                Duration::from_secs(1),
            )),
            branch_failures: Arc::new(std::sync::Mutex::new(None)),
        }
    }
}

#[async_trait]
impl BranchControl for SharablePipeline {
    /// Check if SRT input stream is available
    async fn ready(&self) -> Result<bool, PipelineError> {
        let pipeline_state = self.state.lock_err().await.inspect_err(|e| {
            tracing::error!("Failed to lock pipeline: {}", e);
        })?;
        let pipeline = pipeline_state.pipeline.as_ref();
        if pipeline.is_none() {
            tracing::error!("Pipeline is not initialized");
            return Ok(false);
        }
        let pipeline = pipeline.unwrap();

        let demux = pipeline
            .by_name("demux")
            .ok_or_else(|| PipelineError::Fatal("Failed to find element: demux".to_string()))?;

        let pads = demux.pads();
        let has_video = pads.iter().any(|pad| pad.name().starts_with("video"));
        let has_audio = pads.iter().any(|pad| pad.name().starts_with("audio"));
        if !has_video && !has_audio {
            return Ok(false);
        }

        // The demux exposes its media pads (pad-added) before the output tees are
        // built (no-more-pads -> link_media). add_connection links each new WHEP
        // branch onto those tees, so the input is only truly ready to accept a
        // connection once the matching tee exists. Reporting ready earlier lets a
        // WHEP POST race pad creation and fail with a spurious 500 (and leak a
        // half-added whipsink); gating on the tees turns that window into a
        // retriable NotReady instead.
        let video_ready = !has_video || pipeline.by_name("output_tee_video").is_some();
        let audio_ready = !has_audio || pipeline.by_name("output_tee_audio").is_some();
        Ok(video_ready && audio_ready)
    }

    /// Add a viewer's branch to the pipeline
    /// # Arguments
    /// * `id` - Connection id
    ///
    /// Based on the stream type (audio or video) of the connection, the corresponding branch is created
    /// For whipsink to work, the branch must be linked to the output tee element and synced in state
    /// Return NoSRTStream error if no input stream is available
    async fn add_branch(&self, id: String) -> Result<(), PipelineError> {
        let ready = self.ready().await?;
        if !ready {
            tracing::error!("Demux has no pad available. No connection can be added.");
            return Err(PipelineError::NotReady);
        }

        let pipeline_state = self.state.lock_err().await?;
        // No pipeline means we are between supervisor restarts: retryable.
        let pipeline = pipeline_state
            .pipeline
            .as_ref()
            .ok_or(PipelineError::NotReady)?;
        tracing::debug!("Add connection {} to pipeline", id);

        Branch::for_id(&id)
            .attach(pipeline, pipeline_state.args.port)
            .map_err(|e| PipelineError::Fatal(e.to_string()))
    }

    /// Remove a viewer's branch from the pipeline
    /// # Arguments
    /// * `id` - Connection id (must exist in the pipeline)
    ///
    /// Based on the id, we find the corresponding branch (video or audio) and this branch could
    /// be linked to the output tee element or not (audio queue is not linked when stream contains no audio)
    /// If the branch is linked, we block the tee's source pad with a pad probe and remove the branch in the callback
    /// If the branch is not linked, we remove the branch directly
    /// After removing the branch, we remove the whip sink from the pipeline
    async fn remove_branch(&self, id: String) -> Result<(), PipelineError> {
        // Snapshot the pipeline handle (a cheap GObject ref) and release the
        // state lock: the teardown below awaits GStreamer state changes, and
        // holding the 1s timed lock across those awaits surfaces a slow
        // teardown as spurious LockTimeout errors to every other caller.
        let pipeline = {
            let pipeline_state = self.state.lock_err().await?;
            pipeline_state
                .pipeline
                .as_ref()
                // Between supervisor restarts the branch is already gone;
                // a retry will resolve to NotFound at the coordinator.
                .ok_or_else(|| PipelineError::Transient("Pipeline is not initialized".to_string()))?
                .clone()
        };
        tracing::debug!("Remove connection {} from pipeline", id);

        Branch::for_id(&id)
            .detach(&pipeline)
            .await
            .map_err(|e| PipelineError::Fatal(e.to_string()))
    }

    /// Quit pipeline by sending a quit message to the main loop
    /// This function is used to restart the pipeline in case of
    /// unrecoverable errors
    async fn quit(&self) -> Result<(), PipelineError> {
        let pipeline_state = self.state.lock_err().await?;
        if let Some(main_loop) = pipeline_state.main_loop.as_ref() {
            tracing::debug!("Force-quit pipeline");
            main_loop.quit();
        }

        Ok(())
    }

    fn set_branch_failure_sink(&self, sink: mpsc::Sender<String>) {
        *self.branch_failures.lock().unwrap() = Some(sink);
    }
}

#[async_trait]
impl PipelineLifecycle for SharablePipeline {
    /// Setup pipeline
    ///
    /// Create a pipeline with the all needed elements and register callbacks for dynamic pads
    /// Link them together when the demux element generates all dynamic pads and start playing
    async fn init(&self) -> Result<(), Error> {
        // Initialize GStreamer (only once)
        gst::init()?;
        // Load webrtcsink plugin
        gstrswebrtc::plugin_register_static()?;
        tracing::debug!("Setting up pipeline");

        let args = self.state.lock_err().await?.args.clone();

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
            .property("latency", args.srt_latency as i32)
            .build()?;
        let input_tee = gst::ElementFactory::make("tee").name("input_tee").build()?;

        let whep_queue = Self::create_custom_queue("whep-queue", "0", "0", "no")?;
        let typefind = gst::ElementFactory::make("typefind")
            .name("typefind")
            .build()?;
        let tsdemux = gst::ElementFactory::make("tsdemux")
            .name("demux")
            .property("latency", args.tsdemux_latency as i32)
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
                    // Codec table: the video arms differ only in which
                    // parser element sits between the queue and the tee.
                    let video_parser = if media_type.starts_with("video/x-h264") {
                        Some("h264parse")
                    } else if media_type.starts_with("video/x-h265") {
                        tracing::warn!(
                            "H.265(HEVC) streams can be linked but are not fully supported yet"
                        );
                        Some("h265parse")
                    } else {
                        None
                    };

                    if let Some(parser) = video_parser {
                        let parse = gst::ElementFactory::make(parser).build()?;
                        let output_tee_video = gst::ElementFactory::make("tee")
                            .name("output_tee_video")
                            .build()?;
                        // Add a fakesink to the end of pipeline to consume buffers
                        // it receives and pops EOS to message bus when the SRT input stream is closed
                        let fakesink = gst::ElementFactory::make("fakesink")
                            .property("can-activate-pull", true)
                            .build()?;

                        let video_elements = &[&video_queue, &parse, &output_tee_video, &fakesink];
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
                        Err(StreamError::FailedOperation(format!(
                            "Unknown media type {}",
                            media_type
                        ))
                        .into())
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
                        .ok_or(StreamError::MissingElement("audio-queue".to_string()))?;
                    let sink_pad =
                        audio_queue
                            .static_pad("sink")
                            .ok_or(StreamError::MissingElement(
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
                        .ok_or(StreamError::MissingElement("video-queue".to_string()))?;
                    let sink_pad =
                        video_queue
                            .static_pad("sink")
                            .ok_or(StreamError::MissingElement(
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
            self.state.lock_err().await?.pipeline = Some(pipeline);
        }

        Ok(())
    }

    /// Run pipeline and wait until the message bus receives an EOS or error message
    async fn run(&self) -> Result<(), Error> {
        let (bus, main_loop) = {
            let mut pipeline_state = self.state.lock_err().await?;
            let pipeline = pipeline_state
                .pipeline
                .as_ref()
                .ok_or(StreamError::FailedOperation(
                    "Pipeline called before initialization".to_string(),
                ))?;
            let bus = pipeline.bus().unwrap();
            let main_loop = glib::MainLoop::new(None, false);
            pipeline_state.main_loop = Some(main_loop.clone());
            (bus, main_loop)
        };

        // Wait until an EOS or error message appears
        let main_loop_clone = main_loop.clone();
        let branch_failures = self.branch_failures.clone();
        let bus_watch = move |_: &gst::Bus, msg: &gst::Message| {
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
                    // An error from a WHEP output branch (a `whip-sink-*`/`*-queue-*`
                    // element or anything nested inside it — e.g. its signaller timing
                    // out or its peer going away) must not be fatal. Quitting the main
                    // loop here would drop the SRT ingest and every other viewer, and
                    // the ensuing supervisor restart would reset all in-flight
                    // handshakes — the "wedge" a single bad peer must never be able to
                    // cause. Instead we walk the error source's ancestry to find which
                    // viewer's branch it belongs to and ask the coordinator to reap
                    // that one connection, leaving the pipeline running.
                    let src = err.src();
                    let mut branch_id: Option<String> = None;
                    let mut cursor = src.cloned();
                    while let Some(obj) = cursor {
                        if let Some(id) = branch_id_from_name(obj.name().as_str()) {
                            branch_id = Some(id.to_string());
                            break;
                        }
                        cursor = obj.parent();
                    }

                    if let Some(id) = branch_id {
                        tracing::warn!(
                            "Branch for {} errored; reaping it, pipeline stays up: {} ({:?})",
                            id,
                            err.error(),
                            err.debug()
                        );
                        // Fire-and-forget: the coordinator removes the branch and
                        // drops the connection. `try_send` never blocks the GLib
                        // loop thread; a full/closed channel just means the reap is
                        // dropped (the sweep/DELETE remain a backstop).
                        if let Some(sink) = branch_failures.lock().unwrap().clone() {
                            if sink.try_send(id.clone()).is_err() {
                                tracing::warn!(
                                    "Could not signal coordinator to reap branch {}",
                                    id
                                );
                            }
                        }
                    } else {
                        tracing::error!(
                            "{:?} runs into error : {} ({:?})",
                            src.as_ref().map(|s| s.path_string()),
                            err.error(),
                            err.debug()
                        );
                        main_loop.quit();
                    }
                }
                _ => (),
            };

            // Tell the mainloop to continue executing this callback.
            glib::ControlFlow::Continue
        };

        // The GLib main loop is synchronous: parking it on a tokio worker
        // starves the runtime (the documented e2e hang on current_thread
        // runtimes). It gets its own named OS thread instead, and this
        // async fn just awaits the loop's completion signal.
        let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<(), Error>>();
        std::thread::Builder::new()
            .name("gst-main-loop".to_string())
            .spawn(move || match bus.add_watch(bus_watch) {
                Ok(_watch_guard) => {
                    // Blocks until EOS/fatal error/quit; the watch guard
                    // must live exactly as long as the loop runs.
                    main_loop.run();
                    let _ = done_tx.send(Ok(()));
                }
                Err(e) => {
                    let _ = done_tx.send(Err(e.into()));
                }
            })?;

        done_rx.await.map_err(|_| {
            StreamError::FailedOperation("GLib main loop thread died unexpectedly".to_string())
        })??;

        Ok(())
    }

    /// Close pipeline by sending EOS message
    async fn end(&self) -> Result<(), Error> {
        let pipeline_state = self.state.lock_err().await?;
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

    /// Clean up all elements in the pipeline and reset state
    async fn clean_up(&self) -> Result<(), Error> {
        // Take the pipeline out under the lock, then do the async NULL
        // transition without holding it.
        let pipeline = {
            let mut pipeline_state = self.state.lock_err().await?;
            pipeline_state.main_loop = None;
            pipeline_state.pipeline.take()
        };
        if let Some(pipeline) = pipeline {
            pipeline
                .call_async_future(move |pipeline| {
                    let _ = pipeline.set_state(gst::State::Null).inspect_err(|e| {
                        tracing::error!("Failed to clean pipeline up: {}", e);
                    });
                })
                .await;
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
}
