use super::errors::SignalError;
use super::messages::{Command, ConnectionId, ConnectionInfo, SdpReply, UnitReply};
use super::watchdog::Watchdog;
use crate::domain::SessionDescription;
use crate::stream::PipelineBase;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    pub offer_timeout: Duration,
    pub answer_timeout: Duration,
    pub watchdog_threshold: u32,
    pub sweep_interval: Duration,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            offer_timeout: Duration::from_secs(10),
            answer_timeout: Duration::from_secs(10),
            watchdog_threshold: 3,
            sweep_interval: Duration::from_secs(1),
        }
    }
}

enum ConnectionState {
    AwaitingOffer {
        whep_reply: SdpReply,
        deadline: Instant,
    },
    AwaitingAnswer {
        whip_reply: SdpReply,
        deadline: Instant,
    },
    // `since` is not read yet; a later task surfaces connection age.
    #[allow(dead_code)]
    Established { since: Instant },
}

impl ConnectionState {
    fn name(&self) -> &'static str {
        match self {
            ConnectionState::AwaitingOffer { .. } => "awaiting_offer",
            ConnectionState::AwaitingAnswer { .. } => "awaiting_answer",
            ConnectionState::Established { .. } => "established",
        }
    }
}

/// The signaling actor: sole owner of connection state and of pipeline
/// branch add/remove calls. Runs until every SignalHandle is dropped.
pub struct Coordinator<P: PipelineBase> {
    pipeline: P,
    config: CoordinatorConfig,
    connections: HashMap<ConnectionId, ConnectionState>,
    watchdog: Watchdog,
    rx: mpsc::Receiver<Command>,
}

impl<P: PipelineBase> Coordinator<P> {
    pub fn new(pipeline: P, config: CoordinatorConfig, rx: mpsc::Receiver<Command>) -> Self {
        let watchdog = Watchdog::new(config.watchdog_threshold);
        Self {
            pipeline,
            config,
            connections: HashMap::new(),
            watchdog,
            rx,
        }
    }

