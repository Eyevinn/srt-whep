//! The pipeline supervisor: run the pipeline; when it stops (EOS or error),
//! clean it up and reset signaling; rerun it with backoff — until shutdown.
//!
//! This is the one place that knows the restart policy, the cleanup/reset
//! contract with the coordinator, and the shutdown ordering (EOS → join).
use crate::signal::ResetSignal;
use crate::stream::PipelineLifecycle;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const BASE_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);
const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
const RESET_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Supervisor<P: PipelineLifecycle, S: ResetSignal> {
    pipeline: P,
    signal: S,
    shutdown: watch::Receiver<bool>,
    restart_rx: mpsc::Receiver<()>,
}

impl<P: PipelineLifecycle + 'static, S: ResetSignal + 'static> Supervisor<P, S> {
    /// Spawn the supervision loop. It runs until the shutdown channel reads
    /// `true` (or its sender is dropped).
    pub fn spawn(
        pipeline: P,
        signal: S,
        shutdown: watch::Receiver<bool>,
        restart_rx: mpsc::Receiver<()>,
    ) -> JoinHandle<()> {
        tokio::spawn(
            Self {
                pipeline,
                signal,
                shutdown,
                restart_rx,
            }
            .run(),
        )
    }

    async fn run(mut self) {
        let mut consecutive_failures: u32 = 0;
        loop {
            if *self.shutdown.borrow() {
                break;
            }

            let outcome = self.run_pipeline_until_stopped().await;

            // Explicit, always: clean the pipeline so it can be rerun, and
            // fail all in-flight handshakes so no waiter outlives the run.
            self.cleanup().await;

            match outcome {
                RunOutcome::ShuttingDown => break,
                RunOutcome::Restart => {
                    consecutive_failures = 0; // base-delay rerun, like a clean run
                    tracing::info!("Watchdog requested a restart. Reset and rerun the pipeline.");
                }
                RunOutcome::Completed(Ok(())) => {
                    consecutive_failures = 0;
                    tracing::info!("Pipeline reached EOS. Reset and rerun the pipeline.");
                }
                RunOutcome::Completed(Err(e)) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    tracing::error!("Pipeline stopped with an error: {}", e);
                }
            }

            tokio::select! {
                _ = tokio::time::sleep(backoff_delay(consecutive_failures)) => {}
                _ = wait_for_shutdown(&mut self.shutdown) => break,
            }
        }
        tracing::info!("Pipeline supervisor stopped");
    }

    /// One init→run cycle. Resolves when the pipeline stops on its own
    /// (EOS or error) or, on shutdown, after asking it to stop and joining
    /// it (bounded — a wedged element must not hang process exit).
    async fn run_pipeline_until_stopped(&mut self) -> RunOutcome {
        // Drop any restart request left buffered by the previous run before
        // starting a fresh one. No legitimate request can be pending here: the
        // watchdog only trips against a ready, running pipeline, and between
        // runs the pipeline is Null/re-initializing — so this only clears a
        // stale `()` that raced a natural EOS/error on the prior run's final
        // tick, which would otherwise force-quit this fresh run for nothing.
        while self.restart_rx.try_recv().is_ok() {}

        if let Err(e) = self.pipeline.init().await {
            return RunOutcome::Completed(Err(e));
        }

        // The real pipeline's run() parks its worker thread in the GLib main
        // loop, so it must live on its own task; only something from outside
        // unblocks it: an EOS (end() below), a fatal bus error, or the
        // supervisor's forced quit() in the restart arm (on a watchdog request).
        let mut run_task = tokio::spawn({
            let pipeline = self.pipeline.clone();
            async move { pipeline.run().await }
        });

        tokio::select! {
            res = &mut run_task => RunOutcome::Completed(flatten_join(res)),
            _ = wait_for_shutdown(&mut self.shutdown) => {
                let _ = self.pipeline.end().await;
                match tokio::time::timeout(SHUTDOWN_JOIN_TIMEOUT, &mut run_task).await {
                    Ok(joined) => {
                        if let Err(e) = flatten_join(joined) {
                            tracing::warn!("Pipeline stopped with an error during shutdown: {}", e);
                        }
                    }
                    Err(_) => {
                        run_task.abort();
                        tracing::error!(
                            "Pipeline did not stop within {:?} after EOS; abandoning it",
                            SHUTDOWN_JOIN_TIMEOUT
                        );
                    }
                }
                RunOutcome::ShuttingDown
            }
            Some(()) = self.restart_rx.recv() => {
                // Watchdog asked for a restart: force the run down, then join
                // (bounded — a wedged quit must not hang the loop).
                let _ = self.pipeline.quit().await;
                match tokio::time::timeout(SHUTDOWN_JOIN_TIMEOUT, &mut run_task).await {
                    Ok(joined) => {
                        if let Err(e) = flatten_join(joined) {
                            tracing::warn!("Pipeline errored during watchdog restart: {}", e);
                        }
                    }
                    Err(_) => {
                        run_task.abort();
                        tracing::error!(
                            "Pipeline did not stop within {:?} after watchdog quit; abandoning it",
                            SHUTDOWN_JOIN_TIMEOUT
                        );
                    }
                }
                RunOutcome::Restart
            }
        }
    }

    async fn cleanup(&self) {
        if let Err(e) = self.pipeline.clean_up().await {
            tracing::error!("Failed to clean up pipeline: {}", e);
        }
        // Bounded: the reset command shares the coordinator's single-threaded
        // queue, so a wedged coordinator could otherwise hang the restart loop
        // indefinitely (the shutdown path is already bounded; this one wasn't).
        match tokio::time::timeout(RESET_TIMEOUT, self.signal.reset()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!("Failed to reset signaling state: {}", e),
            Err(_) => tracing::error!("Signaling reset timed out after {:?}", RESET_TIMEOUT),
        }
    }
}

