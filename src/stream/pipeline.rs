use anyhow::{Error, Ok};
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
    #[clap(short, long, value_enum, verbatim_doc_comment)]
    pub srt_mode: SRTMode,

    /// Timeout for discoverer in seconds
    #[clap(short, long, default_value_t = 10)]
    pub discoverer_timeout_sec: u64,

    /// SRT output stream address(ip:port)
    #[clap(short, long)]
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

pub trait PipelineBase: Clone + Send + Sync {
    fn add_client(&self, id: String) -> Result<(), Error>;
    fn remove_connection(&self, id: String) -> Result<(), Error>;
    fn setup_pipeline(&self, args: &Args) -> Result<(), Error>;
    fn close_pipeline(&self) -> Result<(), Error>;
}

#[derive(Clone)]
pub struct DumpPipeline {}

impl DumpPipeline {
    pub fn new(_args: Args) -> Self {
        Self {}
    }
}

impl PipelineBase for DumpPipeline {
    fn add_client(&self, _id: String) -> Result<(), Error> {
        Ok(())
    }

    fn remove_connection(&self, _id: String) -> Result<(), Error> {
        Ok(())
    }

    fn setup_pipeline(&self, _args: &Args) -> Result<(), Error> {
        Ok(())
    }

    fn close_pipeline(&self) -> Result<(), Error> {
        Ok(())
    }
}
