use clap::Parser;
use srt_whep::signal::CoordinatorArgs;
use srt_whep::startup::Application;
use srt_whep::stream::{Args, SharablePipeline};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::error::Error;
use std::net::TcpListener;
use tokio::sync::mpsc;

/// srt-whep: SRT to WHEP (WebRTC) gateway.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    pipeline: Args,
    #[command(flatten)]
    coordinator: CoordinatorArgs,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    let subscriber = get_subscriber("srt_whep".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);

    // The bus-reap channel: the pipeline holds the sender (from birth), the
    // coordinator (inside `assemble`) the receiver. Created here so both ends
    // exist before either is spawned.
    let (branch_failures_tx, branch_failures_rx) = mpsc::channel(64);
    let pipeline = SharablePipeline::new(cli.pipeline.clone(), branch_failures_tx);
    let listener = TcpListener::bind(format!("0.0.0.0:{}", cli.pipeline.port))
        .expect("WHEP port is already in use");
    let app = Application::assemble(
        listener,
        pipeline,
        cli.coordinator.to_config(),
        Some(cli.pipeline.port),
        branch_failures_rx,
    )?;

    // Any termination signal stops everything gracefully: the HTTP server
    // (drain), the supervisor (EOS → NULL-state cleanup → join), and with
    // them the coordinator. `disable_signals()` removed actix's own handlers,
    // so SIGTERM/SIGQUIT (docker stop, k8s) must be re-established here
    // alongside SIGINT — otherwise those would hard-kill with no drain.
    app.run_until_stopped(shutdown_signal()).await?;

    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");
    let mut sigquit = signal(SignalKind::quit()).expect("failed to install SIGQUIT handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("Received SIGINT (Ctrl-C)"),
        _ = sigterm.recv() => tracing::info!("Received SIGTERM"),
        _ = sigquit.recv() => tracing::info!("Received SIGQUIT"),
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
    tracing::info!("Received Ctrl-C signal");
}