enum RunOutcome {
    /// The pipeline stopped on its own: cleanly (EOS) or with an error.
    Completed(Result<(), anyhow::Error>),
    /// A shutdown token arrived; the pipeline was stopped and joined.
    ShuttingDown,
    /// The watchdog requested a restart; the run was force-quit and joined.
    Restart,
}

/// Resolves when shutdown is requested. A dropped sender means the
/// application is tearing down, so it counts as a request too.
async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    let _ = shutdown.wait_for(|&stop| stop).await;
}

fn backoff_delay(consecutive_failures: u32) -> Duration {
    match consecutive_failures {
        0 | 1 => BASE_RESTART_DELAY,
        n => BASE_RESTART_DELAY
            .saturating_mul(2u32.saturating_pow(n - 1).min(64))
            .min(MAX_RESTART_DELAY),
    }
}

fn flatten_join(
    res: Result<Result<(), anyhow::Error>, tokio::task::JoinError>,
) -> Result<(), anyhow::Error> {
    res.map_err(anyhow::Error::from).and_then(|r| r)
}

#[cfg(test)]
mod tests {
    use super::Supervisor;
    use crate::signal::{ResetSignal, SignalError};
    use crate::stream::{TestPipeline, TestPipelineState};
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::{mpsc, watch};

    /// Recording adapter for the supervisor's reset capability: counts the
    /// reset calls; `wedged` makes them hang forever, standing in for a dead
    /// coordinator. The coordinator's side of the contract is pinned by its
    /// own tests — no coordinator is spawned here.
    #[derive(Clone, Default)]
    struct RecordingReset {
        calls: Arc<AtomicU32>,
        wedged: bool,
    }

    impl RecordingReset {
        fn wedged() -> Self {
            Self {
                calls: Arc::default(),
                wedged: true,
            }
        }

        fn count(&self) -> u32 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl ResetSignal for RecordingReset {
        async fn reset(&self) -> Result<(), SignalError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if self.wedged {
                std::future::pending::<()>().await;
            }
            Ok(())
        }
    }

    /// Build the reset fake + restart channel for a supervisor test. The
    /// returned `restart_tx` lets a test drive a watchdog restart directly,
    /// and `restart_rx` is handed to `Supervisor::spawn`.
    fn wire() -> (RecordingReset, mpsc::Sender<()>, mpsc::Receiver<()>) {
        let (restart_tx, restart_rx) = mpsc::channel(1);
        (RecordingReset::default(), restart_tx, restart_rx)
    }

    /// Poll under the paused clock until `f` holds. 1000 × 10ms sleeps give
    /// a 10s virtual-time budget — enough to cross RESET_TIMEOUT (5s) plus a
    /// restart backoff, which the wedged-reset test needs.
    async fn wait_until(pipeline: &TestPipeline, f: impl Fn(&TestPipelineState) -> bool) {
        for _ in 0..1000 {
            if f(&pipeline.snapshot()) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("condition never reached: {:?}", pipeline.snapshot());
    }

    #[tokio::test(start_paused = true)]
    async fn restarts_after_a_failed_run() {
        let pipeline = TestPipeline::default();
        let (reset, _restart_tx, restart_rx) = wire();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), reset, shutdown_rx, restart_rx);

        wait_until(&pipeline, |s| s.run_count == 1).await;
        pipeline.fail_run("gst blew up");

