use anyhow::{Error, Ok};
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use std::sync::Arc;

#[derive(Parser, Debug, Clone)]
#[command(author, version, about, long_about = None)]
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
    pub port: u32,
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
#[async_trait]
pub trait BranchControl: Clone + Send + Sync {
    async fn ready(&self) -> Result<bool, Error>;
    async fn add_branch(&self, id: String) -> Result<(), Error>;
    async fn remove_branch(&self, id: String) -> Result<(), Error>;
    async fn quit(&self) -> Result<(), Error>;
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
}

/// A recording fake for unit and integration tests: `ready` is settable,
/// every call is recorded for assertions, and `run()` parks until released
/// — by `finish_run`/`fail_run` from a test, or by `end`/`quit` exactly
/// like EOS / forced-quit resolve the real pipeline's `run()`.
#[derive(Clone, Default)]
pub struct TestPipeline {
    state: Arc<std::sync::Mutex<TestPipelineState>>,
    run_gate: Arc<tokio::sync::Notify>,
}

impl TestPipeline {
    pub fn set_ready(&self, ready: bool) {
        self.state.lock().unwrap().ready = ready;
    }

    pub fn snapshot(&self) -> TestPipelineState {
        self.state.lock().unwrap().clone()
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
    async fn ready(&self) -> Result<bool, Error> {
        Ok(self.state.lock().unwrap().ready)
    }

    async fn add_branch(&self, id: String) -> Result<(), Error> {
        self.state.lock().unwrap().added.push(id);
        Ok(())
    }

    async fn remove_branch(&self, id: String) -> Result<(), Error> {
        self.state.lock().unwrap().removed.push(id);
        Ok(())
    }

    async fn quit(&self) -> Result<(), Error> {
        self.state.lock().unwrap().quit_count += 1;
        self.run_gate.notify_one();
        Ok(())
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
