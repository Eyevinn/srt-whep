use clap::Parser;
use hello_world::domain::SharableAppState;
use hello_world::pipeline::{setup_pipeline, Args};
use hello_world::startup::run;
use hello_world::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let subscriber = get_subscriber("hello_world".into(), "info".into(), std::io::stdout);
    init_subscriber(subscriber);

    let app_data = SharableAppState::new();
    let listener =
        TcpListener::bind(format!("127.0.0.1:{}", args.port)).expect("Whep port is already in use");

    // Run the pipeline in a separate thread
    let t = tokio::spawn(async move {
        setup_pipeline(&args).expect("Failed to setup pipeline");
    });

    run(listener, app_data)?.await?;
    t.await.expect("Failed to stop server");

    Ok(())
}
