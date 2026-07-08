//! End-to-end test against a real GStreamer pipeline. Requires GStreamer
//! (with x264enc and srt plugins) to be installed — see README for setup.
//!
//! Run with: cargo test --test e2e_gstreamer -- --ignored --nocapture
//!
//! Run it in ISOLATION (one process at a time). It drives a real WebRTC
//! whipclientsink and hardware video encoder; hammering it back-to-back can
//! starve those resources so the whipclientsink fails to emit its SDP offer
//! within the timeout and the run fails (it exits cleanly rather than hanging).
//!
//! Scope: the "wedge risk" — proving that repeatedly hot-plugging and
//! removing whipsink branches does not stall the pipeline. Feeding canned
//! SDP answers to a real whipclientsink would trigger DTLS/ICE against a
//! nonexistent peer and can error the pipeline, so handshakes here are
//! driven to offer receipt and then deliberately abandoned. An abandoned
//! handshake still errors its own whipclientsink branch on the bus; that must
//! stay contained to the branch (see `run()`'s bus watch) and never tear down
//! the shared pipeline. Media playout is verified manually with the WHEP player.
use gst::prelude::*;
use gstreamer as gst;
use srt_whep::signal::CoordinatorConfig;
use srt_whep::startup::Application;
use srt_whep::stream::{Args, BranchControl, SRTMode, SharablePipeline};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
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
        watchdog_window: Duration::from_secs(60),
        sweep_interval: Duration::from_millis(200),
        teardown_timeout: Duration::from_secs(5),
    };

    let listener =
        TcpListener::bind(format!("127.0.0.1:{}", HTTP_PORT)).expect("e2e HTTP port in use");
    // The production wiring: coordinator + supervisor + HTTP server.
    let app = Application::assemble(listener, pipeline.clone(), config).unwrap();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let app_task = tokio::spawn(app.run_until_stopped(async move {
        let _ = stop_rx.await;
    }));

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", HTTP_PORT);

    // Drive the scenario capturing the outcome instead of asserting inline, so
    // teardown below always runs. A panic here while the pipeline's glib
    // MainLoop is parked on a worker thread would otherwise hang the
    // multi-thread runtime on drop and orphan the process (leaking the SRT and
    // HTTP ports for the next run).
    let outcome: Result<(), String> = async {
        // Wait for the SRT input to be demuxed.
        let mut ready = false;
        for _ in 0..100 {
            if pipeline.ready().await.unwrap_or(false) {
                ready = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        if !ready {
            return Err(format!(
                "pipeline never became ready — is GStreamer installed and port {} free?",
                SRT_PORT
            ));
        }

        // Three cycles: receive a real offer from a real whipclientsink, then
        // abandon the handshake. Branch cleanup must not wedge the pipeline.
        for round in 0..3 {
            let response = client
                .post(format!("{}/channel", base))
                .header("Content-Type", "application/sdp")
                .send()
                .await
                .map_err(|e| format!("round {}: POST /channel failed: {}", round, e))?;
            let status = response.status().as_u16();
            let offer = response.text().await.unwrap_or_default();
            if status != 201 {
                return Err(format!(
                    "round {}: no offer received (status {}, body: {})",
                    round, status, offer
                ));
            }
            if !offer.starts_with("v=0") {
                return Err(format!("round {}: not an SDP offer", round));
            }

            // No PATCH: the answer times out (3s) and the branch is removed.
            tokio::time::sleep(Duration::from_secs(4)).await;
        }

        // After the add/remove cycles the pipeline must still hand out offers.
        let response = client
            .post(format!("{}/channel", base))
            .header("Content-Type", "application/sdp")
            .send()
            .await
            .map_err(|e| format!("final POST /channel failed: {}", e))?;
        if response.status().as_u16() != 201 {
            return Err(format!(
                "pipeline wedged after failure cycles (status {})",
                response.status().as_u16()
            ));
        }

        Ok(())
    }
    .await;

    // Orderly teardown = the production shutdown path: one stop signal fans
    // out to the supervisor (EOS → bounded join) and the HTTP server. Bounded
    // by an outer timeout so a wedged element can't hang teardown itself.
    let _ = stop_tx.send(());
    let shutdown = tokio::time::timeout(Duration::from_secs(15), app_task).await;
    let _ = source.set_state(gst::State::Null);

    let shutdown_clean = shutdown.is_ok();
    if let Err(reason) = &outcome {
        // On the happy path the test returns normally and the runtime drops
        // cleanly. But a *flaked* round (e.g. the live whipclientsink failing
        // to emit its offer in time under back-to-back runs) can leave a
        // GStreamer element stuck at the NULL transition; unwinding through a
        // panic would then hang the process on runtime drop joining that
        // worker. Report and hard-exit so the failure surfaces instead of
        // hanging.
        eprintln!("\n=== e2e assertion failed: {} ===\n", reason);
    }
    if !shutdown_clean {
        eprintln!("\n=== e2e: production shutdown path did not finish within 15s ===\n");
    }
    if outcome.is_err() || !shutdown_clean {
        std::process::exit(1);
    }
}
