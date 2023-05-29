use clap::Parser;
use hello_world::domain::SharableAppState;
use hello_world::pipeline::Args;
use hello_world::startup::run;
use hello_world::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;
use tokio::signal;  

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let args = Args::parse();

    let subscriber = get_subscriber("hello_world".into(), "info".into(), std::io::stdout);
    init_subscriber(subscriber);

    let app_data = SharableAppState::new(args.clone());
    let listener =
        TcpListener::bind(format!("127.0.0.1:{}", args.port)).expect("Whep port is already in use");

    // Run the pipeline in a separate thread
    
    run(listener, app_data)?.await?;
    //_t.await.expect("Failed to stop server");

    match signal::ctrl_c().await {
        Ok(()) => {},
        Err(err) => {
            eprintln!("Unable to listen for shutdown signal: {}", err);
            // we also shut down in case of error
        },
    }

    Ok(())
}
