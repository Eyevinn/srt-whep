use clap::Parser;
use hello_world::domain::SharableAppState;
use hello_world::pipeline::{Args, SharablePipeline};
use hello_world::startup::run;
use hello_world::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;
use tokio::signal;
use tokio::task;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let subscriber = get_subscriber("hello_world".into(), "info".into(), std::io::stdout);
    init_subscriber(subscriber);

    let app_data = SharableAppState::new(args.clone());
    let pipeline_data = SharablePipeline::new(args.clone());
    let listener =
        TcpListener::bind(format!("127.0.0.1:{}", args.port)).expect("Whep port is already in use");

    let p2 = pipeline_data.clone();
    // Run the pipeline in a separate thread
    let t = task::spawn(async move {
        p2.setup_pipeline(&args).unwrap();
    });

    run(listener, app_data, pipeline_data)?.await?;
    t.await.expect("Failed to stop server");

    match signal::ctrl_c().await {
        Ok(()) => {}
        Err(err) => {
            eprintln!("Unable to listen for shutdown signal: {}", err);
            // we also shut down in case of error
        }
    }

    Ok(())
}
