use crate::stream::errors::PipelineError;
use anyhow::Error;
use async_trait::async_trait;
use clap::ValueEnum;
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(clap::Args, Debug, Clone)]
pub struct Args {
    /// SRT source stream address(ip:port)
    #[clap(short, long)]
    pub input_address: String,

    /// SRT mode to use:
    /// 1) caller - run a discoverer and then connect to the SRT stream (in listener mode).
    /// 2) listener - wait for a SRT stream (in caller mode) to connect.
    #[clap(short, long, value_enum, verbatim_doc_comment, default_value_t = SRTMode::Caller)]
    pub srt_mode: SRTMode,

    /// SRT stream latency in milliseconds
    /// As the stream receiver, increasing this value will smooth out possible network jitters
    /// but will also add latency to the preview.
    #[clap(long, default_value_t = 0)]
    pub srt_latency: u32,

    /// TSDemux latency in milliseconds
    /// Latency to add for smooth demuxing MPEG2 transport streams
    #[clap(long, default_value_t = 0)]
    pub tsdemux_latency: u32,

    /// Run discoverer before connecting to the SRT stream
    #[clap(short, long, default_value_t = false)]
    pub run_discoverer: bool,

    /// Timeout for discoverer in seconds
    #[clap(short, long, default_value_t = 10)]
    pub discoverer_timeout_sec: u64,

    /// SRT output stream address(ip:port)
    #[clap(short, long, default_value_t = String::from("127.0.0.1:8888"))]
    pub output_address: String,

    /// Port for whep client
    #[clap(short, long, default_value_t = 8000)]
    pub port: u16,
}

#[derive(ValueEnum, Clone, Debug, PartialEq)]
pub enum SRTMode {
    Caller,
    Listener,
}

impl SRTMode {
    pub fn to_str(&self) -> &str {
        match self {
            SRTMode::Caller => "caller",
            SRTMode::Listener => "listener",
        }
    }

    pub fn reverse(&self) -> Self {
        match self {
            SRTMode::Caller => SRTMode::Listener,
            SRTMode::Listener => SRTMode::Caller,
        }
    }
}

/// The coordinator's view of the pipeline: per-viewer branch control.
///
/// `ready` gates branch creation: `add_branch` may only succeed once the
/// input stream is demuxed and the output tees exist. `add_branch` /
/// `remove_branch` attach and detach one viewer's WHEP output branch.
/// `quit` force-restarts the whole pipeline; the coordinator's watchdog
/// calls it when consecutive handshake failures suggest a wedge.
///
/// Errors are typed for policy: [`PipelineError::NotReady`] and
/// [`PipelineError::Transient`] are worth a retry, [`PipelineError::Fatal`]
/// is not.
#[async_trait]
pub trait BranchControl: Clone + Send + Sync {
    async fn ready(&self) -> Result<bool, PipelineError>;
    async fn add_branch(&self, id: String) -> Result<(), PipelineError>;
    async fn remove_branch(&self, id: String) -> Result<(), PipelineError>;
    async fn quit(&self) -> Result<(), PipelineError>;

    /// Install the sink the pipeline reports a per-viewer branch's runtime
    /// failure to (observed on its message bus) so the coordinator can reap
    /// that dead branch's connection instead of leaking it. Called once at
    /// wiring time. Default: a no-op — a fake without a real bus never
    /// reports one.
    fn set_branch_failure_sink(&self, _sink: mpsc::Sender<String>) {}
}

/// The supervisor's view of the pipeline: whole-pipeline lifecycle.
///
/// Call order: `init` → `run` (resolves only at EOS or a fatal error) →
/// `clean_up`, after which `init` may be called again. `end` requests EOS
/// from outside that loop for a graceful shutdown.
#[async_trait]
pub trait PipelineLifecycle: Clone + Send + Sync {
    async fn init(&self) -> Result<(), Error>;
    async fn run(&self) -> Result<(), Error>;
    async fn end(&self) -> Result<(), Error>;
    async fn clean_up(&self) -> Result<(), Error>;
}

/// Snapshot of everything a test pipeline has recorded.
#[derive(Clone, Debug, Default)]
pub struct TestPipelineState {
    pub ready: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub quit_count: u32,
    pub init_count: u32,
    pub run_count: u32,
    pub end_count: u32,
    pub cleanup_count: u32,
    next_run_error: Option<String>,
    block_remove_branch: bool,
}

/// A recording fake for unit and integration tests: `ready` is settable,
/// every call is recorded for assertions, and `run()` parks until released
/// — by `finish_run`/`fail_run` from a test, or by `end`/`quit` exactly
/// like EOS / forced-quit resolve the real pipeline's `run()`.
#[derive(Clone, Default)]
pub struct TestPipeline {
    state: Arc<std::sync::Mutex<TestPipelineState>>,
    run_gate: Arc<tokio::sync::Notify>,
    add_branch_error: Arc<std::sync::Mutex<Option<PipelineError>>>,
    remove_branch_error: Arc<std::sync::Mutex<Option<PipelineError>>>,
    branch_failures: Arc<std::sync::Mutex<Option<mpsc::Sender<String>>>>,
}

impl TestPipeline {
    pub fn set_ready(&self, ready: bool) {
        self.state.lock().unwrap().ready = ready;
    }

    pub fn snapshot(&self) -> TestPipelineState {
        self.state.lock().unwrap().clone()
    }

    /// Make the next `add_branch` call fail with the given error.
    pub fn fail_next_add_branch(&self, err: PipelineError) {
        *self.add_branch_error.lock().unwrap() = Some(err);
    }

