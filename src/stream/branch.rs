//! One viewer's per-connection pipeline elements — "the Branch".
//!
//! This module is the only place that knows the per-connection element
//! name conventions, the loopback WHIP URL/route template, how a branch
//! is linked onto the output tees, and the pad-probe teardown dance.
//! `startup.rs` imports [`WHIP_SINK_ROUTE`] and the WHIP handler imports
//! [`whip_sink_path`], so the HTTP contract and the whipclientsink's
//! endpoint can never drift apart.
use anyhow::{Error, Result};
use gst::prelude::*;
use gstreamer as gst;
use gstrswebrtc::signaller::Signallable;
use gstrswebrtc::webrtcsink::WhipWebRTCSink;

use crate::stream::errors::StreamError;

/// The actix route template for the loopback WHIP endpoint — the single
/// definition shared by the HTTP route table, the WHIP Location header,
/// and the whipclientsink's endpoint URL.
pub const WHIP_SINK_ROUTE: &str = "/whip_sink/{id}";

/// Path of one connection's WHIP resource (the route template, instantiated).
pub fn whip_sink_path(id: &str) -> String {
    WHIP_SINK_ROUTE.replace("{id}", id)
}

/// The loopback URL a connection's whipclientsink POSTs its offer to.
fn whip_endpoint(port: u32, id: &str) -> String {
    format!("http://localhost:{}{}", port, whip_sink_path(id))
}

const WHIP_SINK_PREFIX: &str = "whip-sink-";
const VIDEO_QUEUE_PREFIX: &str = "video-queue-";
const AUDIO_QUEUE_PREFIX: &str = "audio-queue-";

/// If `name` is a per-viewer branch element, return the connection id it
/// belongs to. Recognizes the whip sink AND the per-media queues. The core
/// demux→tee chain's queues are named exactly `video-queue`/`audio-queue`
/// (no id suffix), so the trailing dash in the prefixes keeps their errors
/// out — a core-queue error must stay fatal.
///
/// The bus watch uses this to contain a dying branch's errors to that branch
/// (reaping just that connection) instead of quitting the whole pipeline,
/// which would drop the SRT ingest and every other viewer.
pub(crate) fn branch_id_from_name(name: &str) -> Option<&str> {
    name.strip_prefix(WHIP_SINK_PREFIX)
        .or_else(|| name.strip_prefix(VIDEO_QUEUE_PREFIX))
        .or_else(|| name.strip_prefix(AUDIO_QUEUE_PREFIX))
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
        format!("{}{}", WHIP_SINK_PREFIX, self.id)
    }

    fn video_queue_name(&self) -> String {
        format!("{}{}", VIDEO_QUEUE_PREFIX, self.id)
    }

    fn audio_queue_name(&self) -> String {
        format!("{}{}", AUDIO_QUEUE_PREFIX, self.id)
    }

    /// Create this viewer's whipclientsink and per-media queues, link them
    /// onto the pipeline's output tees, and sync their states.
    ///
    /// Synchronous GStreamer calls only; the caller may hold the pipeline
    /// state lock.
    pub(crate) fn attach(&self, pipeline: &gst::Pipeline, port: u32) -> Result<(), Error> {
        let demux = pipeline
            .by_name("demux")
            .ok_or(StreamError::MissingElement("demux".to_string()))?;
        // WhipWebRTCSink is renamed as 'whipclientsink' since gst-plugin-webrtc version 0.13.0
        let whipsink = gst::ElementFactory::make("whipclientsink")
            .name(self.whip_sink_name())
            .build()?;
        pipeline.add_many([&whipsink])?;
        if let Some(whipsink) = whipsink.dynamic_cast_ref::<WhipWebRTCSink>() {
            let signaller = whipsink.property::<Signallable>("signaller");
            signaller.set_property_from_str("whip-endpoint", &whip_endpoint(port, &self.id));
        }

        if demux
            .pads()
            .into_iter()
            .any(|pad| pad.name().starts_with("video"))
        {
            let output_tee_video = pipeline
                .by_name("output_tee_video")
                .ok_or(StreamError::MissingElement("output_tee_video".to_string()))?;
            let queue_video: gst::Element = gst::ElementFactory::make("queue")
                .name(self.video_queue_name())
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
                .ok_or(StreamError::MissingElement("output_tee_audio".to_string()))?;
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
    /// tee pad-probe dance, then remove the whip sink.
    ///
    /// Awaits GStreamer state changes; the caller must NOT hold the
    /// pipeline state lock.
    pub(crate) async fn detach(&self, pipeline: &gst::Pipeline) -> Result<(), Error> {
        // Remove video/audio branch from pipeline
        Self::remove_branch_from_pipeline(pipeline, &self.video_queue_name()).await?;
        Self::remove_branch_from_pipeline(pipeline, &self.audio_queue_name()).await?;

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
    /// and remove the queue element from the pipeline.
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
        let queue_sink_pad =
            queue
                .static_pad("sink")
                .ok_or(StreamError::MissingElement(format!(
                    "{}'s sink pad",
                    queue_name
                )))?;

        // Remove src pad from tee if queue is linked
        let name = queue_name.to_string();
        if queue_sink_pad.is_linked() {
            let tee_src_pad = queue_sink_pad
                .peer()
                .ok_or(StreamError::MissingElement("tee's src pad".to_string()))?;
            let tee = tee_src_pad
                .parent_element()
                .ok_or(StreamError::MissingElement("output_tee".to_string()))?;

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
            return Err(StreamError::FailedOperation(format!(
                "Queue {} is not linked and can not be removed.",
                name
            ))
            .into());
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths_derive_from_one_convention() {
        let branch = Branch::for_id("abc");
        assert_eq!("whip-sink-abc", branch.whip_sink_name());
        assert_eq!("video-queue-abc", branch.video_queue_name());
        assert_eq!("audio-queue-abc", branch.audio_queue_name());

        assert_eq!("/whip_sink/abc", whip_sink_path("abc"));
        assert_eq!(
            "http://localhost:8000/whip_sink/abc",
            whip_endpoint(8000, "abc")
        );
    }

    #[test]
    fn every_branch_element_is_contained_and_maps_back_to_its_id() {
        let branch = Branch::for_id("abc");
        // ALL of a viewer's elements — the whip sink AND its per-media
        // queues — are recognized as branch-owned, so an error from any of
        // them is contained to that branch instead of restarting the pipeline.
        for name in [
            branch.whip_sink_name(),
            branch.video_queue_name(),
            branch.audio_queue_name(),
        ] {
            assert_eq!(
                Some("abc"),
                branch_id_from_name(&name),
                "{name} not contained"
            );
        }

        // The core (non-branch) elements stay fatal: their errors must still
        // quit the pipeline and trigger a supervisor restart.
        for name in ["video-queue", "audio-queue", "demux", "srt_source"] {
            assert_eq!(None, branch_id_from_name(name), "{name} wrongly contained");
        }
    }
}
