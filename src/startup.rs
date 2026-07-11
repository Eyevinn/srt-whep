use crate::routes::*;
use crate::signal::{spawn_coordinator, CoordinatorConfig, SignalHandle};
use crate::stream::{BranchControl, BranchId, PipelineLifecycle, WHIP_SINK_ROUTE};
use crate::supervisor::Supervisor;
use actix_cors::Cors;
use actix_web::dev::Server;
use actix_web::{guard, web, App, HttpServer};
use std::future::Future;
use std::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing_actix_web::TracingLogger;

pub fn run(listener: TcpListener, signal: SignalHandle) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .route("/list", web::get().to(list))
            .route("/channel", web::post().to(whep_handler))
            .route("/channel", web::route().guard(guard::Options()).to(options))
            .route("/channel/{id}", web::patch().to(whep_patch_handler))
            .route("/channel/{id}", web::delete().to(terminate_session))
            .route(WHIP_SINK_ROUTE, web::post().to(whip_handler))
            .route(WHIP_SINK_ROUTE, web::delete().to(remove_whip_sink))
            .app_data(web::Data::new(signal.clone()))
    })
    // Shutdown is owned by Application::run_until_stopped (one Ctrl-C);
    // actix must not install its own SIGINT handler.
    .disable_signals()
    .listen(listener)?
    .run();

    Ok(server)
}

/// The assembled application: coordinator + supervisor + HTTP server,
/// wired in exactly one place — used by `main`, the signaling integration
/// tests, and the GStreamer e2e test.
pub struct Application {
    server: Server,
    supervisor: JoinHandle<()>,
    signal: SignalHandle,
    shutdown: watch::Sender<bool>,
    port: u16,
}

impl Application {
    pub fn assemble<P>(
        listener: TcpListener,
        pipeline: P,
        config: CoordinatorConfig,
        expected_whip_port: Option<u16>,
        branch_failures: mpsc::Receiver<BranchId>,
    ) -> Result<Self, std::io::Error>
    where
        P: BranchControl + PipelineLifecycle + 'static,
    {
        let port = listener.local_addr()?.port();
        // The pipeline's whipclientsink posts loopback WHIP offers to a fixed
        // port; if the HTTP server is bound elsewhere those offers 404 silently.
        // Fail loudly at wiring time instead. (This coupling exists only for the
        // loopback WHIP bridge — see src/stream/branch.rs — and is deleted with
        // the whepserversink migration, ADR 0001.)
        if let Some(expected) = expected_whip_port {
            if expected != port {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!(
                        "HTTP listener bound to port {port} but the pipeline posts \
                         loopback WHIP offers to port {expected}; they must match"
                    ),
                ));
            }
        }
        // The watchdog restart channel: the coordinator holds the sender (it
        // requests a restart on a trip), the supervisor the receiver (it owns
        // the force-quit + rerun). Created here so both ends exist before
        // either task is spawned — symmetric with the bus-reap channel.
        let (restart_tx, restart_rx) = mpsc::channel(1);
        let (signal, reset) =
            spawn_coordinator(pipeline.clone(), config, branch_failures, restart_tx);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let supervisor = Supervisor::spawn(pipeline, reset, shutdown_rx, restart_rx);
        let server = run(listener, signal.clone())?;
        Ok(Self {
            server,
            supervisor,
            signal,
            shutdown: shutdown_tx,
            port,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn signal(&self) -> SignalHandle {
        self.signal.clone()
    }

    /// Serve until `stop` resolves (or the server dies on its own), then
    /// shut down in order: one token fans out to the supervisor (EOS →
    /// bounded join) and a graceful HTTP stop. The coordinator ends when
    /// its last handle drops with the Application.
    pub async fn run_until_stopped(
        self,
        stop: impl Future<Output = ()>,
    ) -> Result<(), std::io::Error> {
        let server_handle = self.server.handle();
        let mut server_task = tokio::spawn(self.server);

        let server_early_exit = tokio::select! {
            _ = stop => None,
            res = &mut server_task => Some(res),
        };

        let _ = self.shutdown.send(true);

        match server_early_exit {
            None => {
                // Stop the pipeline and drain HTTP concurrently.
                let (_, _) = tokio::join!(server_handle.stop(true), async {
                    let _ = self.supervisor.await;
                });
                let _ = server_task.await;
                Ok(())
            }
            Some(res) => {
                // The server stopped on its own; still stop the pipeline,
                // then surface the server's result.
                let _ = self.supervisor.await;
                match res {
                    Ok(server_result) => server_result,
                    Err(join_error) => Err(std::io::Error::other(join_error)),
                }
            }
        }
    }
}
