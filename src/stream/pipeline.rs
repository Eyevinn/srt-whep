use anyhow::{Error, Ok};
use async_trait::async_trait;
use clap::{Parser, ValueEnum};

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

#[derive(Clone)]
pub struct DumpPipeline {}

impl DumpPipeline {
    pub fn new(_args: Args) -> Self {
        Self {}
    }
}

#[async_trait]
impl PipelineBase for DumpPipeline {
    async fn add_connection(&self, _id: String) -> Result<(), Error> {
        Ok(())
    }

    async fn remove_connection(&self, _id: String) -> Result<(), Error> {
        Ok(())
    }

    async fn init(&mut self, _args: &Args) -> Result<(), Error> {
        Ok(())
    }

    async fn run(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn ready(&self) -> Result<bool, Error> {
        Ok(true)
    }

    async fn end(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn quit(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn clean_up(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn print(&self) -> Result<(), Error> {
        Ok(())
    }
}