    pub async fn run(mut self) {
        let mut sweep = tokio::time::interval(self.config.sweep_interval);
        sweep.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                cmd = self.rx.recv() => match cmd {
                    Some(cmd) => self.handle(cmd).await,
                    None => break, // all handles dropped
                },
                _ = sweep.tick() => self.sweep_expired().await,
            }
        }
    }

    async fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::CreateConnection { id, reply } => self.create_connection(id, reply).await,
            Command::OfferReceived { id, sdp, reply } => self.offer_received(id, sdp, reply).await,
            Command::AnswerReceived { id, sdp, reply } => {
                self.answer_received(id, sdp, reply).await
            }
            Command::RemoveConnection { id, reply } => self.remove_connection(id, reply).await,
            Command::ListConnections { reply } => {
                let list = self
                    .connections
                    .iter()
                    .map(|(id, state)| ConnectionInfo {
                        id: id.clone(),
                        state: state.name().to_string(),
                    })
                    .collect();
                let _ = reply.send(list);
            }
            Command::Reset { reply } => {
                self.reset_all();
                let _ = reply.send(());
            }
        }
    }

    // Entry API can't be held across the pipeline awaits below.
    #[allow(clippy::map_entry)]
    async fn create_connection(&mut self, id: ConnectionId, reply: SdpReply) {
        if self.connections.contains_key(&id) {
            let _ = reply.send(Err(SignalError::WrongState(id)));
            return;
        }
        match self.pipeline.ready().await {
            Ok(true) => {}
            Ok(false) => {
                let _ = reply.send(Err(SignalError::NotReady));
                return;
            }
            Err(e) => {
                let _ = reply.send(Err(SignalError::Pipeline(e.to_string())));
                return;
            }
        }
        if let Err(e) = self.pipeline.add_connection(id.clone()).await {
            let _ = reply.send(Err(SignalError::Pipeline(e.to_string())));
            return;
        }
        let deadline = Instant::now() + self.config.offer_timeout;
        self.connections.insert(
            id,
            ConnectionState::AwaitingOffer {
                whep_reply: reply,
                deadline,
            },
        );
    }

    async fn offer_received(&mut self, id: ConnectionId, sdp: SessionDescription, reply: SdpReply) {
        match self.connections.remove(&id) {
            None => {
                let _ = reply.send(Err(SignalError::NotFound(id)));
            }
            Some(ConnectionState::AwaitingOffer { whep_reply, .. }) => {
                if whep_reply.send(Ok(sdp)).is_err() {
                    // The WHEP client vanished while waiting (actix dropped
                    // its handler future). Fail this handshake now.
                    tracing::warn!("WHEP waiter for {} is gone; failing handshake", id);
                    let _ = reply.send(Err(SignalError::NotFound(id.clone())));
                    self.fail_connection(id).await;
                    return;
                }
                let deadline = Instant::now() + self.config.answer_timeout;
                self.connections.insert(
                    id,
                    ConnectionState::AwaitingAnswer {
                        whip_reply: reply,
                        deadline,
                    },
                );
            }
            Some(other) => {
                // Wrong state: restore untouched, reject the command.
                self.connections.insert(id.clone(), other);
                let _ = reply.send(Err(SignalError::WrongState(id)));
            }
        }
    }

    async fn answer_received(
        &mut self,
        id: ConnectionId,
        sdp: SessionDescription,
        reply: UnitReply,
    ) {
        match self.connections.remove(&id) {
            None => {
                let _ = reply.send(Err(SignalError::NotFound(id)));
            }
            Some(ConnectionState::AwaitingAnswer { whip_reply, .. }) => {
                if whip_reply.send(Ok(sdp)).is_err() {
                    // The whipsink's HTTP request died; it can never receive
                    // the answer, so the handshake is failed.
                    tracing::warn!("WHIP waiter for {} is gone; failing handshake", id);
                    let _ = reply.send(Err(SignalError::NotFound(id.clone())));
                    self.fail_connection(id).await;
                    return;
                }
                self.watchdog.record_success();
                self.connections.insert(
                    id,
                    ConnectionState::Established {
                        since: Instant::now(),
                    },
                );
                let _ = reply.send(Ok(()));
            }
            Some(other) => {
                self.connections.insert(id.clone(), other);
                let _ = reply.send(Err(SignalError::WrongState(id)));
            }
        }
    }

    async fn remove_connection(&mut self, id: ConnectionId, reply: UnitReply) {
        match self.connections.remove(&id) {
            None => {
                let _ = reply.send(Err(SignalError::NotFound(id)));
            }
            Some(state) => {
                // Any pending waiter learns the connection is gone.
                match state {
                    ConnectionState::AwaitingOffer { whep_reply, .. } => {
                        let _ = whep_reply.send(Err(SignalError::NotFound(id.clone())));
                    }
                    ConnectionState::AwaitingAnswer { whip_reply, .. } => {
                        let _ = whip_reply.send(Err(SignalError::NotFound(id.clone())));
                    }
                    ConnectionState::Established { .. } => {}
                }
                let result = self
                    .pipeline
                    .remove_connection(id)
                    .await
                    .map_err(|e| SignalError::Pipeline(e.to_string()));
                let _ = reply.send(result);
            }
        }
    }

    async fn sweep_expired(&mut self) {
        let now = Instant::now();
        let expired: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter_map(|(id, state)| match state {
                ConnectionState::AwaitingOffer { deadline, .. }
                | ConnectionState::AwaitingAnswer { deadline, .. }
                    if *deadline <= now =>
                {
                    Some(id.clone())
                }
                _ => None,
            })
            .collect();

        for id in expired {
            tracing::warn!("Handshake for {} timed out", id);
            match self.connections.remove(&id) {
                Some(ConnectionState::AwaitingOffer { whep_reply, .. }) => {
                    let _ = whep_reply.send(Err(SignalError::Timeout("SDP offer")));
                }
                Some(ConnectionState::AwaitingAnswer { whip_reply, .. }) => {
                    let _ = whip_reply.send(Err(SignalError::Timeout("SDP answer")));
                }
                _ => continue,
            }
            self.fail_connection(id).await;
        }
    }

    /// Clean up a failed handshake: remove its pipeline branch, record the
    /// failure, and restart the pipeline when the watchdog trips.
    async fn fail_connection(&mut self, id: ConnectionId) {
        if let Err(e) = self.pipeline.remove_connection(id.clone()).await {
            tracing::error!("Failed to remove branch for {}: {}", id, e);
        }
        if self.watchdog.record_failure() {
            tracing::error!("Watchdog tripped: restarting the pipeline");
            self.reset_all();
            if let Err(e) = self.pipeline.quit().await {
                tracing::error!("Failed to quit pipeline: {}", e);
            }
        }
    }

    fn reset_all(&mut self) {
        for (_, state) in self.connections.drain() {
            match state {
                ConnectionState::AwaitingOffer { whep_reply, .. } => {
                    let _ = whep_reply.send(Err(SignalError::Unavailable));
                }
                ConnectionState::AwaitingAnswer { whip_reply, .. } => {
                    let _ = whip_reply.send(Err(SignalError::Unavailable));
                }
                ConnectionState::Established { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Coordinator, CoordinatorConfig};
    use crate::domain::{SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
    use crate::signal::messages::Command;
    // Unused by this task's happy-path test; a later task adds error-path
    // tests that reference `SignalError` directly.
    #[allow(unused_imports)]
    use crate::signal::SignalError;
    use crate::stream::TestPipeline;
    use std::time::Duration;
    use tokio::sync::{mpsc, oneshot};

    pub(super) fn offer() -> SessionDescription {
        SessionDescription::parse(VALID_WHIP_OFFER.to_string()).unwrap()
    }

    pub(super) fn answer() -> SessionDescription {
        SessionDescription::parse(VALID_WHEP_ANSWER.to_string()).unwrap()
    }

    pub(super) fn test_config() -> CoordinatorConfig {
        CoordinatorConfig {
            offer_timeout: Duration::from_secs(5),
            answer_timeout: Duration::from_secs(5),
            watchdog_threshold: 3,
            sweep_interval: Duration::from_secs(1),
        }
    }

    pub(super) fn ready_pipeline() -> TestPipeline {
        let pipeline = TestPipeline::default();
        pipeline.set_ready(true);
        pipeline
    }

    pub(super) fn spawn_actor(
        pipeline: TestPipeline,
        config: CoordinatorConfig,
    ) -> mpsc::Sender<Command> {
        let (tx, rx) = mpsc::channel(16);
        tokio::spawn(Coordinator::new(pipeline, config, rx).run());
        tx
    }

    #[tokio::test(start_paused = true)]
    async fn happy_path_create_offer_answer() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        // Browser connects.
        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;
        assert_eq!(vec!["a".to_string()], pipeline.snapshot().added);

        // Whipsink posts its offer; browser's waiter receives it.
        let (whip_tx, whip_rx) = oneshot::channel();
        tx.send(Command::OfferReceived {
            id: "a".into(),
            sdp: offer(),
            reply: whip_tx,
        })
        .await
        .unwrap();
        let delivered = whep_rx.await.unwrap().unwrap();
        assert!(delivered.is_sendonly());

        // Browser PATCHes the answer; whipsink's waiter receives it.
        let (patch_tx, patch_rx) = oneshot::channel();
        tx.send(Command::AnswerReceived {
            id: "a".into(),
            sdp: answer(),
            reply: patch_tx,
        })
        .await
        .unwrap();
        assert!(patch_rx.await.unwrap().is_ok());
        let delivered = whip_rx.await.unwrap().unwrap();
        assert!(!delivered.is_sendonly());

        // Nothing was torn down.
        let snap = pipeline.snapshot();
        assert!(snap.removed.is_empty());
        assert_eq!(0, snap.quit_count);
    }
}
