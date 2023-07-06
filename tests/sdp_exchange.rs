use actix_web::http::StatusCode;
use once_cell::sync::Lazy;
use srt_whep::domain::{SharableAppState, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
use srt_whep::startup::run;
use srt_whep::stream::{Args, DumpPipeline, SRTMode};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;

// Ensure that the `tracing` stack is only initialised once using `once_cell`
static _TRACING: Lazy<()> = Lazy::new(|| {
    let default_filter_level = "debug".to_string();
    let subscriber_name = "test".to_string();
    let subscriber = get_subscriber(subscriber_name, default_filter_level, std::io::stdout);
    init_subscriber(subscriber);
});

fn spawn_app() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let app_data = SharableAppState::new();

    let port = listener.local_addr().unwrap().port();
    let args: Args = {
        Args {
            port: 28,
            input_address: "127.0.0.1:1234".to_string(),
            output_address: "127.0.0.1:1234".to_string(),
            srt_mode: SRTMode::Caller,
            discoverer_timeout_sec: 5,
        }
    };
    let dump_pipeline = DumpPipeline::new(args);

    let server = run(listener, app_data, dump_pipeline).expect("Failed to bind address");
    tokio::spawn(server);

    format!("http://127.0.0.1:{}", port)
}

#[tokio::test]
async fn health_check_works() {
    let address = spawn_app();
    let client = reqwest::Client::new();

    let response = client
        .get(format!("{}/health_check", &address))
        .send()
        .await
        .expect("Failed to execute request.");

    assert!(response.status().is_success());
    assert_eq!(Some(0), response.content_length());
}

#[tokio::test]
async fn subscribe_returns_a_200_after_exchange_offer() {
    let address = spawn_app();
    let whip_offer = VALID_WHIP_OFFER;
    let whep_answer = VALID_WHEP_ANSWER;

    // Send whip offer
    let address_clone = address.clone();
    let handle = tokio::spawn(async move {
        let whip_client = reqwest::Client::new();

        let whip_response = whip_client
            .post(address_clone + "/whip_sink")
            .header("Content-Type", "application/sdp")
            .body(whip_offer.to_string())
            .send()
            .await
            .expect("Failed to send whip offer");

        assert_eq!(StatusCode::OK, whip_response.status());
        assert_eq!("application/sdp", whip_response.headers()["content-type"]);
        assert!(whip_response
            .text()
            .await
            .unwrap()
            .contains("a=setup:passive"));
    });

    // Send empty post
    let whep_client = reqwest::Client::new();
    let whep_post_response = whep_client
        .post(address.clone() + "/channel")
        .header("Content-Type", "application/sdp")
        .send()
        .await
        .expect("Failed to send whep post");

    assert_eq!(StatusCode::CREATED, whep_post_response.status());
    let header = whep_post_response.headers().clone();
    assert_eq!("application/sdp", header["content-type"]);
    let url = header["Location"].to_str().unwrap();
    assert!(url.starts_with("/channel/"));
    let body = whep_post_response.text().await.unwrap();
    assert!(body.contains("a=setup:active"));

    // Send whep offer
    let whep_patch_response = whep_client
        .patch(address.clone() + url)
        .header("Content-Type", "application/sdp")
        .body(whep_answer.to_string())
        .send()
        .await
        .expect("Failed to send whep patch");

    assert_eq!(StatusCode::NO_CONTENT, whep_patch_response.status());
    handle.await.unwrap();
}

#[tokio::test]
async fn subscribe_returns_a_400_for_invalid_sdps() {
    let address = spawn_app();
    let client = reqwest::Client::new();

    let test_cases = vec![
        ("v=1", "invalid version"),
        ("v=0", "missing the sendonly/recvonly attribute"),
        ("", "empty string"),
        (" ", "whitespace only"),
    ];

    for (invalid_body, error_message) in test_cases {
        let response = client
            .post(address.clone() + "/whip_sink")
            .header("Content-Type", "application/sdp")
            .body(invalid_body)
            .send()
            .await
            .expect("Failed to execute request.");

        assert_eq!(
            400,
            response.status().as_u16(),
            "The API did not fail with 400 Bad Request when the payload was {}.",
            error_message
        );
    }
}
