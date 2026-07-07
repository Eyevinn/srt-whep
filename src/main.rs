use clap::Parser;
use srt_whep::signal::CoordinatorConfig;
use srt_whep::startup::Application;
use srt_whep::stream::{Args, SharablePipeline};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::error::Error;
use std::net::TcpListener;
use tokio::signal;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let subscriber = get_subscriber("srt_whep".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);

    let pipeline = SharablePipeline::new(args.clone());
    let listener =
        TcpListener::bind(format!("0.0.0.0:{}", args.port)).expect("WHEP port is already in use");
    let app = Application::assemble(listener, pipeline, CoordinatorConfig::default())?;

    // One Ctrl-C stops everything: the HTTP server, the supervisor
    // (EOS → join), and with them the coordinator.
    app.run_until_stopped(async {
        let _ = signal::ctrl_c().await;
        tracing::info!("Received Ctrl-C signal");
    })
    .await?;

    Ok(())
}
