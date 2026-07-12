use anyhow::{anyhow, Context, Error, Result};
use async_trait::async_trait;
use gst::{prelude::*, Pipeline};
use gstreamer as gst;
use std::sync::Arc;
use std::time::Duration;
use timed_locks::Mutex;
use tokio::sync::mpsc;

use crate::stream::branch::Branch;
use crate::stream::bus::{classify_bus_message, BusAction};
use crate::stream::egress;
use crate::stream::errors::PipelineError;
use crate::stream::naming::{self, BranchId};
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
    /// Present from birth (a constructor argument). The bus watch reports a
    /// per-viewer branch's runtime error here so the coordinator can reap that
    /// branch's connection instead of the error being merely logged. Lives on
    /// this wrapper, which survives pipeline reruns, so a supervisor restart
    /// keeps reaping without re-wiring.
    branch_failures: mpsc::Sender<BranchId>,
}

impl SharablePipeline {
    pub fn new(args: Args, branch_failures: mpsc::Sender<BranchId>) -> Self {
        Self {
            state: Arc::new(Mutex::new_with_timeout(
                PipelineWrapper::new(args),
                Duration::from_secs(1),
            )),
            branch_failures,
        }
    }

    /// Whether the input is demuxed and the matching output tees exist, so a
    /// branch can be linked. Pure check over an already-locked pipeline; the
    /// single source of truth for both `ready()` and `add_branch()`.
    fn input_ready(pipeline: &Pipeline) -> Result<bool, PipelineError> {
        let demux = pipeline.by_name(naming::DEMUX).ok_or_else(|| {
            PipelineError::Fatal(format!("Failed to find element: {}", naming::DEMUX))
        })?;

        let pads = demux.pads();
        let has_video = pads.iter().any(|pad| pad.name().starts_with("video"));
        let has_audio = pads.iter().any(|pad| pad.name().starts_with("audio"));
        if !has_video && !has_audio {
            return Ok(false);
        }

        // The demux exposes its media pads (pad-added) before the output tees
        // are built (no-more-pads -> link_media). A branch links onto those
        // tees, so the input is only truly ready once the matching tee exists.
        let video_ready = !has_video || pipeline.by_name(naming::OUTPUT_TEE_VIDEO).is_some();
        let audio_ready = !has_audio || pipeline.by_name(naming::OUTPUT_TEE_AUDIO).is_some();
        Ok(video_ready && audio_ready)
    }
}

#[async_trait]
impl BranchControl for SharablePipeline {
    /// Check if SRT input stream is available
    async fn ready(&self) -> Result<bool, PipelineError> {
        let pipeline_state = self.state.lock_err().await.inspect_err(|e| {
            tracing::error!("Failed to lock pipeline: {}", e);
        })?;
        let Some(pipeline) = pipeline_state.pipeline.as_ref() else {
            tracing::error!("Pipeline is not initialized");
            return Ok(false);
        };
        Self::input_ready(pipeline)
    }

