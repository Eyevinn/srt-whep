use crate::signal::SignalHandle;
use crate::stream::{PipelineLifecycle, SharablePipeline};
use tokio_async_drop::tokio_async_drop;

// Run the pipe and clean up when it finishes
pub struct PipelineGuard {
    pipeline: SharablePipeline,
    signal: SignalHandle,
}

impl PipelineGuard {
    pub fn new(pipeline: SharablePipeline, signal: SignalHandle) -> Self {
        Self { pipeline, signal }
    }

    /// Run a pipeline until it encounters EOS or an error. Clean up the pipeline after it finishes.
    pub async fn run(&self) -> Result<(), anyhow::Error> {
        self.pipeline.init().await?;

        // Block until EOS or error message pops up
        self.pipeline.run().await?;

        Ok(())
    }

    /// Clean up a pipeline on Drop.
    async fn cleanup(&self) -> Result<(), anyhow::Error> {
        // Clean up the pipeline when it finishes so it can be rerun
        self.pipeline.clean_up().await?;

        // Fail all in-flight handshakes and drop signaling state.
        self.signal.reset().await?;

        Ok(())
    }
}

impl Drop for PipelineGuard {
    fn drop(&mut self) {
        tokio_async_drop!({
            if (self.cleanup().await).is_ok() {
                tracing::info!("Successfully clean up pipeline and reset state.");
            } else {
                tracing::error!("Failed to clean up pipeline and reset state.");
            }
        });
    }
}
