//! The pipeline supervisor: run the pipeline; when it stops (EOS or error),
//! clean it up and reset signaling; rerun it with backoff — until shutdown.
//!
//! This is the one place that knows the restart policy, the cleanup/reset
//! contract with the coordinator, and the shutdown ordering (EOS → join).
use crate::signal::SignalHandle;
use crate::stream::PipelineLifecycle;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

const BASE_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);
const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_secs(5);
const RESET_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Supervisor<P: PipelineLifecycle> {
    pipeline: P,
    signal: SignalHandle,
    shutdown: watch::Receiver<bool>,
}

impl<P: PipelineLifecycle + 'static> Supervisor<P> {
    /// Spawn the supervision loop. It runs until the shutdown channel reads
    /// `true` (or its sender is dropped).
    pub fn spawn(
        pipeline: P,
        signal: SignalHandle,
        shutdown: watch::Receiver<bool>,
    ) -> JoinHandle<()> {
        tokio::spawn(
            Self {
                pipeline,
                signal,
                shutdown,
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
        if let Err(e) = self.pipeline.init().await {
            return RunOutcome::Completed(Err(e));
        }

        // The real pipeline's run() parks its worker thread in the GLib
        // main loop, so it must live on its own task; only an EOS/quit from
        // outside (end() below, or the coordinator's watchdog) unblocks it.
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
    /// The pipeline stopped on its own: cleanly (EOS/quit) or with an error.
    Completed(Result<(), anyhow::Error>),
    /// A shutdown token arrived; the pipeline was stopped and joined.
    ShuttingDown,
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
    use crate::signal::{spawn_coordinator, CoordinatorConfig, SignalError, SignalHandle};
    use crate::stream::{BranchControl, TestPipeline, TestPipelineState};
    use std::time::Duration;
    use tokio::sync::{mpsc, watch};

    /// These tests exercise the supervisor, never reaping, so the coordinator
    /// gets a disconnected failure receiver (its sender dropped) and default
    /// config.
    fn spawn_coordinator_no_reaper(pipeline: TestPipeline) -> SignalHandle {
        let (_fail_tx, fail_rx) = mpsc::channel(1);
        spawn_coordinator(pipeline, CoordinatorConfig::default(), fail_rx)
    }

    async fn wait_until(pipeline: &TestPipeline, f: impl Fn(&TestPipelineState) -> bool) {
        for _ in 0..500 {
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
        let signal = spawn_coordinator_no_reaper(pipeline.clone());
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);

        wait_until(&pipeline, |s| s.run_count == 1).await;
        pipeline.fail_run("gst blew up");

        // Cleanup ran, then a fresh init/run cycle started.
        wait_until(&pipeline, |s| s.cleanup_count == 1).await;
        wait_until(&pipeline, |s| s.run_count == 2).await;
        assert_eq!(2, pipeline.snapshot().init_count);
    }

    #[tokio::test(start_paused = true)]
    async fn reset_on_cleanup_fails_inflight_handshakes() {
        let pipeline = TestPipeline::default();
        pipeline.set_ready(true);
        let signal = spawn_coordinator_no_reaper(pipeline.clone());
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        let waiter = {
            let signal = signal.clone();
            tokio::spawn(async move { signal.create_connection("a".into()).await })
        };
        wait_until(&pipeline, |s| s.added.len() == 1).await;

        pipeline.fail_run("gst blew up");
        let result = waiter.await.unwrap();
        assert!(matches!(result, Err(SignalError::Unavailable)));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_sends_eos_joins_and_stops_the_loop() {
        let pipeline = TestPipeline::default();
        let signal = spawn_coordinator_no_reaper(pipeline.clone());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);
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
        let signal = spawn_coordinator_no_reaper(pipeline.clone());
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        drop(shutdown_tx);
        sup.await.unwrap();
        assert_eq!(1, pipeline.snapshot().cleanup_count);
    }

    #[tokio::test(start_paused = true)]
    async fn quit_restarts_like_a_clean_run() {
        // The watchdog's quit() resolves run() with Ok, exactly like EOS.
        let pipeline = TestPipeline::default();
        let signal = spawn_coordinator_no_reaper(pipeline.clone());
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        pipeline.quit().await.unwrap();
        wait_until(&pipeline, |s| s.cleanup_count == 1).await;
        wait_until(&pipeline, |s| s.run_count == 2).await;
    }

    #[tokio::test(start_paused = true)]
    async fn backoff_doubles_on_consecutive_failures_and_resets_on_success() {
        let pipeline = TestPipeline::default();
        let signal = spawn_coordinator_no_reaper(pipeline.clone());
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);

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
