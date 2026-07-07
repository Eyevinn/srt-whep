use once_cell::sync::Lazy;
use srt_whep::domain::{VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
use srt_whep::signal::{spawn_coordinator, CoordinatorConfig};
use srt_whep::startup::run;
use srt_whep::stream::TestPipeline;
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;
use std::time::Duration;

static TRACING: Lazy<()> = Lazy::new(|| {
    let subscriber = get_subscriber("test".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);
});

/// Comfortable timeouts for functional tests: long enough that a slow CI
/// machine never trips them accidentally.
fn functional_config() -> CoordinatorConfig {
    CoordinatorConfig {
        offer_timeout: Duration::from_secs(5),
        answer_timeout: Duration::from_secs(5),
        watchdog_threshold: 3,
        sweep_interval: Duration::from_millis(50),
    }
}

/// Short timeouts for tests that deliberately let handshakes expire.
#[allow(dead_code)]
fn expiring_config(watchdog_threshold: u32) -> CoordinatorConfig {
    CoordinatorConfig {
        offer_timeout: Duration::from_millis(300),
        answer_timeout: Duration::from_millis(300),
        watchdog_threshold,
        sweep_interval: Duration::from_millis(50),
    }
}

fn spawn_app(config: CoordinatorConfig) -> (String, TestPipeline) {
    Lazy::force(&TRACING);
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let port = listener.local_addr().unwrap().port();
    let pipeline = TestPipeline::default();
    pipeline.set_ready(true);
    let signal = spawn_coordinator(pipeline.clone(), config);
    let server = run(listener, signal).expect("Failed to start server");
    tokio::spawn(server);
    (format!("http://127.0.0.1:{}", port), pipeline)
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
            reqwest::Client::new()
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
            reqwest::Client::new()
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

    let patch_response = reqwest::Client::new()
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
    let response = reqwest::Client::new()
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

    let response = reqwest::Client::new()
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
    let client = reqwest::Client::new();

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
    let client = reqwest::Client::new();

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

    let response = client
        .delete(format!("{}/channel/ghost", address))
        .send()
        .await
        .unwrap();
    assert_eq!(404, response.status());
}

#[tokio::test]
async fn options_reports_cors_and_accept_post() {
    let (address, _pipeline) = spawn_app(functional_config());
    let response = reqwest::Client::new()
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
