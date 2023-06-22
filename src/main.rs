use clap::Parser;
use srt_whep::domain::SharableAppState;
use srt_whep::pipeline::{Args, SharablePipeline};
use srt_whep::startup::run;
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;
use tokio::signal;
use tokio::task;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let subscriber = get_subscriber("srt_whep".into(), "info".into(), std::io::stdout);
    init_subscriber(subscriber);

    let app_data = SharableAppState::new(args.port);
    let pipeline_data = SharablePipeline::new(args.clone());
    let listener =
        TcpListener::bind(format!("0.0.0.0:{}", args.port)).expect("Whep port is already in use");

    let p2 = pipeline_data.clone();
    // Run the pipeline in a separate thread
    let t = task::spawn(async move {
        if let Err(error) = p2.setup_pipeline(&args) {
            tracing::error!("Failed to setup pipeline: {}", error);
        }
    });

    // Start the http server
    run(listener, app_data, pipeline_data.clone())?.await?;

    signal::ctrl_c().await?;
    tracing::info!("Received Ctrl-C signal");
    if let Err(error) = pipeline_data.close_pipeline() {
        tracing::error!("Failed to close pipeline: {}", error);
    }
    t.abort();

    Ok(())
}
