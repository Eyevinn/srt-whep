use futures::future::join;
use once_cell::sync::Lazy;
use srt_whep::domain::{SharableAppState, VALID_WHEP_OFFER, VALID_WHIP_OFFER};
use srt_whep::startup::run;
use srt_whep::stream::{Args, SRTMode, SharablePipeline};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;

// Ensure that the `tracing` stack is only initialised once using `once_cell`
static _TRACING: Lazy<()> = Lazy::new(|| {
    let default_filter_level = "info".to_string();
    let subscriber_name = "test".to_string();
    if std::env::var("TEST_LOG").is_ok() {
        let subscriber = get_subscriber(subscriber_name, default_filter_level, std::io::stdout);
        init_subscriber(subscriber);
    } else {
        let subscriber = get_subscriber(subscriber_name, default_filter_level, std::io::sink);
        init_subscriber(subscriber);
    };
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
        }
    };
    let pipeline_data = SharablePipeline::new(args);

    let server = run(listener, app_data, pipeline_data).expect("Failed to bind address");
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
    let client = reqwest::Client::new();
    let whip_offer = VALID_WHIP_OFFER;
    let whep_offer = VALID_WHEP_OFFER;

    let whip_response = client
        .post(address.clone() + "/whip_sink")
        .header("Content-Type", "application/sdp")
        .body(whip_offer.to_string())
        .send();
    let whep_response = client
        .post(address.clone() + "/whep_sink")
        .header("Content-Type", "application/sdp")
        .body(whep_offer.to_string())
        .send();
    let (whip_response, whep_response) = join(whip_response, whep_response).await;
    let (_whip_response, _whep_response) = (
        whip_response.expect("Failed to receive whip response"),
        whep_response.expect("Failed to receive whep response"),
    );

    // assert_eq!(StatusCode::OK, whip_response.status());
    // assert_eq!("application/sdp", whip_response.headers()["content-type"]);
    // assert!(whip_response
    //     .text()
    //     .await
    //     .unwrap()
    //     .contains("a=setup:passive"));

    // assert_eq!(StatusCode::CREATED, whep_response.status());
    // assert_eq!("application/sdp", whep_response.headers()["content-type"]);
    // assert!(whep_response
    //     .text()
    //     .await
    //     .unwrap()
    //     .contains("a=setup:active"));
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
