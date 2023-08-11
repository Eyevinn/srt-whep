use clap::Parser;
use srt_whep::domain::SharableAppState;
use srt_whep::startup::run;
use srt_whep::stream::{Args, PipelineBase, SharablePipeline};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::error::Error;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::signal;
use tokio::task;
use tokio::time::{sleep, Duration};

/// Run a pipeline until it encounters EOS or an error. Clean up the pipeline after it finishes.
/// This function can be called multiple times to handle EOS.
async fn run_pipeline(pipeline: &mut SharablePipeline, args: &Args) -> Result<(), Box<dyn Error>> {
    pipeline.init(args).await?;

    // Block until EOS or error message pops up
    pipeline.run().await?;

    // Clean up the pipeline when it finishes so it can be rerun
    pipeline.clean_up().await?;
    Ok(())
}

/// Stop a pipeline and clean it up.
/// This function is called once when the program exits.
async fn stop_pipeline(pipeline: &SharablePipeline) -> Result<(), Box<dyn Error>> {
    pipeline.end().await?;
    pipeline.clean_up().await?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let subscriber = get_subscriber("srt_whep".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);

    let app_data = SharableAppState::new();
    let pipeline_data = SharablePipeline::new(args.clone());
    let listener =
        TcpListener::bind(format!("0.0.0.0:{}", args.port)).expect("WHEP port is already in use");
    let should_stop = Arc::new(AtomicBool::new(false));

    let should_stop_clone = should_stop.clone();
    let mut pipeline_clone = pipeline_data.clone();
    // Run the pipeline in a separate thread
    let pipeline_thread = task::spawn(async move {
        while !should_stop_clone.load(Ordering::Relaxed) {
            if let Err(err) = run_pipeline(&mut pipeline_clone, &args).await {
                tracing::error!("Failed to run pipeline: {}", err);

                // break the loop when the pipeline runs into an error
                break;
            };

            // Reset and rerun the pipeline when it encounters EOS
            sleep(Duration::from_secs(1)).await;
            tracing::info!("Pipeline reaches EOS. Reset and rerun the pipeline");
        }

        // Stop the pipeline when the thread is aborted
        stop_pipeline(&pipeline_clone)
            .await
            .expect("Failed to stop pipeline");
    });

    // Start the http server
    run(listener, app_data, pipeline_data.clone())?.await?;

    signal::ctrl_c().await?;
    tracing::debug!("Received Ctrl-C signal");

    // Mannually stop the pipeline thread
    should_stop.store(true, Ordering::Relaxed);
    pipeline_data.end().await?;
    pipeline_thread.abort();

    Ok(())
}
