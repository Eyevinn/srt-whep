//! Policy for the pipeline's bus watch: what a bus message means for the
//! pipeline's fate, decided as a pure classification so the containment
//! guarantee is unit-testable without SRT or a live peer.
//!
//! An error from a WHEP output branch (a `whip-sink-*`/`*-queue-*` element or
//! anything nested inside it -- e.g. its signaller timing out or its peer
//! going away) must not be fatal. Quitting the main loop would drop the SRT
//! ingest and every other viewer, and the ensuing supervisor restart would
//! reset all in-flight handshakes -- the "wedge" a single bad peer must never
//! be able to cause (ADR 0002). Instead the error source's ancestry is walked
//! to find which viewer's branch it belongs to, so the coordinator can reap
//! that one connection while the pipeline stays up. Errors from core
//! (viewer-independent) elements stay fatal, as does end-of-stream.
//!
//! The decision lives here; the mechanism (`main_loop.quit()`, the reap
//! channel `try_send`) stays with the bus watch in `gst_pipeline.rs`.

use gst::prelude::*;
use gstreamer as gst;

use crate::stream::naming::{self, BranchId};

/// What the bus watch must do with one bus message.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum BusAction {
    /// Stop the pipeline: end of stream, or a fatal error from a core
    /// (viewer-independent) element. The supervisor restarts from there.
    Quit,
    /// One viewer's branch failed at runtime: ask the coordinator to reap
    /// exactly that branch. The pipeline stays up.
    ReapBranch(BranchId),
    /// Not lifecycle-relevant.
    Ignore,
}

/// Classify one bus message into the action the watch must take.
///
/// Pure -- no side effects, no locks -- so it is safe on the GLib loop thread
/// and testable with a hand-built element hierarchy. For an error message the
/// source's ancestry is walked upward until a branch-derived name
/// ([`naming::branch_id_from_name`]) identifies the owning viewer; an error
/// that reaches the top without a match (a core element, or no source at all)
/// is fatal.
pub(crate) fn classify_bus_message(msg: &gst::Message) -> BusAction {
    use gst::MessageView;

    match msg.view() {
        MessageView::Eos(..) => BusAction::Quit,
        MessageView::Error(err) => {
            let mut cursor = err.src().cloned();
            while let Some(obj) = cursor {
                if let Some(id) = naming::branch_id_from_name(obj.name().as_str()) {
                    return BusAction::ReapBranch(BranchId::new(id));
                }
                cursor = obj.parent();
            }
            BusAction::Quit
        }
        _ => BusAction::Ignore,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gst::prelude::*;

    fn named_bin(name: &str) -> gst::Bin {
        gst::init().unwrap();
        gst::Bin::builder().name(name).build()
    }

    fn error_from(src: &gst::Bin) -> gst::Message {
        gst::message::Error::builder(gst::CoreError::Failed, "boom")
            .src(src)
            .build()
    }

    #[test]
    fn branch_element_error_reaps_only_that_branch() {
        let queue = named_bin(&naming::video_queue_name("abc"));
        assert_eq!(
            BusAction::ReapBranch(BranchId::new("abc")),
            classify_bus_message(&error_from(&queue))
        );
    }

    #[test]
    fn error_nested_inside_a_branch_walks_up_to_its_branch() {
        // The real failure mode: the whip sink is a bin, and what errors is
        // some element buried inside it (its signaller, an internal webrtcbin)
        // whose own name says nothing about the viewer.
        let sink = named_bin(&naming::whip_sink_name("abc"));
        let inner = named_bin("some-internal-element");
        sink.add(&inner).unwrap();
        assert_eq!(
            BusAction::ReapBranch(BranchId::new("abc")),
            classify_bus_message(&error_from(&inner))
        );
    }

    #[test]
    fn core_element_error_is_fatal() {
        let queue = named_bin(naming::VIDEO_QUEUE);
        assert_eq!(BusAction::Quit, classify_bus_message(&error_from(&queue)));
    }

    #[test]
    fn error_without_a_source_is_fatal() {
        gst::init().unwrap();
        let msg = gst::message::Error::builder(gst::CoreError::Failed, "boom").build();
        assert_eq!(BusAction::Quit, classify_bus_message(&msg));
    }

    #[test]
    fn eos_quits() {
        gst::init().unwrap();
        assert_eq!(
            BusAction::Quit,
            classify_bus_message(&gst::message::Eos::builder().build())
        );
    }

    #[test]
    fn other_messages_are_ignored() {
        gst::init().unwrap();
        assert_eq!(
            BusAction::Ignore,
            classify_bus_message(&gst::message::Buffering::builder(50).build())
        );
    }

    #[test]
    fn containment_scope_holds_at_the_hierarchy_level() {
        // ADR 0002's containment scope, asserted through the classifier the
        // bus watch actually runs -- against the naming consts/derivations
        // (not literals), so this breaks the instant a rename splits the two.
        // Every core element error must stay fatal...
        for name in [
            naming::DEMUX,
            naming::VIDEO_QUEUE,
            naming::AUDIO_QUEUE,
            naming::OUTPUT_TEE_VIDEO,
            naming::OUTPUT_TEE_AUDIO,
            naming::SRT_SOURCE,
        ] {
            let bin = named_bin(name);
            assert_eq!(
                BusAction::Quit,
                classify_bus_message(&error_from(&bin)),
                "{name} must be fatal"
            );
        }
        // ...and every branch-derived element error must reap its viewer.
        for name in [
            naming::whip_sink_name("abc"),
            naming::video_queue_name("abc"),
            naming::audio_queue_name("abc"),
            naming::video_decoder_name("abc"),
        ] {
            let bin = named_bin(&name);
            assert_eq!(
                BusAction::ReapBranch(BranchId::new("abc")),
                classify_bus_message(&error_from(&bin)),
                "{name} must be contained"
            );
        }
    }
}
