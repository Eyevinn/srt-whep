use hello_world::domain::SharableAppState;
use hello_world::pipeline::setup_pipeline;
use hello_world::startup::run;
use hello_world::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;

#[tokio::main]
async fn main() -> std::io::Result<()> {
    let subscriber = get_subscriber("hello_world".into(), "info".into(), std::io::stdout);
    init_subscriber(subscriber);

    let app_data = SharableAppState::new();
    let listener = TcpListener::bind("127.0.0.1:8000").expect("Failed to bind random port");
   
    // Run the pipeline in a separate thread
    let t = tokio::spawn(async move {
        setup_pipeline().expect("Failed to setup pipeline");
    });

    run(listener, app_data)?.await?;
    t.await.expect("Failed to stop server");
    
    Ok(())
}
