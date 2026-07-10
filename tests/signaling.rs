use once_cell::sync::Lazy;
use reqwest::StatusCode;
use srt_whep::domain::{VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
use srt_whep::signal::CoordinatorConfig;
use srt_whep::startup::Application;
use srt_whep::stream::TestPipeline;
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;
use std::time::Duration;

static TRACING: Lazy<()> = Lazy::new(|| {
    let subscriber = get_subscriber("test".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);
});

/// Test HTTP client that ignores system/env proxy settings, so these
/// integration tests stay hermetic regardless of the environment they run in.
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("failed to build test client")
}

/// Comfortable timeouts for functional tests: long enough that a slow CI
/// machine never trips them accidentally.
fn functional_config() -> CoordinatorConfig {
    CoordinatorConfig {
        offer_timeout: Duration::from_secs(5),
        answer_timeout: Duration::from_secs(5),
        watchdog_threshold: 3,
        watchdog_window: Duration::from_secs(60),
        sweep_interval: Duration::from_millis(50),
        teardown_timeout: Duration::from_secs(5),
    }
}

/// Short timeouts for tests that deliberately let handshakes expire.
fn expiring_config(watchdog_threshold: u32) -> CoordinatorConfig {
    CoordinatorConfig {
        offer_timeout: Duration::from_millis(300),
        answer_timeout: Duration::from_millis(300),
        watchdog_threshold,
        watchdog_window: Duration::from_secs(60),
        sweep_interval: Duration::from_millis(50),
        teardown_timeout: Duration::from_secs(5),
    }
}

fn spawn_app(config: CoordinatorConfig) -> (String, TestPipeline) {
    Lazy::force(&TRACING);
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    // Share the bus-reap channel between the fake and the coordinator, exactly
    // as production wires the real pipeline — so `fail_branch` reaches the actor
    // (the reaping integration test depends on this).
    let (branch_failures_tx, branch_failures_rx) = tokio::sync::mpsc::channel(64);
    let pipeline = TestPipeline::new(branch_failures_tx);
    pipeline.set_ready(true);
    // The production wiring, supervisor included: TestPipeline::run parks
    // until end/quit, so the supervisor sits idle unless the watchdog
    // trips — exactly like the real pipeline between restarts.
    let app = Application::assemble(listener, pipeline.clone(), config, None, branch_failures_rx)
        .expect("Failed to assemble app");
    let address = format!("http://127.0.0.1:{}", app.port());
    tokio::spawn(app.run_until_stopped(std::future::pending()));
    (address, pipeline)
}

/// In production the coordinator's add_connection points a whipclientsink at
/// /whip_sink/{id}. Tests learn the id the same way: from the pipeline.
async fn wait_for_added_connection(pipeline: &TestPipeline, index: usize) -> String {
    for _ in 0..200 {
        let added = pipeline.snapshot().added;
        if added.len() > index {
            return added[index].clone();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("connection {} was never added to the pipeline", index);
}

/// Drives one full WHEP<->WHIP exchange and returns the connection id.
async fn complete_exchange(address: &str, pipeline: &TestPipeline, index: usize) -> String {
    let whep_task = {
        let address = address.to_string();
        tokio::spawn(async move {
            http_client()
                .post(format!("{}/channel", address))
                .header("Content-Type", "application/sdp")
                .send()
                .await
                .expect("whep post failed")
        })
    };

    let id = wait_for_added_connection(pipeline, index).await;

    let whip_task = {
        let address = address.to_string();
        let id = id.clone();
        tokio::spawn(async move {
            http_client()
                .post(format!("{}/whip_sink/{}", address, id))
                .header("Content-Type", "application/sdp")
                .body(VALID_WHIP_OFFER)
                .send()
                .await
                .expect("whip post failed")
        })
    };

    let whep_response = whep_task.await.unwrap();
    assert_eq!(201, whep_response.status());
    assert_eq!(
        "application/sdp",
        whep_response.headers()["content-type"].to_str().unwrap()
    );
    let location = whep_response.headers()["Location"]
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(format!("/channel/{}", id), location);
    let offer = whep_response.text().await.unwrap();
    assert!(offer.contains("a=sendonly"));

    let patch_response = http_client()
        .patch(format!("{}{}", address, location))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHEP_ANSWER)
        .send()
        .await
        .expect("patch failed");
    assert_eq!(204, patch_response.status());

    let whip_response = whip_task.await.unwrap();
    assert_eq!(201, whip_response.status());
    let answer = whip_response.text().await.unwrap();
    assert!(answer.contains("a=recvonly"));

    id
}

#[tokio::test]
async fn full_sdp_exchange_succeeds() {
    let (address, pipeline) = spawn_app(functional_config());
    complete_exchange(&address, &pipeline, 0).await;
    assert_eq!(0, pipeline.snapshot().quit_count);
}

#[tokio::test]
async fn non_empty_channel_post_is_rejected() {
    let (address, _pipeline) = spawn_app(functional_config());
    let response = http_client()
        .post(format!("{}/channel", address))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHIP_OFFER)
        .send()
        .await
        .unwrap();
    assert_eq!(400, response.status());
}

