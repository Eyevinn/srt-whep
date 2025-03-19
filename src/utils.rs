use crate::domain::SharableAppState;
use crate::stream::{Args, PipelineBase, SharablePipeline};
use tokio_async_drop::tokio_async_drop;

// Run the pipe and clean up when it finishes
pub struct PipelineGuard {
    pipeline: SharablePipeline,
    args: Args,
    state: SharableAppState,
}

impl PipelineGuard {
    pub fn new(pipeline: SharablePipeline, args: Args, state: SharableAppState) -> Self {
        Self {
            pipeline,
            args,
            state,
        }
    }

    /// Run a pipeline until it encounters EOS or an error. Clean up the pipeline after it finishes.
    pub async fn run(&mut self) -> Result<(), anyhow::Error> {
        self.pipeline.init(&self.args).await?;

        // Block until EOS or error message pops up
        self.pipeline.run().await?;

        Ok(())
    }

    /// Clean up a pipeline on Drop.
    async fn cleanup(&self) -> Result<(), anyhow::Error> {
        // Clean up the pipeline when it finishes so it can be rerun
        self.pipeline.clean_up().await?;

        // Reset app state
        self.state.reset().await?;

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