    /// Make the next `remove_branch` call fail with the given error.
    pub fn fail_next_remove_branch(&self, err: PipelineError) {
        *self.remove_branch_error.lock().unwrap() = Some(err);
    }

    /// Make every `remove_branch` call hang forever, simulating a wedged
    /// GStreamer teardown, so the coordinator's teardown timeout is exercised.
    pub fn block_remove_branch(&self) {
        self.state.lock().unwrap().block_remove_branch = true;
    }

    /// Simulate the bus watch reporting a per-viewer branch's runtime
    /// failure (its whipsink errored / its peer went away), exactly as the
    /// real pipeline does, so the coordinator reaps that connection.
    pub fn fail_branch(&self, id: &str) {
        if let Some(sink) = self.branch_failures.lock().unwrap().clone() {
            let _ = sink.try_send(id.to_string());
        }
    }

    /// Release a parked `run()` as a clean EOS.
    pub fn finish_run(&self) {
        self.run_gate.notify_one();
    }

    /// Release a parked `run()` with an error.
    pub fn fail_run(&self, msg: &str) {
        self.state.lock().unwrap().next_run_error = Some(msg.to_string());
        self.run_gate.notify_one();
    }
}

#[async_trait]
impl BranchControl for TestPipeline {
    async fn ready(&self) -> Result<bool, PipelineError> {
        Ok(self.state.lock().unwrap().ready)
    }

    async fn add_branch(&self, id: String) -> Result<(), PipelineError> {
        if let Some(err) = self.add_branch_error.lock().unwrap().take() {
            return Err(err);
        }
        // Mirror the real adapter: a branch cannot be added to a not-ready input.
        if !self.state.lock().unwrap().ready {
            return Err(PipelineError::NotReady);
        }
        self.state.lock().unwrap().added.push(id);
        Ok(())
    }

    async fn remove_branch(&self, id: String) -> Result<(), PipelineError> {
        if let Some(err) = self.remove_branch_error.lock().unwrap().take() {
            return Err(err);
        }
        if self.state.lock().unwrap().block_remove_branch {
            // A wedged teardown that never resolves; the coordinator's
            // teardown timeout is what unblocks the actor.
            std::future::pending::<()>().await;
        }
        self.state.lock().unwrap().removed.push(id);
        Ok(())
    }

    async fn quit(&self) -> Result<(), PipelineError> {
        self.state.lock().unwrap().quit_count += 1;
        self.run_gate.notify_one();
        Ok(())
    }

    fn set_branch_failure_sink(&self, sink: mpsc::Sender<String>) {
        *self.branch_failures.lock().unwrap() = Some(sink);
    }
}

#[async_trait]
impl PipelineLifecycle for TestPipeline {
    async fn init(&self) -> Result<(), Error> {
        self.state.lock().unwrap().init_count += 1;
        Ok(())
    }

    async fn run(&self) -> Result<(), Error> {
        self.state.lock().unwrap().run_count += 1;
        self.run_gate.notified().await;
        match self.state.lock().unwrap().next_run_error.take() {
            Some(msg) => Err(anyhow::anyhow!(msg)),
            None => Ok(()),
        }
    }

    async fn end(&self) -> Result<(), Error> {
        self.state.lock().unwrap().end_count += 1;
        self.run_gate.notify_one();
        Ok(())
    }

    async fn clean_up(&self) -> Result<(), Error> {
        self.state.lock().unwrap().cleanup_count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{BranchControl, PipelineLifecycle, TestPipeline};
    use crate::stream::errors::PipelineError;

    #[tokio::test]
    async fn add_branch_on_a_not_ready_fake_is_not_ready() {
        let pipeline = TestPipeline::default(); // ready = false
        assert!(matches!(
            pipeline.add_branch("a".to_string()).await,
            Err(PipelineError::NotReady)
        ));
        assert!(pipeline.snapshot().added.is_empty());
    }

    #[tokio::test]
    async fn test_pipeline_records_calls() {
        let pipeline = TestPipeline::default();
        assert!(!pipeline.ready().await.unwrap());

        pipeline.set_ready(true);
        assert!(pipeline.ready().await.unwrap());

        pipeline.add_branch("a".to_string()).await.unwrap();
        pipeline.remove_branch("a".to_string()).await.unwrap();
        pipeline.quit().await.unwrap();

        let snap = pipeline.snapshot();
        assert_eq!(vec!["a".to_string()], snap.added);
        assert_eq!(vec!["a".to_string()], snap.removed);
        assert_eq!(1, snap.quit_count);
    }

    #[tokio::test]
    async fn test_pipeline_run_parks_until_released() {
        let pipeline = TestPipeline::default();

        let runner = {
            let pipeline = pipeline.clone();
            tokio::spawn(async move { pipeline.run().await })
        };
        tokio::task::yield_now().await;
        assert_eq!(1, pipeline.snapshot().run_count);
        assert!(!runner.is_finished());

        pipeline.fail_run("boom");
        let result = runner.await.unwrap();
        assert_eq!("boom", result.unwrap_err().to_string());

        // end() releases a parked run with Ok (EOS semantics).
        let runner = {
            let pipeline = pipeline.clone();
            tokio::spawn(async move { pipeline.run().await })
        };
        tokio::task::yield_now().await;
        pipeline.end().await.unwrap();
        assert!(runner.await.unwrap().is_ok());

        pipeline.init().await.unwrap();
        pipeline.clean_up().await.unwrap();
        let snap = pipeline.snapshot();
        assert_eq!(1, snap.init_count);
        assert_eq!(2, snap.run_count);
        assert_eq!(1, snap.end_count);
        assert_eq!(1, snap.cleanup_count);
    }
}