#[tokio::test]
async fn not_ready_pipeline_returns_503_with_retry_after() {
    let (address, pipeline) = spawn_app(functional_config());
    pipeline.set_ready(false);

    let response = http_client()
        .post(format!("{}/channel", address))
        .send()
        .await
        .unwrap();
    assert_eq!(503, response.status());
    assert_eq!("3", response.headers()["Retry-After"].to_str().unwrap());
}

#[tokio::test]
async fn invalid_sdps_are_rejected_with_400() {
    let (address, _pipeline) = spawn_app(functional_config());
    let client = http_client();

    let test_cases = vec![
        ("v=1", "invalid version"),
        ("v=0", "missing the sendonly/recvonly attribute"),
        ("", "empty string"),
        (" ", "whitespace only"),
    ];

    for (invalid_body, description) in test_cases {
        let response = client
            .post(format!("{}/whip_sink/some-id", address))
            .header("Content-Type", "application/sdp")
            .body(invalid_body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            400,
            response.status().as_u16(),
            "expected 400 for {}",
            description
        );
    }
}

#[tokio::test]
async fn unknown_ids_return_404() {
    let (address, _pipeline) = spawn_app(functional_config());
    let client = http_client();

    // Valid offer, but nobody created this connection.
    let response = client
        .post(format!("{}/whip_sink/ghost", address))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHIP_OFFER)
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    let response = client
        .patch(format!("{}/channel/ghost", address))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHEP_ANSWER)
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());

    // DELETE of an unknown id is idempotent (204), covered by delete_is_idempotent.
}

#[tokio::test]
async fn delete_is_idempotent() {
    let (address, pipeline) = spawn_app(functional_config());
    let client = http_client();

    let id = complete_exchange(&address, &pipeline, 0).await;

    // First DELETE terminates a live session: 200, branch torn down.
    let first = client
        .delete(format!("{}/channel/{}", address, id))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, first.status());
    assert!(pipeline.snapshot().removed.contains(&id));

    // Repeat DELETE of the same id: already gone -> 204 no-op, not 404.
    let repeat = client
        .delete(format!("{}/channel/{}", address, id))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::NO_CONTENT, repeat.status());

    // DELETE of an id that never existed -> 204.
    let ghost = client
        .delete(format!("{}/channel/never-existed", address))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::NO_CONTENT, ghost.status());
}

#[tokio::test]
async fn options_reports_cors_and_accept_post() {
    let (address, _pipeline) = spawn_app(functional_config());
    let response = http_client()
        .request(reqwest::Method::OPTIONS, format!("{}/channel", address))
        .send()
        .await
        .unwrap();
    assert_eq!(204, response.status());
    assert_eq!(
        "application/sdp",
        response.headers()["ACCEPT-POST"].to_str().unwrap()
    );
}

#[tokio::test]
async fn failed_handshake_does_not_affect_the_next_one() {
    let (address, pipeline) = spawn_app(expiring_config(3));

    // First viewer: nothing ever answers the whipsink leg -> offer times out.
    let response = http_client()
        .post(format!("{}/channel", address))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());

    let first_id = wait_for_added_connection(&pipeline, 0).await;
    let snap = pipeline.snapshot();
    assert!(snap.removed.contains(&first_id), "branch not cleaned up");
    assert_eq!(
        0, snap.quit_count,
        "single failure must not restart pipeline"
    );

    // Second viewer: full exchange succeeds on the same server.
    complete_exchange(&address, &pipeline, 1).await;
    assert_eq!(0, pipeline.snapshot().quit_count);
}

