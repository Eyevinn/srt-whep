mod coordinator;
mod errors;
mod messages;
pub mod watchdog;

pub use coordinator::{Coordinator, CoordinatorConfig};
pub use errors::SignalError;
pub use messages::{Command, ConnectionId, ConnectionInfo};

use crate::domain::SessionDescription;
use crate::stream::PipelineBase;
use tokio::sync::{mpsc, oneshot};

/// Clone-able handle to the coordinator actor. HTTP handlers and the
/// pipeline supervisor talk to the actor exclusively through this.
#[derive(Clone)]
pub struct SignalHandle {
    tx: mpsc::Sender<Command>,
}

/// Spawn the coordinator actor and return the handle for it.
pub fn spawn_coordinator<P: PipelineBase + 'static>(
    pipeline: P,
    config: CoordinatorConfig,
) -> SignalHandle {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(Coordinator::new(pipeline, config, rx).run());
    SignalHandle { tx }
}

impl SignalHandle {
    async fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<Result<T, SignalError>>) -> Command,
    ) -> Result<T, SignalError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(make(reply_tx))
            .await
            .map_err(|_| SignalError::Unavailable)?;
        reply_rx.await.map_err(|_| SignalError::Unavailable)?
    }

    pub async fn create_connection(&self, id: String) -> Result<SessionDescription, SignalError> {
        self.request(|reply| Command::CreateConnection { id, reply })
            .await
    }

    pub async fn offer_received(
        &self,
        id: String,
        sdp: SessionDescription,
    ) -> Result<SessionDescription, SignalError> {
        self.request(|reply| Command::OfferReceived { id, sdp, reply })
            .await
    }

    pub async fn answer_received(
        &self,
        id: String,
        sdp: SessionDescription,
    ) -> Result<(), SignalError> {
        self.request(|reply| Command::AnswerReceived { id, sdp, reply })
            .await
    }

    pub async fn remove_connection(&self, id: String) -> Result<(), SignalError> {
        self.request(|reply| Command::RemoveConnection { id, reply })
            .await
    }

    pub async fn list_connections(&self) -> Result<Vec<ConnectionInfo>, SignalError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Command::ListConnections { reply: reply_tx })
            .await
            .map_err(|_| SignalError::Unavailable)?;
        reply_rx.await.map_err(|_| SignalError::Unavailable)
    }

    pub async fn reset(&self) -> Result<(), SignalError> {
        let (reply_tx, reply_rx) = oneshot::channel();
        self.tx
            .send(Command::Reset { reply: reply_tx })
            .await
            .map_err(|_| SignalError::Unavailable)?;
        reply_rx.await.map_err(|_| SignalError::Unavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::{spawn_coordinator, CoordinatorConfig};
    use crate::domain::{SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
    use crate::stream::TestPipeline;

    #[tokio::test(start_paused = true)]
    async fn handle_drives_a_full_handshake() {
        let pipeline = TestPipeline::default();
        pipeline.set_ready(true);
        let handle = spawn_coordinator(pipeline.clone(), CoordinatorConfig::default());

        // The three legs run concurrently, exactly like the HTTP handlers do.
        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        tokio::task::yield_now().await; // connection registered

        let whip = {
            let handle = handle.clone();
            let offer = SessionDescription::parse(VALID_WHIP_OFFER.to_string()).unwrap();
            tokio::spawn(async move { handle.offer_received("a".to_string(), offer).await })
        };
        tokio::task::yield_now().await; // offer delivered

        let answer = SessionDescription::parse(VALID_WHEP_ANSWER.to_string()).unwrap();
        handle
            .answer_received("a".to_string(), answer)
            .await
            .unwrap();

        assert!(whep.await.unwrap().unwrap().is_sendonly());
        assert!(!whip.await.unwrap().unwrap().is_sendonly());
    }
}
