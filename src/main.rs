use clap::Parser;
use srt_whep::domain::SharableAppState;
use srt_whep::startup::run;
use srt_whep::stream::{Args, SharablePipeline};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::error::Error;
use std::net::TcpListener;
use tokio::signal;
use tokio::task;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let subscriber = get_subscriber("srt_whep".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);

    let app_data = SharableAppState::new();
    let pipeline_data = SharablePipeline::new(args.clone());
    let listener =
        TcpListener::bind(format!("0.0.0.0:{}", args.port)).expect("Whep port is already in use");

    let pipeline_clone = pipeline_data.clone();
    // Run the pipeline in a separate thread
    let pipeline_thread = task::spawn(async move {
        if let Err(error) = pipeline_clone.setup_pipeline(&args) {
            tracing::error!("Failed to setup pipeline: {}", error);
        }
    });

    // Start the http server
    run(listener, app_data, pipeline_data.clone())?.await?;

    signal::ctrl_c().await?;
    tracing::debug!("Received Ctrl-C signal");
    // Stop the pipeline
    if let Err(error) = pipeline_data.close_pipeline() {
        tracing::error!("Failed to close pipeline: {}", error);
    }
    pipeline_thread.abort();

    Ok(())
}