        // Cleanup ran, then a fresh init/run cycle started.
        wait_until(&pipeline, |s| s.cleanup_count == 1).await;
        wait_until(&pipeline, |s| s.run_count == 2).await;
        assert_eq!(2, pipeline.snapshot().init_count);
    }

    #[tokio::test(start_paused = true)]
    async fn reset_is_requested_after_every_stop() {
        // The supervisor's side of the restart contract: signaling is reset
        // after *every* stop — error, clean EOS, watchdog restart, shutdown —
        // so no waiter outlives the run it was created against. The
        // coordinator's side (a reset fails all in-flight waiters and clears
        // the connection map) is pinned by
        // `reset_fails_all_waiters_and_clears_state` in signal::coordinator.
        let pipeline = TestPipeline::default();
        let (reset, restart_tx, restart_rx) = wire();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sup = Supervisor::spawn(pipeline.clone(), reset.clone(), shutdown_rx, restart_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        pipeline.fail_run("gst blew up"); // stop 1: error
        wait_until(&pipeline, |s| s.run_count == 2).await;
        assert_eq!(1, reset.count());

        pipeline.finish_run(); // stop 2: clean EOS
        wait_until(&pipeline, |s| s.run_count == 3).await;
        assert_eq!(2, reset.count());

        restart_tx.send(()).await.unwrap(); // stop 3: watchdog restart
        wait_until(&pipeline, |s| s.run_count == 4).await;
        assert_eq!(3, reset.count());

        shutdown_tx.send(true).unwrap(); // stop 4: shutdown
        sup.await.unwrap();
        assert_eq!(4, reset.count());
    }

    #[tokio::test(start_paused = true)]
    async fn a_wedged_reset_does_not_hang_the_restart_loop() {
        // RESET_TIMEOUT is the bound at the cleanup site: a coordinator that
        // never answers must not wedge the supervisor. The rerun still
        // happens once the timeout fires (paused clock — the 5s elapse
        // virtually).
        let pipeline = TestPipeline::default();
        let reset = RecordingReset::wedged();
        let (_restart_tx, restart_rx) = mpsc::channel(1);
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), reset.clone(), shutdown_rx, restart_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        pipeline.fail_run("gst blew up");

        wait_until(&pipeline, |s| s.run_count == 2).await;
        assert_eq!(1, reset.count()); // attempted exactly once, then timed out
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_sends_eos_joins_and_stops_the_loop() {
        let pipeline = TestPipeline::default();
        let (reset, _restart_tx, restart_rx) = wire();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sup = Supervisor::spawn(pipeline.clone(), reset, shutdown_rx, restart_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        shutdown_tx.send(true).unwrap();
        sup.await.unwrap();

        let snap = pipeline.snapshot();
        assert_eq!(1, snap.end_count); // graceful EOS was requested
        assert_eq!(1, snap.cleanup_count); // cleaned up exactly once
        assert_eq!(1, snap.run_count); // and never restarted
    }

    #[tokio::test(start_paused = true)]
    async fn dropped_shutdown_sender_also_stops_the_loop() {
        let pipeline = TestPipeline::default();
        let (reset, _restart_tx, restart_rx) = wire();
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sup = Supervisor::spawn(pipeline.clone(), reset, shutdown_rx, restart_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        drop(shutdown_tx);
        sup.await.unwrap();
        assert_eq!(1, pipeline.snapshot().cleanup_count);
    }

    #[tokio::test(start_paused = true)]
    async fn restart_request_restarts_like_a_clean_run() {
        // A watchdog restart request force-quits the run and reruns, exactly
        // like EOS — cleanup runs and the pipeline is rerun at base delay.
        let pipeline = TestPipeline::default();
        let (reset, restart_tx, restart_rx) = wire();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), reset, shutdown_rx, restart_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        restart_tx.send(()).await.unwrap();

        wait_until(&pipeline, |s| s.quit_count == 1).await; // supervisor force-quit
        wait_until(&pipeline, |s| s.cleanup_count == 1).await;
        wait_until(&pipeline, |s| s.run_count == 2).await;
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_doubles_on_consecutive_failures_and_resets_on_success() {
        let pipeline = TestPipeline::default();
        let (reset, _restart_tx, restart_rx) = wire();
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), reset, shutdown_rx, restart_rx);

        wait_until(&pipeline, |s| s.run_count == 1).await;
        let t0 = tokio::time::Instant::now();
        pipeline.fail_run("1st");
        wait_until(&pipeline, |s| s.run_count == 2).await;
        let first_gap = t0.elapsed();

        let t1 = tokio::time::Instant::now();
        pipeline.fail_run("2nd");
        wait_until(&pipeline, |s| s.run_count == 3).await;
        let second_gap = t1.elapsed();

        // Paused clock: gaps are timer-driven. The second delay must be
        // roughly double the first (slack for the polling quanta).
        assert!(
            second_gap >= first_gap * 2 - Duration::from_millis(200),
            "expected doubled backoff, got {:?} then {:?}",
            first_gap,
            second_gap
        );

        // A clean EOS run resets the backoff to its base.
        let t2 = tokio::time::Instant::now();
        pipeline.finish_run();
        wait_until(&pipeline, |s| s.run_count == 4).await;
        let after_success_gap = t2.elapsed();
        assert!(
            after_success_gap < second_gap,
            "expected backoff reset, got {:?} after {:?}",
            after_success_gap,
            second_gap
        );
    }
}
