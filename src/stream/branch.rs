//! One viewer's per-connection pipeline elements — "the Branch" — AND the
//! entire loopback-WHIP bridge.
//!
//! Everything here exists only because egress uses a WHIP *client*
//! (`whipclientsink`): the loopback route template ([`WHIP_SINK_ROUTE`] /
//! [`whip_sink_path`]), the endpoint URL the sink POSTs its offer to
//! (`whip_endpoint`), and the attach/detach of that sink. The listener↔pipeline
//! port coupling asserted in `startup::Application::assemble` exists for the
//! same reason.
//!
//! This module is the deletion boundary for the `whepserversink` migration
//! (ADR 0001, Future Work): moving egress to the native server-initiated
//! `whepserversink` removes this bridge wholesale. Keep loopback-specific
//! surface confined here so that migration stays a clean deletion.
//!
//! `startup.rs` imports [`WHIP_SINK_ROUTE`] and the WHIP handler imports
//! [`whip_sink_path`], so the HTTP contract and the whipclientsink's endpoint
//! can never drift apart.
use anyhow::{Context, Error, Result};
use gst::prelude::*;
use gstreamer as gst;

use crate::stream::naming;

/// The actix route template for the loopback WHIP endpoint — the single
/// definition shared by the HTTP route table, the WHIP Location header,
/// and the whipclientsink's endpoint URL.
pub const WHIP_SINK_ROUTE: &str = "/whip_sink/{id}";

/// Path of one connection's WHIP resource (the route template, instantiated).
pub fn whip_sink_path(id: &str) -> String {
    WHIP_SINK_ROUTE.replace("{id}", id)
}

/// The loopback URL a connection's whipclientsink POSTs its offer to.
fn whip_endpoint(port: u16, id: &str) -> String {
    format!("http://localhost:{}{}", port, whip_sink_path(id))
}

/// Handle on one connection's branch, keyed by connection id. Cheap to
/// construct; element lookups happen by derived name so a branch can be
/// detached even when its attach half-failed.
pub(crate) struct Branch {
    id: String,
}

impl Branch {
    pub(crate) fn for_id(id: &str) -> Self {
        Self { id: id.to_string() }
    }

    fn whip_sink_name(&self) -> String {
        naming::whip_sink_name(&self.id)
    }

    fn video_queue_name(&self) -> String {
        naming::video_queue_name(&self.id)
    }

    fn audio_queue_name(&self) -> String {
        naming::audio_queue_name(&self.id)
    }

    fn video_decoder_name(&self) -> String {
        naming::video_decoder_name(&self.id)
    }

    /// Create this viewer's whipclientsink and per-media queues, link them
    /// onto the pipeline's output tees, and sync their states.
    ///
    /// When `decode_video` is set, an `avdec_h264` is inserted between the
    /// video queue and the whip sink so whipclientsink receives raw video and
    /// re-encodes it internally. This works around a caps-negotiation bug in
    /// webrtcsink 0.15.x on macOS, where H264 passthrough fails with
    /// not-negotiated on `GstAppSrc:video_0` (see `--decode-video`).
    ///
    /// On error, attach does NOT undo its own work: elements it already
    /// added (whip sink, queues, decoder) stay in the pipeline. Call
    /// [`Self::detach`] to clean up — it removes everything this branch put
    /// in the pipeline, however far attach got.
    ///
    /// Synchronous GStreamer calls only; the caller may hold the pipeline
    /// state lock.
    pub(crate) fn attach(
        &self,
        pipeline: &gst::Pipeline,
        port: u16,
        decode_video: bool,
    ) -> Result<(), Error> {
        let demux = pipeline
            .by_name(naming::DEMUX)
            .with_context(|| format!("Failed to find element: {}", naming::DEMUX))?;
        // WhipWebRTCSink is renamed as 'whipclientsink' since gst-plugin-webrtc version 0.13.0
        let whipsink = gst::ElementFactory::make("whipclientsink")
            .name(self.whip_sink_name())
            .build()?;
        pipeline.add_many([&whipsink])?;
        // Point this connection's WHIP signaller at the in-process loopback
        // endpoint. We reach it as a plain GObject property rather than through
        // the webrtcsink crate's Rust types: those types are version-locked to a
        // specific GStreamer release, and binding them into this binary let a
        // stale statically-registered copy shadow the installed rswebrtc plugin.
        // The "signaller" object and its "whip-endpoint" property are part of the
        // element's stable API, so this works against whichever plugin version
        // the GStreamer installation provides.
        if whipsink.find_property("signaller").is_some() {
            let signaller = whipsink.property::<gst::glib::Object>("signaller");
            signaller.set_property_from_str("whip-endpoint", &whip_endpoint(port, &self.id));
        }

        if demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("video"))
        {
            let output_tee_video = pipeline
                .by_name(naming::OUTPUT_TEE_VIDEO)
                .with_context(|| format!("Failed to find element: {}", naming::OUTPUT_TEE_VIDEO))?;
            let queue_video: gst::Element = gst::ElementFactory::make("queue")
                .name(self.video_queue_name())
                .build()?;
            pipeline.add_many([&queue_video])?;

            if decode_video {
                let decoder = gst::ElementFactory::make("avdec_h264")
                    .name(self.video_decoder_name())
                    .build()?;
                pipeline.add_many([&decoder])?;
                gst::Element::link_many([&output_tee_video, &queue_video, &decoder, &whipsink])?;

                let video_elements = &[&output_tee_video, &queue_video, &decoder];
                for e in video_elements {
                    e.sync_state_with_parent()?;
                }
            } else {
                gst::Element::link_many([&output_tee_video, &queue_video, &whipsink])?;

                let video_elements = &[&output_tee_video, &queue_video];
                for e in video_elements {
                    e.sync_state_with_parent()?;
                }
            }

            tracing::debug!("Successfully linked video to whip sink");
        }

