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

#[async_trait]
pub trait PipelineBase: Clone + Send + Sync {
    async fn add_connection(&self, id: String) -> Result<(), Error>;
    async fn remove_connection(&self, id: String) -> Result<(), Error>;

    async fn init(&mut self, args: &Args) -> Result<(), Error>;
    async fn run(&self) -> Result<(), Error>;
    async fn ready(&self) -> Result<bool, Error>;
    async fn end(&self) -> Result<(), Error>;
    async fn quit(&self) -> Result<(), Error>;
    async fn clean_up(&self) -> Result<(), Error>;

    async fn print(&self) -> Result<(), Error>;
}

/// Snapshot of everything a test pipeline has recorded.
#[derive(Clone, Debug, Default)]
pub struct TestPipelineState {
    pub ready: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub quit_count: u32,
}

/// A recording fake for unit and integration tests: `ready` is settable and
/// every connection add/remove and quit call is recorded for assertions.
#[derive(Clone, Default)]
pub struct TestPipeline(Arc<std::sync::Mutex<TestPipelineState>>);

impl TestPipeline {
    pub fn set_ready(&self, ready: bool) {
        self.0.lock().unwrap().ready = ready;
    }

    pub fn snapshot(&self) -> TestPipelineState {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait]
impl PipelineBase for TestPipeline {
    async fn add_connection(&self, id: String) -> Result<(), Error> {
        self.0.lock().unwrap().added.push(id);
        Ok(())
    }

    async fn remove_connection(&self, id: String) -> Result<(), Error> {
        self.0.lock().unwrap().removed.push(id);
        Ok(())
    }

    async fn init(&mut self, _args: &Args) -> Result<(), Error> {
        Ok(())
    }

    async fn run(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn ready(&self) -> Result<bool, Error> {
        Ok(self.0.lock().unwrap().ready)
    }

    async fn end(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn quit(&self) -> Result<(), Error> {
        self.0.lock().unwrap().quit_count += 1;
        Ok(())
    }

    async fn clean_up(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn print(&self) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{PipelineBase, TestPipeline};

    #[tokio::test]
    async fn test_pipeline_records_calls() {
        let pipeline = TestPipeline::default();
        assert!(!pipeline.ready().await.unwrap());

        pipeline.set_ready(true);
        assert!(pipeline.ready().await.unwrap());

        pipeline.add_connection("a".to_string()).await.unwrap();
        pipeline.remove_connection("a".to_string()).await.unwrap();
        pipeline.quit().await.unwrap();

        let snap = pipeline.snapshot();
        assert_eq!(vec!["a".to_string()], snap.added);
        assert_eq!(vec!["a".to_string()], snap.removed);
        assert_eq!(1, snap.quit_count);
    }
}
