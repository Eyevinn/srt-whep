use clap::Parser;
use srt_whep::signal::{spawn_coordinator, CoordinatorConfig};
use srt_whep::startup::run;
use srt_whep::stream::{Args, PipelineLifecycle, SharablePipeline};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use srt_whep::utils::PipelineGuard;
use std::error::Error;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::signal;
use tokio::task;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let subscriber = get_subscriber("srt_whep".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);

    let pipeline = SharablePipeline::new(args.clone());
    let signal_handle = spawn_coordinator(pipeline.clone(), CoordinatorConfig::default());
    let listener =
        TcpListener::bind(format!("0.0.0.0:{}", args.port)).expect("WHEP port is already in use");
    let should_stop = Arc::new(AtomicBool::new(false));

    let should_stop_clone = should_stop.clone();
    let signal_clone = signal_handle.clone();
    let pipeline_clone = pipeline.clone();

    // Run the pipeline in a separate thread
    let pipeline_thread = task::spawn(async move {
        let mut loops = 0;
        while !should_stop_clone.load(Ordering::Relaxed) {
            tracing::debug!("Looping pipeline: {}", loops);
            loops += 1;

            let pipeline_guard = PipelineGuard::new(pipeline_clone.clone(), signal_clone.clone());

            if let Err(err) = pipeline_guard.run().await {
                tracing::error!("Pipeline runs into error: {}", err);
            } else {
                tracing::info!("Pipeline reaches EOS. Reset and rerun the pipeline.");
            }

            sleep(Duration::from_secs(1)).await;
        }
    });

    // Start the http server
    run(listener, signal_handle)?.await?;

    signal::ctrl_c().await?;
    tracing::debug!("Received Ctrl-C signal");

    // Manually stop the pipeline thread
    should_stop.store(true, Ordering::Relaxed);
    pipeline.end().await?;
    pipeline_thread.await?;

    Ok(())
}