        if demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("audio"))
        {
            let output_tee_audio = pipeline
                .by_name(naming::OUTPUT_TEE_AUDIO)
                .with_context(|| format!("Failed to find element: {}", naming::OUTPUT_TEE_AUDIO))?;
            let queue_audio: gst::Element = gst::ElementFactory::make("queue")
                .name(self.audio_queue_name())
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

    /// Tear this viewer's branch down: remove the per-media queues via the
    /// tee pad-probe dance, then the optional decoder and the whip sink.
    ///
    /// Tolerant of partial attach state: removes every element this branch
    /// put in the pipeline, however far [`Self::attach`] got. Elements that
    /// were never created are skipped; a queue that was added but never
    /// linked is removed directly (there is no tee pad to release).
    ///
    /// Awaits GStreamer state changes; the caller must NOT hold the
    /// pipeline state lock.
    pub(crate) async fn detach(&self, pipeline: &gst::Pipeline) -> Result<(), Error> {
        // Remove video/audio branch from pipeline
        Self::remove_branch_from_pipeline(pipeline, &self.video_queue_name()).await?;
        Self::remove_branch_from_pipeline(pipeline, &self.audio_queue_name()).await?;

        // Remove the optional H264 decoder if this branch was attached with
        // --decode-video. Presence-by-name, so teardown needs no extra flag.
        let decoder_name = self.video_decoder_name();
        if let Some(decoder) = pipeline.by_name(&decoder_name) {
            tracing::debug!("Removing {} from pipeline", decoder_name);
            Self::remove_element_from_pipeline(pipeline, &decoder).await?;
        }

        // Remove whip sink from pipeline
        // If whip sink fails to send offer, it is removed from
        // pipeline automatically (so no need to remove it again)
        let whip_sink_name = self.whip_sink_name();
        if let Some(whip_sink) = pipeline.by_name(&whip_sink_name) {
            tracing::debug!("Removing {} from pipeline", whip_sink_name);
            Self::remove_element_from_pipeline(pipeline, &whip_sink).await?;
        }

        Ok(())
    }

    /// Remove element from a pipeline
    /// # Arguments
    /// * `pipeline` - Pipeline
    /// * `element` - Element to be removed
    ///
    /// To remove an element from a pipeline, one has to set the state of the element to NULL
    /// and remove it from the pipeline.
    async fn remove_element_from_pipeline(
        pipeline: &gst::Pipeline,
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

    /// Remove one per-media branch queue from a pipeline
    /// # Arguments
    /// * `pipeline` - Pipeline
    /// * `queue_name` - Name of the queue element to be removed
    ///
    /// To remove a branch from a pipeline, one has to remove the src pad from the tee element
    /// and remove the queue element from the pipeline. A queue that exists but
    /// was never linked (a partial attach) has no tee pad and is removed directly.
    async fn remove_branch_from_pipeline(
        pipeline: &gst::Pipeline,
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
            .with_context(|| format!("Failed to find element: {}'s sink pad", queue_name))?;

        // Remove src pad from tee if queue is linked
        if queue_sink_pad.is_linked() {
            let tee_src_pad = queue_sink_pad
                .peer()
                .context("Failed to find element: tee's src pad")?;
            let tee = tee_src_pad
                .parent_element()
                .context("Failed to find element: output_tee")?;

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
            // Added but never linked: a partial attach got this far and no
            // further. There is no tee pad to release — remove the element
            // directly so detach cleans everything it can reach.
            tracing::debug!("{} was never linked; removing directly", queue_name);
            Self::remove_element_from_pipeline(pipeline, &queue).await?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_paths_derive_from_the_route_template() {
        assert_eq!("/whip_sink/abc", whip_sink_path("abc"));
        assert_eq!(
            "http://localhost:8000/whip_sink/abc",
            whip_endpoint(8000, "abc")
        );
    }

    #[tokio::test]
    async fn detach_after_a_partial_attach_removes_everything_it_can_reach() {
        gst::init().unwrap();
        let pipeline = gst::Pipeline::new();
        let branch = Branch::for_id("t1");
        // Simulate an attach that added its elements but died before linking:
        // detach finds everything by derived name, so stand-in factories are
        // fine — what matters is that the queue exists and is unlinked.
        let queue = gst::ElementFactory::make("queue")
            .name(branch.video_queue_name())
            .build()
            .unwrap();
        let decoder = gst::ElementFactory::make("identity")
            .name(branch.video_decoder_name())
            .build()
            .unwrap();
        let sink = gst::ElementFactory::make("fakesink")
            .name(branch.whip_sink_name())
            .build()
            .unwrap();
        pipeline.add_many([&queue, &decoder, &sink]).unwrap();

        branch
            .detach(&pipeline)
            .await
            .expect("detach must tolerate partial attach state");

        for name in [
            branch.video_queue_name(),
            branch.video_decoder_name(),
            branch.whip_sink_name(),
        ] {
            assert!(
                pipeline.by_name(&name).is_none(),
                "{name} must be removed by detach"
            );
        }
    }
}
