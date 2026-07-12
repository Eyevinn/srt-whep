//! Egress chain construction -- the codec table.
//!
//! When the demux announces its pads, each media type gets an egress chain
//! hung off its pre-built ingest queue: video is parsed (`h264parse` /
//! `h265parse`) and fanned out through a named output tee; audio is transcoded
//! AAC -> Opus and fanned out the same way. The named tees
//! ([`naming::OUTPUT_TEE_VIDEO`] / [`naming::OUTPUT_TEE_AUDIO`]) are the
//! attach points for WHEP branches; the terminating fakesink keeps each chain
//! consuming buffers -- and pops EOS onto the message bus when the SRT input
//! closes -- even with zero viewers attached.
//!
//! The decision (which parser, which chain, what counts as unsupported) lives
//! here; the dispatch (walking the demux's src pads on no-more-pads) stays
//! with `init()` in `gst_pipeline.rs`.

use anyhow::{anyhow, Error};
use gst::prelude::*;
use gstreamer as gst;

use crate::stream::naming;

/// Build and start the egress chain for one demuxed media type.
///
/// The queues are the ones `init()` already constructed and added to the
/// pipeline -- this function links the new chain onto the matching one and
/// leaves the other untouched. Every element it creates is synced to the
/// pipeline's state before returning; without that the chain would sit in
/// Null and never process data. Unknown media types are an error and build
/// nothing.
pub(crate) fn build_egress_chain(
    pipeline: &gst::Pipeline,
    media_type: &str,
    video_queue: &gst::Element,
    audio_queue: &gst::Element,
) -> Result<(), Error> {
    // Codec table: the video arms differ only in which parser element sits
    // between the queue and the tee.
    let video_parser = if media_type.starts_with("video/x-h264") {
        Some("h264parse")
    } else if media_type.starts_with("video/x-h265") {
        tracing::warn!("H.265(HEVC) streams can be linked but are not fully supported yet");
        Some("h265parse")
    } else {
        None
    };

    if let Some(parser) = video_parser {
        let parse = gst::ElementFactory::make(parser).build()?;
        let output_tee_video = gst::ElementFactory::make("tee")
            .name(naming::OUTPUT_TEE_VIDEO)
            .build()?;
        // Add a fakesink to the end of pipeline to consume buffers
        // it receives and pops EOS to message bus when the SRT input stream is closed
        let fakesink = gst::ElementFactory::make("fakesink")
            .property("can-activate-pull", true)
            .build()?;

        let video_elements = &[video_queue, &parse, &output_tee_video, &fakesink];
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
            .name(naming::OUTPUT_TEE_AUDIO)
            .build()?;
        let fakesink = gst::ElementFactory::make("fakesink")
            .property("can-activate-pull", true)
            .build()?;

        let audio_elements = &[
            audio_queue,
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
        Err(anyhow!("Unknown media type {}", media_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipeline holding the two pre-built ingest queues, exactly as `init()`
    /// has them by the time the demux announces its pads.
    fn pipeline_with_queues() -> (gst::Pipeline, gst::Element, gst::Element) {
        gst::init().unwrap();
        let pipeline = gst::Pipeline::default();
        let video_queue = gst::ElementFactory::make("queue")
            .name(naming::VIDEO_QUEUE)
            .build()
            .unwrap();
        let audio_queue = gst::ElementFactory::make("queue")
            .name(naming::AUDIO_QUEUE)
            .build()
            .unwrap();
        pipeline.add_many([&video_queue, &audio_queue]).unwrap();
        (pipeline, video_queue, audio_queue)
    }

    /// Factory name of the element the queue got linked to -- which parser
    /// (or transcode head) the codec table picked.
    fn linked_factory(queue: &gst::Element) -> String {
        queue
            .static_pad("src")
            .unwrap()
            .peer()
            .expect("queue src pad should be linked")
            .parent_element()
            .unwrap()
            .factory()
            .unwrap()
            .name()
            .to_string()
    }

    #[test]
    fn h264_is_parsed_into_the_video_tee() {
        let (pipeline, video_queue, audio_queue) = pipeline_with_queues();
        build_egress_chain(&pipeline, "video/x-h264", &video_queue, &audio_queue).unwrap();
        assert!(pipeline.by_name(naming::OUTPUT_TEE_VIDEO).is_some());
        assert!(pipeline.by_name(naming::OUTPUT_TEE_AUDIO).is_none());
        assert_eq!("h264parse", linked_factory(&video_queue));
    }

    #[test]
    fn h265_swaps_only_the_parser() {
        let (pipeline, video_queue, audio_queue) = pipeline_with_queues();
        build_egress_chain(&pipeline, "video/x-h265", &video_queue, &audio_queue).unwrap();
        assert!(pipeline.by_name(naming::OUTPUT_TEE_VIDEO).is_some());
        assert_eq!("h265parse", linked_factory(&video_queue));
    }

    #[test]
    fn audio_is_transcoded_into_the_audio_tee() {
        let (pipeline, video_queue, audio_queue) = pipeline_with_queues();
        build_egress_chain(&pipeline, "audio/mpeg", &video_queue, &audio_queue).unwrap();
        assert!(pipeline.by_name(naming::OUTPUT_TEE_AUDIO).is_some());
        assert!(pipeline.by_name(naming::OUTPUT_TEE_VIDEO).is_none());
        // The transcode chain starts at the parser; WHEP delivers Opus, so the
        // chain must re-encode rather than pass AAC through.
        assert_eq!("aacparse", linked_factory(&audio_queue));
    }

    #[test]
    fn unknown_media_is_an_error_and_builds_nothing() {
        let (pipeline, video_queue, audio_queue) = pipeline_with_queues();
        let result = build_egress_chain(&pipeline, "text/x-raw", &video_queue, &audio_queue);
        assert!(result.is_err());
        assert!(pipeline.by_name(naming::OUTPUT_TEE_VIDEO).is_none());
        assert!(pipeline.by_name(naming::OUTPUT_TEE_AUDIO).is_none());
        assert!(!video_queue.static_pad("src").unwrap().is_linked());
        assert!(!audio_queue.static_pad("src").unwrap().is_linked());
    }
}
