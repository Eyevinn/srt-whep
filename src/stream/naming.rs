//! Single source of truth for GStreamer element names in the stream plane, and
//! the classifier that decides whether a bus error belongs to one viewer's
//! branch (reap just that branch) or the core demux->tee chain (fatal ->
//! supervisor restart).
//!
//! Branch element names are DERIVED from the core stems, so the load-bearing
//! relationship -- a branch queue is exactly `<core-queue-name>-<id>`, and the
//! trailing dash is the only thing keeping a core-queue error fatal -- holds by
//! construction here, not by hand across `gst_pipeline.rs` and `branch.rs`.
//!
//! Pure string logic: no GStreamer types, no `src/signal` dependency (the
//! acyclic module graph from ADR 0001 stays intact).

// Core (viewer-independent) elements. These names are referenced from more than
// one place, so they live here once.
pub(crate) const DEMUX: &str = "demux";
pub(crate) const VIDEO_QUEUE: &str = "video-queue";
pub(crate) const AUDIO_QUEUE: &str = "audio-queue";
pub(crate) const OUTPUT_TEE_VIDEO: &str = "output_tee_video";
pub(crate) const OUTPUT_TEE_AUDIO: &str = "output_tee_audio";
pub(crate) const SRT_SOURCE: &str = "srt_source";

// Branch-only stems (no core element shares these names).
const WHIP_SINK_STEM: &str = "whip-sink";
const VIDEO_DECODER_STEM: &str = "avdec-h264"; // present only under --decode-video

pub(crate) fn video_queue_name(id: &str) -> String {
    format!("{VIDEO_QUEUE}-{id}")
}

pub(crate) fn audio_queue_name(id: &str) -> String {
    format!("{AUDIO_QUEUE}-{id}")
}

pub(crate) fn whip_sink_name(id: &str) -> String {
    format!("{WHIP_SINK_STEM}-{id}")
}

pub(crate) fn video_decoder_name(id: &str) -> String {
    format!("{VIDEO_DECODER_STEM}-{id}")
}

/// If `name` is a per-viewer branch element, return the connection id it
/// belongs to. Recognizes the whip sink, the per-media queues, and the optional
/// `--decode-video` H264 decoder.
///
/// A branch element is exactly `<stem>-<id>`: strip the stem, then REQUIRE the
/// '-'. A core queue named exactly `video-queue` strips to "" and the missing
/// '-' makes it return `None` -- that is what keeps a core-queue error fatal.
///
/// The bus watch uses this to contain a dying branch's errors to that branch
/// (reaping just that connection) instead of quitting the whole pipeline, which
/// would drop the SRT ingest and every other viewer.
pub(crate) fn branch_id_from_name(name: &str) -> Option<&str> {
    for stem in [WHIP_SINK_STEM, VIDEO_QUEUE, AUDIO_QUEUE, VIDEO_DECODER_STEM] {
        if let Some(id) = name
            .strip_prefix(stem)
            .and_then(|rest| rest.strip_prefix('-'))
        {
            return Some(id);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_derive_from_one_convention() {
        assert_eq!("whip-sink-abc", whip_sink_name("abc"));
        assert_eq!("video-queue-abc", video_queue_name("abc"));
        assert_eq!("audio-queue-abc", audio_queue_name("abc"));
        assert_eq!("avdec-h264-abc", video_decoder_name("abc"));
    }

    #[test]
    fn every_branch_element_maps_back_to_its_id() {
        // ALL of a viewer's elements -- the whip sink, its per-media queues, and
        // the optional decoder -- are recognized as branch-owned, so an error
        // from any of them is contained to that branch.
        for name in [
            whip_sink_name("abc"),
            video_queue_name("abc"),
            audio_queue_name("abc"),
            video_decoder_name("abc"),
        ] {
            assert_eq!(
                Some("abc"),
                branch_id_from_name(&name),
                "{name} not contained"
            );
        }
    }

    #[test]
    fn core_element_names_never_classify_as_a_branch() {
        // The load-bearing invariant: a core element error must stay fatal.
        // Asserted against the consts (not literals) so this test breaks the
        // instant the derive-from-stem relationship is broken.
        for name in [
            DEMUX,
            VIDEO_QUEUE,
            AUDIO_QUEUE,
            OUTPUT_TEE_VIDEO,
            OUTPUT_TEE_AUDIO,
            SRT_SOURCE,
        ] {
            assert_eq!(None, branch_id_from_name(name), "{name} wrongly contained");
        }
    }
}
