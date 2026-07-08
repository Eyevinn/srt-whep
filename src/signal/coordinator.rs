use super::errors::SignalError;
use super::messages::{Command, ConnectionId, ConnectionInfo, SdpReply, UnitReply};
use super::watchdog::Watchdog;
use crate::domain::SessionDescription;
use crate::stream::BranchControl;
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    pub offer_timeout: Duration,
    pub answer_timeout: Duration,
    pub watchdog_threshold: u32,
    /// Only failures within this window of each other count toward a watchdog
    /// trip; older failures decay so unrelated ones over a long span never
    /// force a pipeline restart that would drop established viewers.
    pub watchdog_window: Duration,
    pub sweep_interval: Duration,
    /// Upper bound on a single branch teardown (`remove_branch`/`quit`). These
    /// run inline in the actor's select loop; bounding them keeps one wedged
    /// GStreamer teardown from stalling every other signaling command, the
    /// expiry sweep, and the watchdog.
    pub teardown_timeout: Duration,
}

impl Default for CoordinatorConfig {
    fn default() -> Self {
        Self {
            offer_timeout: Duration::from_secs(10),
            answer_timeout: Duration::from_secs(10),
            watchdog_threshold: 3,
            watchdog_window: Duration::from_secs(60),
            sweep_interval: Duration::from_secs(1),
            teardown_timeout: Duration::from_secs(5),
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

/// Outcome of delivering the whipsink's SDP offer to the parked WHEP waiter.
enum OfferDelivery {
    /// The WHEP client received the offer; advance to awaiting the answer.
    Delivered,
    /// The WHEP client had vanished; the handshake must be failed.
    WaiterGone,
}

impl ConnectionState {
    fn name(&self) -> &'static str {
        match self {
            ConnectionState::AwaitingOffer { .. } => "awaiting_offer",
            ConnectionState::AwaitingAnswer { .. } => "awaiting_answer",
            ConnectionState::Established { .. } => "established",
        }
    }

    /// Fail whichever reply waiter is parked on this connection, if any.
    /// Shared by the DELETE, reap, and reset paths: the connection is going
    /// away, so a client still awaiting a reply must learn it now. An
    /// `Established` connection has no parked waiter, so this is a no-op.
    fn fail_waiter(self, err: SignalError) {
        match self {
            ConnectionState::AwaitingOffer { whep_reply, .. } => {
                let _ = whep_reply.send(Err(err));
            }
            ConnectionState::AwaitingAnswer { whip_reply, .. } => {
                let _ = whip_reply.send(Err(err));
            }
            ConnectionState::Established { .. } => {}
        }
    }

    /// Deliver the whipsink's SDP offer to the parked WHEP waiter.
    /// `Ok(..)` means this was the legal `AwaitingOffer` state; the variant
    /// reports whether the waiter was still there. `Err(self)` means the offer
    /// arrived in the wrong state — the caller restores the connection unchanged.
    fn deliver_offer(self, sdp: SessionDescription) -> Result<OfferDelivery, ConnectionState> {
        match self {
            ConnectionState::AwaitingOffer { whep_reply, .. } => {
                if whep_reply.send(Ok(sdp)).is_err() {
                    Ok(OfferDelivery::WaiterGone)
                } else {
                    Ok(OfferDelivery::Delivered)
                }
            }
            other => Err(other),
        }
    }
}

/// The signaling actor: sole owner of connection state and of pipeline
/// branch add/remove calls. Runs until every SignalHandle is dropped.
pub struct Coordinator<P: BranchControl> {
    pipeline: P,
    config: CoordinatorConfig,
    connections: HashMap<ConnectionId, ConnectionState>,
    watchdog: Watchdog,
    rx: mpsc::Receiver<Command>,
    // Per-branch runtime failures observed on the pipeline bus. The
    // coordinator owns the receiver; the pipeline holds the matching sender
    // (installed below). This is a separate channel from `rx`, so it never
    // gates shutdown — the actor still stops when every SignalHandle drops.
    branch_failures: mpsc::Receiver<ConnectionId>,
}

impl<P: BranchControl> Coordinator<P> {
    pub fn new(pipeline: P, config: CoordinatorConfig, rx: mpsc::Receiver<Command>) -> Self {
        let watchdog = Watchdog::new(config.watchdog_threshold, config.watchdog_window);
        let (fail_tx, fail_rx) = mpsc::channel(64);
        pipeline.set_branch_failure_sink(fail_tx);
        Self {
            pipeline,
            config,
            connections: HashMap::new(),
            watchdog,
            rx,
            branch_failures: fail_rx,
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
                Some(id) = self.branch_failures.recv() => self.reap_branch(id).await,
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
                self.watchdog.record_success();
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
                let _ = reply.send(Err(e.into()));
                return;
            }
        }
        if let Err(add_err) = self.pipeline.add_branch(id.clone()).await {
            // Attach failed partway through. The id was never inserted, so
            // DELETE could never reach it — detach the half-attached branch
            // now (detach tolerates a half-built branch) so we don't orphan a
            // whipclientsink with no cleanup path.
            if let Err(cleanup_err) = self.remove_branch_bounded(id.clone()).await {
                tracing::error!(
                    "Failed to detach half-attached branch for {}: {}",
                    id,
                    cleanup_err
                );
            }
            let _ = reply.send(Err(add_err.into()));
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
        let Some(state) = self.connections.remove(&id) else {
            let _ = reply.send(Err(SignalError::NotFound(id)));
            return;
        };
        match state.deliver_offer(sdp) {
            Ok(OfferDelivery::Delivered) => {
                let deadline = Instant::now() + self.config.answer_timeout;
                self.connections.insert(
                    id,
                    ConnectionState::AwaitingAnswer {
                        whip_reply: reply,
                        deadline,
                    },
                );
            }
            Ok(OfferDelivery::WaiterGone) => {
                // The WHEP client vanished while waiting (actix dropped its
                // handler future). Fail this handshake now.
                tracing::warn!("WHEP waiter for {} is gone; failing handshake", id);
                let _ = reply.send(Err(SignalError::NotFound(id.clone())));
                self.fail_connection(id).await;
            }
            Err(other) => {
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
        let Some(state) = self.connections.remove(&id) else {
            let _ = reply.send(Err(SignalError::NotFound(id)));
            return;
        };
        // Remove the branch FIRST; only drop the map entry once teardown
        // succeeds. On failure we re-insert the connection so a retried DELETE
        // can try again — dropping it first would return 404 on retry while
        // leaking the branch.
        match self.remove_branch_bounded(id.clone()).await {
            Ok(()) => {
                // The connection is really gone now; let any pending waiter learn it.
                state.fail_waiter(SignalError::NotFound(id.clone()));
                let _ = reply.send(Ok(()));
            }
            Err(e) => {
                self.connections.insert(id, state);
                let _ = reply.send(Err(e));
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
        if let Err(e) = self.remove_branch_bounded(id.clone()).await {
            tracing::error!("Failed to remove branch for {}: {}", id, e);
        }
        if self.watchdog.record_failure() {
            tracing::error!("Watchdog tripped: restarting the pipeline");
            self.reset_all();
            self.quit_bounded().await;
        }
    }

    /// A per-viewer branch failed at runtime (its whipsink errored, its peer
    /// went away), reported by the pipeline's bus watch. Drop the connection
    /// and detach its branch so it can't linger as a ghost `/list` entry with
    /// orphaned elements. A dead peer is not a pipeline-health signal, so the
    /// watchdog is deliberately left untouched.
    async fn reap_branch(&mut self, id: ConnectionId) {
        let Some(state) = self.connections.remove(&id) else {
            return; // already gone: raced a DELETE or an expiry sweep
        };
        tracing::warn!("Reaping branch for {} after a runtime failure", id);
        state.fail_waiter(SignalError::NotFound(id.clone()));
        if let Err(e) = self.remove_branch_bounded(id.clone()).await {
            tracing::error!("Failed to remove branch for {}: {}", id, e);
        }
    }

    /// Remove a branch, bounded by `teardown_timeout`. All teardown awaits run
    /// inline in the actor's select loop; without a bound, one wedged
    /// GStreamer teardown would stall every signaling command, the expiry
    /// sweep, and the watchdog. A timeout surfaces as a retryable error.
    async fn remove_branch_bounded(&self, id: ConnectionId) -> Result<(), SignalError> {
        match tokio::time::timeout(
            self.config.teardown_timeout,
            self.pipeline.remove_branch(id.clone()),
        )
        .await
        {
            Ok(res) => res.map_err(SignalError::from),
            Err(_) => {
                tracing::error!(
                    "Branch teardown for {} exceeded {:?}",
                    id,
                    self.config.teardown_timeout
                );
                Err(SignalError::PipelineBusy(
                    "branch teardown timed out".into(),
                ))
            }
        }
    }

    /// Force-restart the pipeline, bounded so a wedged quit can't stall the actor.
    async fn quit_bounded(&self) {
        match tokio::time::timeout(self.config.teardown_timeout, self.pipeline.quit()).await {
            Ok(Ok(())) => {}
            Ok(Err(e)) => tracing::error!("Failed to quit pipeline: {}", e),
            Err(_) => tracing::error!("Pipeline quit exceeded {:?}", self.config.teardown_timeout),
        }
    }

    fn reset_all(&mut self) {
        for (_, state) in self.connections.drain() {
            state.fail_waiter(SignalError::Unavailable);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Coordinator, CoordinatorConfig};
    use crate::domain::{SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
    use crate::signal::messages::Command;
    use crate::signal::SignalError;
    use crate::stream::TestPipeline;
    use std::time::Duration;
    use tokio::sync::{mpsc, oneshot};

    use super::ConnectionState;
    use tokio::time::Instant;

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
            watchdog_window: Duration::from_secs(60),
            sweep_interval: Duration::from_secs(1),
            teardown_timeout: Duration::from_secs(5),
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

    /// Drive a full WHEP<->WHIP handshake so the connection reaches Established.
    async fn establish(tx: &mpsc::Sender<Command>, id: &str) {
        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: id.into(),
            reply: whep_tx,
        })
        .await
        .unwrap();
        let (whip_tx, whip_rx) = oneshot::channel();
        tx.send(Command::OfferReceived {
            id: id.into(),
            sdp: offer(),
            reply: whip_tx,
        })
        .await
        .unwrap();
        whep_rx.await.unwrap().unwrap();
        let (patch_tx, patch_rx) = oneshot::channel();
        tx.send(Command::AnswerReceived {
            id: id.into(),
            sdp: answer(),
            reply: patch_tx,
        })
        .await
        .unwrap();
        patch_rx.await.unwrap().unwrap();
        whip_rx.await.unwrap().unwrap();
    }

    async fn list_ids(tx: &mpsc::Sender<Command>) -> Vec<String> {
        let (list_tx, list_rx) = oneshot::channel();
        tx.send(Command::ListConnections { reply: list_tx })
            .await
            .unwrap();
        list_rx.await.unwrap().into_iter().map(|c| c.id).collect()
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

    #[tokio::test(start_paused = true)]
    async fn offer_timeout_fails_only_that_connection() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();

        // Awaiting the reply parks every task on timers; the paused clock
        // auto-advances through sweep ticks until the deadline fires.
        let result = whep_rx.await.unwrap();
        assert!(matches!(result, Err(SignalError::Timeout("SDP offer"))));
        tokio::task::yield_now().await; // let the actor finish branch cleanup

        let snap = pipeline.snapshot();
        assert_eq!(vec!["a".to_string()], snap.removed);
        assert_eq!(0, snap.quit_count); // one failure: watchdog not tripped
    }

    #[tokio::test(start_paused = true)]
    async fn answer_timeout_fails_the_whip_waiter() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();

        let (whip_tx, whip_rx) = oneshot::channel();
        tx.send(Command::OfferReceived {
            id: "a".into(),
            sdp: offer(),
            reply: whip_tx,
        })
        .await
        .unwrap();
        assert!(whep_rx.await.unwrap().is_ok()); // offer delivered

        // No PATCH arrives; the answer deadline fires.
        let result = whip_rx.await.unwrap();
        assert!(matches!(result, Err(SignalError::Timeout("SDP answer"))));
        tokio::task::yield_now().await; // let the actor finish branch cleanup
        assert_eq!(vec!["a".to_string()], pipeline.snapshot().removed);
    }

    #[tokio::test(start_paused = true)]
    async fn abandoned_whep_client_is_reaped_by_the_sweep() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();
        tokio::task::yield_now().await; // let the actor register the connection
        drop(whep_rx); // browser disconnected; actix dropped the handler future

        tokio::time::advance(Duration::from_secs(6)).await; // past offer_timeout (5s)
        tokio::task::yield_now().await; // let the sweep run

        let snap = pipeline.snapshot();
        assert_eq!(vec!["a".to_string()], snap.added);
        assert_eq!(vec!["a".to_string()], snap.removed);
    }

    #[tokio::test(start_paused = true)]
    async fn transient_pipeline_failure_stays_retryable() {
        use crate::stream::PipelineError;
        use actix_web::ResponseError;

        let pipeline = ready_pipeline();
        pipeline.fail_next_add_branch(PipelineError::Transient("state lock timed out".into()));
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();

        let err = whep_rx.await.unwrap().unwrap_err();
        // Retryable at the seam stays retryable on the wire: 503 + Retry-After.
        assert_eq!(503, err.status_code().as_u16());
        assert!(err.error_response().headers().get("Retry-After").is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn not_ready_pipeline_rejects_creation() {
        let pipeline = TestPipeline::default(); // ready = false
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();

        assert!(matches!(whep_rx.await.unwrap(), Err(SignalError::NotReady)));
        assert!(pipeline.snapshot().added.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_id_is_not_found_for_every_command() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whip_tx, whip_rx) = oneshot::channel();
        tx.send(Command::OfferReceived {
            id: "ghost".into(),
            sdp: offer(),
            reply: whip_tx,
        })
        .await
        .unwrap();
        assert!(matches!(
            whip_rx.await.unwrap(),
            Err(SignalError::NotFound(_))
        ));

        let (patch_tx, patch_rx) = oneshot::channel();
        tx.send(Command::AnswerReceived {
            id: "ghost".into(),
            sdp: answer(),
            reply: patch_tx,
        })
        .await
        .unwrap();
        assert!(matches!(
            patch_rx.await.unwrap(),
            Err(SignalError::NotFound(_))
        ));

        let (rm_tx, rm_rx) = oneshot::channel();
        tx.send(Command::RemoveConnection {
            id: "ghost".into(),
            reply: rm_tx,
        })
        .await
        .unwrap();
        assert!(matches!(
            rm_rx.await.unwrap(),
            Err(SignalError::NotFound(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn wrong_state_commands_are_rejected_without_corruption() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        // Duplicate create.
        let (t1, r1) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: t1,
        })
        .await
        .unwrap();
        let (t2, r2) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: t2,
        })
        .await
        .unwrap();
        assert!(matches!(r2.await.unwrap(), Err(SignalError::WrongState(_))));

        // PATCH before the offer exists is a wrong-state command.
        let (t3, r3) = oneshot::channel();
        tx.send(Command::AnswerReceived {
            id: "a".into(),
            sdp: answer(),
            reply: t3,
        })
        .await
        .unwrap();
        assert!(matches!(r3.await.unwrap(), Err(SignalError::WrongState(_))));

        // The original handshake still works after both rejections.
        let (t4, r4) = oneshot::channel();
        tx.send(Command::OfferReceived {
            id: "a".into(),
            sdp: offer(),
            reply: t4,
        })
        .await
        .unwrap();
        assert!(r1.await.unwrap().is_ok());

        let (t5, r5) = oneshot::channel();
        tx.send(Command::AnswerReceived {
            id: "a".into(),
            sdp: answer(),
            reply: t5,
        })
        .await
        .unwrap();
        assert!(r5.await.unwrap().is_ok());
        assert!(r4.await.unwrap().is_ok());
        assert_eq!(1, pipeline.snapshot().added.len()); // no duplicate branch
    }

    #[tokio::test(start_paused = true)]
    async fn watchdog_trips_after_three_consecutive_failures() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config()); // threshold 3

        for i in 0..3 {
            let (whep_tx, whep_rx) = oneshot::channel();
            tx.send(Command::CreateConnection {
                id: format!("conn-{}", i),
                reply: whep_tx,
            })
            .await
            .unwrap();
            // Each handshake times out via auto-advance.
            assert!(matches!(
                whep_rx.await.unwrap(),
                Err(SignalError::Timeout(_))
            ));
        }

        tokio::task::yield_now().await; // let the actor finish the trip handling
        assert_eq!(1, pipeline.snapshot().quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn success_between_failures_prevents_the_trip() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        // Two failures.
        for i in 0..2 {
            let (whep_tx, whep_rx) = oneshot::channel();
            tx.send(Command::CreateConnection {
                id: format!("fail-{}", i),
                reply: whep_tx,
            })
            .await
            .unwrap();
            let _ = whep_rx.await.unwrap();
        }

        // One success resets the counter.
        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "ok".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();
        let (whip_tx, whip_rx) = oneshot::channel();
        tx.send(Command::OfferReceived {
            id: "ok".into(),
            sdp: offer(),
            reply: whip_tx,
        })
        .await
        .unwrap();
        assert!(whep_rx.await.unwrap().is_ok());
        let (patch_tx, patch_rx) = oneshot::channel();
        tx.send(Command::AnswerReceived {
            id: "ok".into(),
            sdp: answer(),
            reply: patch_tx,
        })
        .await
        .unwrap();
        assert!(patch_rx.await.unwrap().is_ok());
        assert!(whip_rx.await.unwrap().is_ok());

        // Two more failures: still below threshold thanks to the reset.
        for i in 2..4 {
            let (whep_tx, whep_rx) = oneshot::channel();
            tx.send(Command::CreateConnection {
                id: format!("fail-{}", i),
                reply: whep_tx,
            })
            .await
            .unwrap();
            let _ = whep_rx.await.unwrap();
        }

        tokio::task::yield_now().await;
        assert_eq!(0, pipeline.snapshot().quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn reset_clears_the_watchdog_counter() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config()); // threshold 3

        // Two consecutive timeout failures.
        for i in 0..2 {
            let (whep_tx, whep_rx) = oneshot::channel();
            tx.send(Command::CreateConnection {
                id: format!("fail-{}", i),
                reply: whep_tx,
            })
            .await
            .unwrap();
            let _ = whep_rx.await.unwrap();
        }

        // Pipeline restarted: supervisor sends Reset.
        let (reset_tx, reset_rx) = oneshot::channel();
        tx.send(Command::Reset { reply: reset_tx }).await.unwrap();
        reset_rx.await.unwrap();

        // Two more failures on the fresh pipeline: still below threshold.
        for i in 2..4 {
            let (whep_tx, whep_rx) = oneshot::channel();
            tx.send(Command::CreateConnection {
                id: format!("fail-{}", i),
                reply: whep_tx,
            })
            .await
            .unwrap();
            let _ = whep_rx.await.unwrap();
        }

        tokio::task::yield_now().await;
        assert_eq!(0, pipeline.snapshot().quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn reset_fails_all_waiters_and_clears_state() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();

        let (reset_tx, reset_rx) = oneshot::channel();
        tx.send(Command::Reset { reply: reset_tx }).await.unwrap();
        reset_rx.await.unwrap();

        assert!(matches!(
            whep_rx.await.unwrap(),
            Err(SignalError::Unavailable)
        ));

        let (list_tx, list_rx) = oneshot::channel();
        tx.send(Command::ListConnections { reply: list_tx })
            .await
            .unwrap();
        assert!(list_rx.await.unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn list_reports_ids_and_state_names() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();
        tokio::task::yield_now().await;

        let (list_tx, list_rx) = oneshot::channel();
        tx.send(Command::ListConnections { reply: list_tx })
            .await
            .unwrap();
        let list = list_rx.await.unwrap();
        assert_eq!(1, list.len());
        assert_eq!("a", list[0].id);
        assert_eq!("awaiting_offer", list[0].state);

        drop(whep_rx); // silence unused warning; connection will be swept later
    }

    #[tokio::test(start_paused = true)]
    async fn add_branch_failure_detaches_the_half_attached_branch() {
        use crate::stream::PipelineError;

        let pipeline = ready_pipeline();
        pipeline.fail_next_add_branch(PipelineError::Fatal("attach blew up".into()));
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();

        // The create fails and the id is never registered...
        assert!(whep_rx.await.unwrap().is_err());
        tokio::task::yield_now().await;

        // ...but the half-attached branch is detached, not orphaned.
        assert_eq!(vec!["a".to_string()], pipeline.snapshot().removed);
        assert!(list_ids(&tx).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn failed_delete_keeps_the_connection_retryable() {
        use crate::stream::PipelineError;

        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());
        establish(&tx, "a").await;

        // First DELETE: the branch teardown fails transiently.
        pipeline.fail_next_remove_branch(PipelineError::Transient("lock timed out".into()));
        let (d1_tx, d1_rx) = oneshot::channel();
        tx.send(Command::RemoveConnection {
            id: "a".into(),
            reply: d1_tx,
        })
        .await
        .unwrap();
        assert!(matches!(
            d1_rx.await.unwrap(),
            Err(SignalError::PipelineBusy(_))
        ));

        // The connection is kept, so a retried DELETE still finds it (no 404).
        assert_eq!(vec!["a".to_string()], list_ids(&tx).await);

        // Retried DELETE succeeds, removes the branch, and clears the entry.
        let (d2_tx, d2_rx) = oneshot::channel();
        tx.send(Command::RemoveConnection {
            id: "a".into(),
            reply: d2_tx,
        })
        .await
        .unwrap();
        assert!(d2_rx.await.unwrap().is_ok());
        assert!(pipeline.snapshot().removed.contains(&"a".to_string()));
        assert!(list_ids(&tx).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn established_connection_is_reaped_on_branch_failure() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config());
        establish(&tx, "a").await;

        // The pipeline's bus watch reports the branch errored at runtime.
        pipeline.fail_branch("a");
        for _ in 0..5 {
            tokio::task::yield_now().await; // let the actor drain the reap channel
        }

        // The established connection is reaped: branch detached, entry gone.
        assert!(pipeline.snapshot().removed.contains(&"a".to_string()));
        assert!(list_ids(&tx).await.is_empty());
        // A dead peer is not a pipeline-health failure: no restart.
        assert_eq!(0, pipeline.snapshot().quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn wedged_teardown_does_not_stall_the_actor() {
        let pipeline = ready_pipeline();
        let tx = spawn_actor(pipeline.clone(), test_config()); // teardown_timeout 5s
        establish(&tx, "a").await;

        // Its teardown wedges and never completes.
        pipeline.block_remove_branch();
        let (d_tx, d_rx) = oneshot::channel();
        tx.send(Command::RemoveConnection {
            id: "a".into(),
            reply: d_tx,
        })
        .await
        .unwrap();

        // The teardown timeout fires (auto-advanced paused clock) and the actor
        // recovers with a retryable error instead of hanging forever.
        assert!(matches!(
            d_rx.await.unwrap(),
            Err(SignalError::PipelineBusy(_))
        ));

        // The actor is still responsive, and the connection stayed retryable.
        assert_eq!(vec!["a".to_string()], list_ids(&tx).await);
    }

    #[tokio::test]
    async fn fail_waiter_notifies_the_awaiting_offer_waiter() {
        let (tx, rx) = oneshot::channel();
        let state = ConnectionState::AwaitingOffer {
            whep_reply: tx,
            deadline: Instant::now(),
        };
        state.fail_waiter(SignalError::Unavailable);
        assert!(matches!(rx.await.unwrap(), Err(SignalError::Unavailable)));
    }

    #[tokio::test]
    async fn fail_waiter_notifies_the_awaiting_answer_waiter() {
        let (tx, rx) = oneshot::channel();
        let state = ConnectionState::AwaitingAnswer {
            whip_reply: tx,
            deadline: Instant::now(),
        };
        state.fail_waiter(SignalError::NotFound("a".into()));
        assert!(matches!(rx.await.unwrap(), Err(SignalError::NotFound(_))));
    }
}