    /// Add a viewer's branch to the pipeline
    /// # Arguments
    /// * `id` - Connection id
    ///
    /// Based on the stream type (audio or video) of the connection, the corresponding branch is created
    /// For whipsink to work, the branch must be linked to the output tee element and synced in state
    /// Return NoSRTStream error if no input stream is available
    async fn add_branch(&self, id: String) -> Result<(), PipelineError> {
        // Attach under the state lock (attach is synchronous and may hold it).
        // Clone the pipeline handle so that, if attach fails, we can detach the
        // half-built branch AFTER releasing the lock -- detach awaits GStreamer
        // state changes and must not run under the 1s timed state lock.
        let (pipeline, attach_result) = {
            let pipeline_state = self.state.lock_err().await?;
            // No pipeline means we are between supervisor restarts: retryable.
            let pipeline = pipeline_state
                .pipeline
                .as_ref()
                .ok_or(PipelineError::NotReady)?;

            if !Self::input_ready(pipeline)? {
                tracing::error!("Demux has no pad available. No connection can be added.");
                return Err(PipelineError::NotReady); // pre-attach: nothing to clean up
            }

            tracing::debug!("Add connection {} to pipeline", id);
            let attach_result = Branch::for_id(&id).attach(
                pipeline,
                pipeline_state.args.port,
                pipeline_state.args.decode_video,
            );
            (pipeline.clone(), attach_result)
        };

        if let Err(attach_err) = attach_result {
            // Attach ran partway: best-effort detach of our own half-built
            // branch so the caller never has to reason about stream-plane
            // cleanup (ADR 0002 -- the semantic is unchanged; only the location
            // moved here from the coordinator). detach stops at the first queue
            // that was added but never linked (remove_branch_from_pipeline
            // errors on an unlinked queue), so a partial attach can leave
            // elements behind; they are untracked (the id never entered the
            // connection map) and are cleared on the next pipeline restart. The
            // original attach error is what we report.
            tracing::warn!(
                "attach for {} failed ({}); detaching half-built branch",
                id,
                attach_err
            );
            if let Err(cleanup_err) = Branch::for_id(&id).detach(&pipeline).await {
                tracing::error!(
                    "cleanup after failed attach for {} also failed: {}",
                    id,
                    cleanup_err
                );
            }
            return Err(PipelineError::Fatal(attach_err.to_string()));
        }
        Ok(())
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
        // The WHIP sink (whipclientsink) comes from the rswebrtc plugin that the
        // GStreamer installation provides, discovered on the plugin path. We do
        // NOT statically register a crate-pinned copy here: that copy is built
        // against a fixed (older) GStreamer and would shadow the installed
        // plugin, breaking the WebRTC RTP path against a newer runtime.
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
            .name(naming::SRT_SOURCE)
            .property("uri", uri)
            .property("latency", args.srt_latency as i32)
            .build()?;
        let input_tee = gst::ElementFactory::make("tee").name("input_tee").build()?;

        let whep_queue = Self::create_custom_queue("whep-queue", "0", "0", "no")?;
        let typefind = gst::ElementFactory::make("typefind")
            .name("typefind")
            .build()?;
        let tsdemux = gst::ElementFactory::make("tsdemux")
            .name(naming::DEMUX)
            .property("latency", args.tsdemux_latency as i32)
            .build()?;

        let video_queue = Self::create_custom_queue(naming::VIDEO_QUEUE, "0", "0", "no")?;
        let audio_queue = Self::create_custom_queue(naming::AUDIO_QUEUE, "0", "0", "no")?;
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

                let linked =
                    egress::build_egress_chain(&pipeline, media_type, &video_queue, &audio_queue)
                        .map_err(|err| {
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
                    let audio_queue = pipeline.by_name(naming::AUDIO_QUEUE).with_context(|| {
                        format!("Failed to find element: {}", naming::AUDIO_QUEUE)
                    })?;
                    let sink_pad = audio_queue.static_pad("sink").with_context(|| {
                        format!("Failed to find element: {}'s sink pad", naming::AUDIO_QUEUE)
                    })?;
                    src_pad.link(&sink_pad)?;

                    tracing::info!("Successfully inserted audio sink");
                }
                if is_video {
                    // Get the queue element's sink pad and link the decodebin's newly created
                    // src pad for the video stream to it.
                    let video_queue = pipeline.by_name(naming::VIDEO_QUEUE).with_context(|| {
                        format!("Failed to find element: {}", naming::VIDEO_QUEUE)
                    })?;
                    let sink_pad = video_queue.static_pad("sink").with_context(|| {
                        format!("Failed to find element: {}'s sink pad", naming::VIDEO_QUEUE)
                    })?;
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
                .context("Pipeline called before initialization")?;
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
            // The quit-vs-reap-vs-ignore decision (ADR 0002's containment
            // scope) lives in `bus::classify_bus_message`; this closure only
            // executes the chosen action and logs the message it holds.
            match classify_bus_message(msg) {
                BusAction::Quit => {
                    match msg.view() {
                        MessageView::Eos(..) => tracing::info!("received eos"),
                        MessageView::Error(err) => tracing::error!(
                            "{:?} runs into error : {} ({:?})",
                            err.src().map(|s| s.path_string()),
                            err.error(),
                            err.debug()
                        ),
                        _ => (),
                    }
                    main_loop.quit();
                }
                BusAction::ReapBranch(id) => {
                    if let MessageView::Error(err) = msg.view() {
                        tracing::warn!(
                            "Branch for {} errored; reaping it, pipeline stays up: {} ({:?})",
                            id.as_str(),
                            err.error(),
                            err.debug()
                        );
                    }
                    // Fire-and-forget: the coordinator removes the branch and
                    // drops the connection. `try_send` never blocks the GLib
                    // loop thread; a full/closed channel just means the reap is
                    // dropped (the sweep/DELETE remain a backstop).
                    if branch_failures.try_send(id.clone()).is_err() {
                        tracing::warn!(
                            "Could not signal coordinator to reap branch {}",
                            id.as_str()
                        );
                    }
                }
                BusAction::Ignore => (),
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

        done_rx
            .await
            .map_err(|_| anyhow!("GLib main loop thread died unexpectedly"))??;

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
