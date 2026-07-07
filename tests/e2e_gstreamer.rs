//! End-to-end test against a real GStreamer pipeline. Requires GStreamer
//! (with x264enc and srt plugins) to be installed — see README for setup.
//!
//! Run with: cargo test --test e2e_gstreamer -- --ignored --nocapture
//!
//! Scope: the "wedge risk" — proving that repeatedly hot-plugging and
//! removing whipsink branches does not stall the pipeline. Feeding canned
//! SDP answers to a real whipclientsink would trigger DTLS/ICE against a
//! nonexistent peer and can error the pipeline, so handshakes here are
//! driven to offer receipt and then deliberately abandoned. Media playout
//! is verified manually with the WHEP player.
use gst::prelude::*;
use gstreamer as gst;
use srt_whep::signal::{spawn_coordinator, CoordinatorConfig};
use srt_whep::startup::run;
use srt_whep::stream::{Args, PipelineBase, SRTMode, SharablePipeline};
use srt_whep::utils::PipelineGuard;
use std::net::TcpListener;
use std::time::Duration;

const SRT_PORT: u16 = 9911;
const HTTP_PORT: u16 = 8199;

fn start_srt_source() -> gst::Pipeline {
    gst::init().unwrap();
    let pipeline = gst::parse::launch(&format!(
        "videotestsrc is-live=true \
         ! video/x-raw,width=320,height=240,framerate=25/1 \
         ! x264enc tune=zerolatency key-int-max=25 bitrate=500 \
         ! mpegtsmux ! srtsink uri=srt://127.0.0.1:{}?mode=listener wait-for-connection=false",
        SRT_PORT
    ))
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    pipeline
}

#[tokio::test]
#[ignore]
async fn pipeline_survives_repeated_handshake_failures() {
    let source = start_srt_source();

    let args = Args {
        input_address: format!("127.0.0.1:{}", SRT_PORT),
        output_address: "127.0.0.1:9912".to_string(),
        srt_mode: SRTMode::Caller,
        srt_latency: 100,
        tsdemux_latency: 100,
        run_discoverer: false,
        discoverer_timeout_sec: 5,
        port: HTTP_PORT as u32, // whipclientsink posts back to this port
    };

    let pipeline = SharablePipeline::new(args.clone());
    let config = CoordinatorConfig {
        offer_timeout: Duration::from_secs(10),
        answer_timeout: Duration::from_secs(3),
        watchdog_threshold: 10, // deliberate failures below must not trip it
        sweep_interval: Duration::from_millis(200),
    };
    let signal = spawn_coordinator(pipeline.clone(), config);

    let listener =
        TcpListener::bind(format!("127.0.0.1:{}", HTTP_PORT)).expect("e2e HTTP port in use");
    let server = run(listener, signal.clone()).unwrap();
    tokio::spawn(server);

    // Supervise the pipeline exactly like main.rs does.
    let pipeline_clone = pipeline.clone();
    let args_clone = args.clone();
    let signal_clone = signal.clone();
    tokio::spawn(async move {
        loop {
            let mut guard = PipelineGuard::new(
                pipeline_clone.clone(),
                args_clone.clone(),
                signal_clone.clone(),
            );
            if let Err(e) = guard.run().await {
                eprintln!("pipeline error: {}", e);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // Wait for the SRT input to be demuxed.
    let mut ready = false;
    for _ in 0..100 {
        if pipeline.ready().await.unwrap_or(false) {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        ready,
        "pipeline never became ready — is GStreamer installed and port {} free?",
        SRT_PORT
    );

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", HTTP_PORT);

    // Three cycles: receive a real offer from a real whipclientsink, then
    // abandon the handshake. Branch cleanup must not wedge the pipeline.
    for round in 0..3 {
        let response = client
            .post(format!("{}/channel", base))
            .header("Content-Type", "application/sdp")
            .send()
            .await
            .expect("POST /channel failed");
        assert_eq!(
            201,
            response.status().as_u16(),
            "round {}: no offer received",
            round
        );
        let offer = response.text().await.unwrap();
        assert!(
            offer.starts_with("v=0"),
            "round {}: not an SDP offer",
            round
        );

        // No PATCH: the answer times out (3s) and the branch is removed.
        tokio::time::sleep(Duration::from_secs(4)).await;
    }

    // After the add/remove cycles the pipeline must still hand out offers.
    let response = client
        .post(format!("{}/channel", base))
        .header("Content-Type", "application/sdp")
        .send()
        .await
        .expect("final POST /channel failed");
    assert_eq!(
        201,
        response.status().as_u16(),
        "pipeline wedged after failure cycles"
    );

    source.set_state(gst::State::Null).unwrap();
}