#[tokio::test]
async fn watchdog_restarts_pipeline_after_consecutive_failures() {
    let (address, pipeline) = spawn_app(expiring_config(2));
    let client = http_client();

    for _ in 0..2 {
        let response = client
            .post(format!("{}/channel", address))
            .send()
            .await
            .unwrap();
        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
    }

    // Threshold 2: the second consecutive failure requests a restart; the
    // supervisor force-quits the pipeline. That hop is async (coordinator ->
    // channel -> supervisor), so poll rather than assert immediately.
    for _ in 0..200 {
        if pipeline.snapshot().quit_count == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(1, pipeline.snapshot().quit_count);
}

#[tokio::test]
async fn whip_resource_location_is_routable_and_delete_removes_the_connection() {
    let (address, pipeline) = spawn_app(functional_config());
    let id = complete_exchange(&address, &pipeline, 0).await;

    // complete_exchange asserted the WHEP Location; fetch the WHIP one now.
    let response = http_client()
        .post(format!("{}/whip_sink/{}", address, id))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHIP_OFFER)
        .send()
        .await
        .unwrap();
    // Re-posting on an established connection is a wrong-state error, which
    // the signaling contract (src/signal/errors.rs) maps to 409 Conflict.
    assert_eq!(409, response.status().as_u16());

    // Drive a full second handshake (WHEP POST blocks until the WHIP offer
    // arrives, then the WHIP POST blocks until the WHEP PATCH answer
    // arrives) so the WHIP side actually reaches 201 and we can inspect its
    // Location header.
    let whep_task = {
        let address = address.clone();
        tokio::spawn(async move {
            http_client()
                .post(format!("{}/channel", address))
                .header("Content-Type", "application/sdp")
                .send()
                .await
                .unwrap()
        })
    };
    let second_id = wait_for_added_connection(&pipeline, 1).await;
    let whip_task = {
        let address = address.clone();
        let second_id = second_id.clone();
        tokio::spawn(async move {
            http_client()
                .post(format!("{}/whip_sink/{}", address, second_id))
                .header("Content-Type", "application/sdp")
                .body(VALID_WHIP_OFFER)
                .send()
                .await
                .unwrap()
        })
    };

    let whep_response = whep_task.await.unwrap();
    assert_eq!(201, whep_response.status().as_u16());
    let whep_location = whep_response.headers()["Location"]
        .to_str()
        .unwrap()
        .to_string();
    let patch_response = http_client()
        .patch(format!("{}{}", address, whep_location))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHEP_ANSWER)
        .send()
        .await
        .unwrap();
    assert_eq!(204, patch_response.status().as_u16());

    let whip_response = whip_task.await.unwrap();
    assert_eq!(201, whip_response.status().as_u16());
    let location = whip_response.headers()["Location"]
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(format!("/whip_sink/{}", second_id), location);

    // The advertised resource URL must actually work: DELETE terminates.
    let del = http_client()
        .delete(format!("{}{}", address, location))
        .send()
        .await
        .unwrap();
    assert_eq!(200, del.status().as_u16());
    assert!(pipeline.snapshot().removed.contains(&second_id));
}

#[tokio::test]
async fn list_and_delete_manage_the_connection_lifecycle() {
    let (address, pipeline) = spawn_app(functional_config());
    let client = http_client();

    let id = complete_exchange(&address, &pipeline, 0).await;

    // Established connection is listed with its state.
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/list", address))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(1, list.len());
    assert_eq!(id, list[0]["id"]);
    assert_eq!("established", list[0]["state"]);

    // DELETE removes it from the pipeline and the list.
    let response = client
        .delete(format!("{}/channel/{}", address, id))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert!(pipeline.snapshot().removed.contains(&id));

    let list: Vec<serde_json::Value> = client
        .get(format!("{}/list", address))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn a_branch_runtime_failure_reaps_the_established_connection() {
    let (address, pipeline) = spawn_app(functional_config());
    let client = http_client();

    let id = complete_exchange(&address, &pipeline, 0).await;

    // The pipeline's bus watch reports this viewer's branch errored at runtime
    // (its peer went away / its whipsink failed). The coordinator must reap the
    // dead branch instead of leaking it as a ghost /list entry forever.
    pipeline.fail_branch(&id);

    for _ in 0..200 {
        if pipeline.snapshot().removed.contains(&id) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        pipeline.snapshot().removed.contains(&id),
        "the errored branch was never reaped"
    );

    let list: Vec<serde_json::Value> = client
        .get(format!("{}/list", address))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        list.is_empty(),
        "reaped connection still listed: {:?}",
        list
    );

    // A dead peer is not a pipeline-health signal: the pipeline stays up.
    assert_eq!(0, pipeline.snapshot().quit_count);
}

#[tokio::test]
async fn assemble_rejects_a_mismatched_whip_port() {
    Lazy::force(&TRACING);
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let bound = listener.local_addr().unwrap().port();
    let pipeline = TestPipeline::default();
    // Deliberately claim a different callback port than the one bound.
    let wrong = bound.checked_add(1).unwrap_or(bound - 1);
    // Assembly fails on the port check before the reap channel is used.
    let (_fail_tx, fail_rx) = tokio::sync::mpsc::channel(1);
    let result = Application::assemble(
        listener,
        pipeline,
        functional_config(),
        Some(wrong),
        fail_rx,
    );
    assert!(result.is_err(), "mismatched whip port must fail assembly");
}
