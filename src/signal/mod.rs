mod coordinator;
mod errors;
mod messages;
mod watchdog;

pub use coordinator::{Coordinator, CoordinatorArgs, CoordinatorConfig};
pub use errors::SignalError;
use messages::Command;
pub use messages::{ConnectionId, ConnectionInfo};

use crate::domain::{SdpAnswer, SdpOffer};
use crate::stream::{BranchControl, BranchId};
use tokio::sync::{mpsc, oneshot};

/// Clone-able handle to the coordinator actor. HTTP handlers and the
/// pipeline supervisor talk to the actor exclusively through this.
#[derive(Clone)]
pub struct SignalHandle {
    tx: mpsc::Sender<Command>,
}

/// Spawn the coordinator actor and return the handle for it. `branch_failures`
/// is the receiving end of the pipeline's bus-reap channel (its sender was
/// handed to the pipeline at construction), so the sink is present from birth
/// and no post-construction installer is needed. The watchdog trip sends a
/// `()` restart request to the supervisor over `restart_tx`.
pub fn spawn_coordinator<P: BranchControl + 'static>(
    pipeline: P,
    config: CoordinatorConfig,
    branch_failures: mpsc::Receiver<BranchId>,
    restart_tx: mpsc::Sender<()>,
) -> SignalHandle {
    let (tx, rx) = mpsc::channel(64);
    tokio::spawn(Coordinator::new(pipeline, config, rx, branch_failures, restart_tx).run());
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

    /// Register a new connection (WHEP POST). Sends `CreateConnection` and
    /// awaits the coordinator's reply, which resolves to the SDP offer once
    /// the whipsink delivers it, or an error on timeout/failure.
    pub async fn create_connection(&self, id: String) -> Result<SdpOffer, SignalError> {
        self.request(|reply| Command::CreateConnection { id, reply })
            .await
    }

    /// Hand the whipsink's SDP offer to the coordinator (loopback WHIP POST).
    /// Sends `OfferReceived` and awaits the reply, which resolves to the SDP
    /// answer once the browser PATCHes it, or an error on timeout/failure.
    pub async fn offer_received(
        &self,
        id: String,
        sdp: SdpOffer,
    ) -> Result<SdpAnswer, SignalError> {
        self.request(|reply| Command::OfferReceived { id, sdp, reply })
            .await
    }

    /// Hand the browser's SDP answer to the coordinator (WHEP PATCH). Sends
    /// `AnswerReceived`; the reply is `Ok(())` once the answer is accepted,
    /// or an error if the connection is unknown or the coordinator is gone.
    pub async fn answer_received(&self, id: String, sdp: SdpAnswer) -> Result<(), SignalError> {
        self.request(|reply| Command::AnswerReceived { id, sdp, reply })
            .await
    }

    /// Tear down a connection (WHEP/WHIP DELETE or internal cleanup). Sends
    /// `RemoveConnection`; the reply is `Ok(())` once it is removed, or an
    /// error if the coordinator is unavailable.
    pub async fn remove_connection(&self, id: String) -> Result<(), SignalError> {
        self.request(|reply| Command::RemoveConnection { id, reply })
            .await
    }

    /// List the current connections and their states (GET /list). Sends
    /// `ListConnections` and awaits the reply carrying the snapshot; errors
    /// only if the coordinator is unavailable.
    pub async fn list_connections(&self) -> Result<Vec<ConnectionInfo>, SignalError> {
        self.request(|reply| Command::ListConnections { reply })
            .await
    }

    /// Reset the coordinator after a pipeline restart (supervisor only).
    /// Sends `Reset`, which fails all in-flight waiters and clears the
    /// connection map; the reply is `Ok(())` once done, or an error if the
    /// coordinator is unavailable.
    pub async fn reset(&self) -> Result<(), SignalError> {
        self.request(|reply| Command::Reset { reply }).await
    }
}

#[cfg(test)]
mod tests {
    use super::{spawn_coordinator, CoordinatorConfig};
    use crate::domain::{SdpAnswer, SdpOffer, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
    use crate::stream::TestPipeline;
    use tokio::sync::mpsc;

    #[tokio::test(start_paused = true)]
    async fn handle_drives_a_full_handshake() {
        let pipeline = TestPipeline::default();
        pipeline.set_ready(true);
        // This test never reaps: a disconnected failure receiver.
        let (_fail_tx, fail_rx) = mpsc::channel(1);
        // This test never trips the watchdog: nothing consumes restarts.
        let (restart_tx, _restart_rx) = mpsc::channel::<()>(1);
        let handle = spawn_coordinator(
            pipeline.clone(),
            CoordinatorConfig::default(),
            fail_rx,
            restart_tx,
        );

        // The three legs run concurrently, exactly like the HTTP handlers do.
        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        tokio::task::yield_now().await; // connection registered

        let whip = {
            let handle = handle.clone();
            let offer = SdpOffer::parse(VALID_WHIP_OFFER.to_string()).unwrap();
            tokio::spawn(async move { handle.offer_received("a".to_string(), offer).await })
        };
        tokio::task::yield_now().await; // offer delivered

        let answer = SdpAnswer::parse(VALID_WHEP_ANSWER.to_string()).unwrap();
        handle
            .answer_received("a".to_string(), answer)
            .await
            .unwrap();

        assert!(whep.await.unwrap().unwrap().is_sendonly());
        assert!(!whip.await.unwrap().unwrap().is_sendonly());
    }
}
