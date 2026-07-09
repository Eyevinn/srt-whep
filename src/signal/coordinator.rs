use super::errors::SignalError;
use super::messages::{AnswerReply, Command, ConnectionId, ConnectionInfo, OfferReply, UnitReply};
use super::watchdog::Watchdog;
use crate::domain::{SdpAnswer, SdpOffer};
use crate::stream::{BranchControl, BranchId};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::Instant;

/// Single source of truth for the coordinator's default timing/watchdog knobs.
/// Both `Default for CoordinatorConfig` and the `CoordinatorArgs` clap
/// `default_value_t` attributes read from these, so "run with no flags is
/// today's behavior" holds by construction — no drift-guard test needed. The
/// unit is in each name because the CLI takes secs/ms while `CoordinatorConfig`
/// holds `Duration`.
const DEFAULT_OFFER_TIMEOUT_SEC: u64 = 10;
const DEFAULT_ANSWER_TIMEOUT_SEC: u64 = 10;
const DEFAULT_WATCHDOG_THRESHOLD: u32 = 3;
const DEFAULT_WATCHDOG_WINDOW_SEC: u64 = 60;
const DEFAULT_SWEEP_INTERVAL_MS: u64 = 1000;
const DEFAULT_TEARDOWN_TIMEOUT_SEC: u64 = 5;

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
            offer_timeout: Duration::from_secs(DEFAULT_OFFER_TIMEOUT_SEC),
            answer_timeout: Duration::from_secs(DEFAULT_ANSWER_TIMEOUT_SEC),
            watchdog_threshold: DEFAULT_WATCHDOG_THRESHOLD,
            watchdog_window: Duration::from_secs(DEFAULT_WATCHDOG_WINDOW_SEC),
            sweep_interval: Duration::from_millis(DEFAULT_SWEEP_INTERVAL_MS),
            teardown_timeout: Duration::from_secs(DEFAULT_TEARDOWN_TIMEOUT_SEC),
        }
    }
}

/// CLI surface for the coordinator's timing/watchdog knobs. Kept separate
/// from `stream::Args` so `stream` never depends on `signal`; the crate-root
/// binary flattens both into one parser.
#[derive(clap::Args, Debug, Clone)]
pub struct CoordinatorArgs {
    /// Seconds a WHEP client waits for the whipsink's SDP offer.
    #[clap(long, default_value_t = DEFAULT_OFFER_TIMEOUT_SEC)]
    pub offer_timeout_sec: u64,
    /// Seconds the whipsink waits for the browser's SDP answer.
    #[clap(long, default_value_t = DEFAULT_ANSWER_TIMEOUT_SEC)]
    pub answer_timeout_sec: u64,
    /// Consecutive handshake failures (within the window) that trip a restart.
    #[clap(long, default_value_t = DEFAULT_WATCHDOG_THRESHOLD)]
    pub watchdog_threshold: u32,
    /// Seconds over which failures decay for the watchdog.
    #[clap(long, default_value_t = DEFAULT_WATCHDOG_WINDOW_SEC)]
    pub watchdog_window_sec: u64,
    /// Expiry-sweep interval in milliseconds.
    #[clap(long, default_value_t = DEFAULT_SWEEP_INTERVAL_MS)]
    pub sweep_interval_ms: u64,
    /// Upper bound, in seconds, on a single branch teardown/quit.
    #[clap(long, default_value_t = DEFAULT_TEARDOWN_TIMEOUT_SEC)]
    pub teardown_timeout_sec: u64,
}

impl CoordinatorArgs {
    pub fn to_config(&self) -> CoordinatorConfig {
        CoordinatorConfig {
            offer_timeout: Duration::from_secs(self.offer_timeout_sec),
            answer_timeout: Duration::from_secs(self.answer_timeout_sec),
            watchdog_threshold: self.watchdog_threshold,
            watchdog_window: Duration::from_secs(self.watchdog_window_sec),
            sweep_interval: Duration::from_millis(self.sweep_interval_ms),
            teardown_timeout: Duration::from_secs(self.teardown_timeout_sec),
        }
    }
}

enum ConnectionState {
    AwaitingOffer {
        whep_reply: OfferReply,
        deadline: Instant,
    },
    AwaitingAnswer {
        whip_reply: AnswerReply,
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

/// Outcome of delivering the browser's SDP answer to the parked WHIP waiter.
enum AnswerDelivery {
    /// The whipsink received the answer; the connection is established.
    Established,
    /// The whipsink's request had died; the handshake must be failed.
    WaiterGone,
}

impl ConnectionState {
    fn awaiting_offer(whep_reply: OfferReply, deadline: Instant) -> Self {
        ConnectionState::AwaitingOffer {
            whep_reply,
            deadline,
        }
    }

    fn awaiting_answer(whip_reply: AnswerReply, deadline: Instant) -> Self {
        ConnectionState::AwaitingAnswer {
            whip_reply,
            deadline,
        }
    }

    fn established(since: Instant) -> Self {
        ConnectionState::Established { since }
    }

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
    fn deliver_offer(self, sdp: SdpOffer) -> Result<OfferDelivery, ConnectionState> {
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

    /// Deliver the browser's SDP answer to the parked WHIP waiter.
    /// `Ok(..)` means this was the legal `AwaitingAnswer` state; the variant
    /// reports whether the waiter was still there. `Err(self)` returns the
    /// unchanged state for the caller to restore and reject.
    fn deliver_answer(self, sdp: SdpAnswer) -> Result<AnswerDelivery, ConnectionState> {
        match self {
            ConnectionState::AwaitingAnswer { whip_reply, .. } => {
                if whip_reply.send(Ok(sdp)).is_err() {
                    Ok(AnswerDelivery::WaiterGone)
                } else {
                    Ok(AnswerDelivery::Established)
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
    // (from its own construction). This is a separate channel from `rx`, so it
    // never gates shutdown — the actor still stops when every SignalHandle drops.
    branch_failures: mpsc::Receiver<BranchId>,
}

impl<P: BranchControl> Coordinator<P> {
    pub fn new(
        pipeline: P,
        config: CoordinatorConfig,
        rx: mpsc::Receiver<Command>,
        branch_failures: mpsc::Receiver<BranchId>,
    ) -> Self {
        let watchdog = Watchdog::new(config.watchdog_threshold, config.watchdog_window);
        Self {
            pipeline,
            config,
            connections: HashMap::new(),
            watchdog,
            rx,
            branch_failures,
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
                // Map the stream plane's `BranchId` into the signal plane's
                // `ConnectionId` at this seam (they are the same value; the
                // newtype keeps the two module vocabularies from leaking).
                Some(branch) = self.branch_failures.recv() => self.reap_branch(branch.into_string()).await,
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
    async fn create_connection(&mut self, id: ConnectionId, reply: OfferReply) {
        if self.connections.contains_key(&id) {
            let _ = reply.send(Err(SignalError::WrongState(id)));
            return;
        }
        // Bound add_branch on the actor's critical path. Its failure path now
        // detaches a half-built branch internally (ADR 0002); an unbounded
        // detach would let one wedged GStreamer teardown stall every command,
        // the sweep, and the watchdog -- the same bound remove_branch_bounded
        // gives the other teardown paths. The synchronous attach has no await
        // to cancel; this bounds the internal cleanup detach.
        match tokio::time::timeout(
            self.config.teardown_timeout,
            self.pipeline.add_branch(id.clone()),
        )
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(add_err)) => {
                // Error variants mean retry policy only -- no matching here.
                let _ = reply.send(Err(add_err.into()));
                return;
            }
            Err(_) => {
                tracing::error!(
                    "add_branch for {} exceeded {:?}",
                    id,
                    self.config.teardown_timeout
                );
                let _ = reply.send(Err(SignalError::PipelineBusy(
                    "branch attach/cleanup timed out".into(),
                )));
                return;
            }
        }
        let deadline = Instant::now() + self.config.offer_timeout;
        self.connections
            .insert(id, ConnectionState::awaiting_offer(reply, deadline));
    }

    async fn offer_received(&mut self, id: ConnectionId, sdp: SdpOffer, reply: AnswerReply) {
        let Some(state) = self.connections.remove(&id) else {
            let _ = reply.send(Err(SignalError::NotFound(id)));
            return;
        };
        match state.deliver_offer(sdp) {
            Ok(OfferDelivery::Delivered) => {
                let deadline = Instant::now() + self.config.answer_timeout;
                self.connections
                    .insert(id, ConnectionState::awaiting_answer(reply, deadline));
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

    async fn answer_received(&mut self, id: ConnectionId, sdp: SdpAnswer, reply: UnitReply) {
        let Some(state) = self.connections.remove(&id) else {
            let _ = reply.send(Err(SignalError::NotFound(id)));
            return;
        };
        match state.deliver_answer(sdp) {
            Ok(AnswerDelivery::Established) => {
                self.watchdog.record_success();
                self.connections
                    .insert(id, ConnectionState::established(Instant::now()));
                let _ = reply.send(Ok(()));
            }
            Ok(AnswerDelivery::WaiterGone) => {
                // The whipsink's HTTP request died; it can never receive the
                // answer, so the handshake is failed.
                tracing::warn!("WHIP waiter for {} is gone; failing handshake", id);
                let _ = reply.send(Err(SignalError::NotFound(id.clone())));
                self.fail_connection(id).await;
            }
            Err(other) => {
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
    use super::ConnectionState;
    use super::CoordinatorConfig;
    use crate::domain::{SdpAnswer, SdpOffer, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
    use crate::signal::{spawn_coordinator, SignalError, SignalHandle};
    use crate::stream::{BranchId, TestPipeline};
    use std::time::Duration;
    use tokio::sync::{mpsc, oneshot};
    use tokio::time::Instant;

    pub(super) fn offer() -> SdpOffer {
        SdpOffer::parse(VALID_WHIP_OFFER.to_string()).unwrap()
    }

    pub(super) fn answer() -> SdpAnswer {
        SdpAnswer::parse(VALID_WHEP_ANSWER.to_string()).unwrap()
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

    /// Spawn the coordinator and return its `SignalHandle` -- the exact facade
    /// the HTTP routes use. No reaper wired: a disconnected failure receiver
    /// (its sender dropped) so `branch_failures.recv()` yields `None` and that
    /// select arm stays idle. Tests that exercise reaping use
    /// `spawn_actor_with_reaper`.
    pub(super) fn spawn_actor(pipeline: TestPipeline, config: CoordinatorConfig) -> SignalHandle {
        let (_fail_tx, fail_rx) = mpsc::channel(1);
        spawn_coordinator(pipeline, config, fail_rx)
    }

    /// Spawn the coordinator sharing `branch_failures` with a pipeline built via
    /// `TestPipeline::new`, so the fake's `fail_branch` reaches this coordinator.
    pub(super) fn spawn_actor_with_reaper(
        pipeline: TestPipeline,
        config: CoordinatorConfig,
        branch_failures: mpsc::Receiver<BranchId>,
    ) -> SignalHandle {
        spawn_coordinator(pipeline, config, branch_failures)
    }

    /// Drive a full WHEP<->WHIP handshake so the connection reaches Established.
    async fn establish(handle: &SignalHandle, id: &str) {
        let whep = {
            let handle = handle.clone();
            let id = id.to_string();
            tokio::spawn(async move { handle.create_connection(id).await })
        };
        tokio::task::yield_now().await; // connection registered
        let whip = {
            let handle = handle.clone();
            let id = id.to_string();
            tokio::spawn(async move { handle.offer_received(id, offer()).await })
        };
        tokio::task::yield_now().await; // offer delivered
        handle
            .answer_received(id.to_string(), answer())
            .await
            .unwrap();
        whep.await.unwrap().unwrap();
        whip.await.unwrap().unwrap();
    }

    async fn list_ids(handle: &SignalHandle) -> Vec<String> {
        handle
            .list_connections()
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect()
    }

    #[tokio::test(start_paused = true)]
    async fn happy_path_create_offer_answer() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        // Browser connects; create stays in-flight until the offer arrives.
        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        for _ in 0..5 {
            tokio::task::yield_now().await; // let the actor register + add the branch
        }
        assert_eq!(vec!["a".to_string()], pipeline.snapshot().added);

        // Whipsink posts its offer; the browser's waiter receives it.
        let whip = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.offer_received("a".to_string(), offer()).await })
        };
        let delivered = whep.await.unwrap().unwrap();
        assert!(delivered.is_sendonly());

        // Browser PATCHes the answer; the whipsink's waiter receives it.
        handle
            .answer_received("a".to_string(), answer())
            .await
            .unwrap();
        let delivered = whip.await.unwrap().unwrap();
        assert!(!delivered.is_sendonly());

        // Nothing was torn down.
        let snap = pipeline.snapshot();
        assert!(snap.removed.is_empty());
        assert_eq!(0, snap.quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn offer_timeout_fails_only_that_connection() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        // Awaiting create parks on its reply; the paused clock auto-advances
        // through sweep ticks until the offer deadline fires.
        let result = handle.create_connection("a".to_string()).await;
        assert!(matches!(result, Err(SignalError::Timeout("SDP offer"))));
        tokio::task::yield_now().await; // let the actor finish branch cleanup

        let snap = pipeline.snapshot();
        assert_eq!(vec!["a".to_string()], snap.removed);
        assert_eq!(0, snap.quit_count); // one failure: watchdog not tripped
    }

    #[tokio::test(start_paused = true)]
    async fn answer_timeout_fails_the_whip_waiter() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        tokio::task::yield_now().await;
        let whip = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.offer_received("a".to_string(), offer()).await })
        };
        assert!(whep.await.unwrap().is_ok()); // offer delivered to the create waiter

        // No PATCH arrives; the answer deadline fires.
        let result = whip.await.unwrap();
        assert!(matches!(result, Err(SignalError::Timeout("SDP answer"))));
        tokio::task::yield_now().await; // let the actor finish branch cleanup
        assert_eq!(vec!["a".to_string()], pipeline.snapshot().removed);
    }

    #[tokio::test(start_paused = true)]
    async fn abandoned_whep_client_is_reaped_by_the_sweep() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        for _ in 0..5 {
            tokio::task::yield_now().await; // let the actor register the connection
        }
        whep.abort(); // browser disconnected; the in-flight request future is dropped
        tokio::task::yield_now().await; // let the abort drop the reply receiver

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
        let handle = spawn_actor(pipeline.clone(), test_config());

        let err = handle.create_connection("a".to_string()).await.unwrap_err();
        // Retryable at the seam stays retryable on the wire: 503 + Retry-After.
        assert_eq!(503, err.status_code().as_u16());
        assert!(err.error_response().headers().get("Retry-After").is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn not_ready_pipeline_rejects_creation() {
        let pipeline = TestPipeline::default(); // ready = false
        let handle = spawn_actor(pipeline.clone(), test_config());

        assert!(matches!(
            handle.create_connection("a".to_string()).await,
            Err(SignalError::NotReady)
        ));
        assert!(pipeline.snapshot().added.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_id_is_not_found_for_every_command() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        assert!(matches!(
            handle.offer_received("ghost".to_string(), offer()).await,
            Err(SignalError::NotFound(_))
        ));
        assert!(matches!(
            handle.answer_received("ghost".to_string(), answer()).await,
            Err(SignalError::NotFound(_))
        ));
        assert!(matches!(
            handle.remove_connection("ghost".to_string()).await,
            Err(SignalError::NotFound(_))
        ));
    }

    #[tokio::test(start_paused = true)]
    async fn wrong_state_commands_are_rejected_without_corruption() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        // First create stays in-flight (awaiting its offer).
        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        tokio::task::yield_now().await; // register "a"

        // Duplicate create is rejected.
        assert!(matches!(
            handle.create_connection("a".to_string()).await,
            Err(SignalError::WrongState(_))
        ));
        // PATCH before the offer exists is a wrong-state command.
        assert!(matches!(
            handle.answer_received("a".to_string(), answer()).await,
            Err(SignalError::WrongState(_))
        ));

        // The original handshake still works after both rejections.
        let whip = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.offer_received("a".to_string(), offer()).await })
        };
        assert!(whep.await.unwrap().is_ok()); // offer delivered to the first create

        handle
            .answer_received("a".to_string(), answer())
            .await
            .unwrap();
        assert!(whip.await.unwrap().is_ok());
        assert_eq!(1, pipeline.snapshot().added.len()); // no duplicate branch
    }

    #[tokio::test(start_paused = true)]
    async fn watchdog_trips_after_three_consecutive_failures() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config()); // threshold 3

        for i in 0..3 {
            // Each handshake times out via auto-advance.
            assert!(matches!(
                handle.create_connection(format!("conn-{}", i)).await,
                Err(SignalError::Timeout(_))
            ));
        }

        tokio::task::yield_now().await; // let the actor finish the trip handling
        assert_eq!(1, pipeline.snapshot().quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn success_between_failures_prevents_the_trip() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        // Two failures.
        for i in 0..2 {
            let _ = handle.create_connection(format!("fail-{}", i)).await;
        }

        // One success resets the counter.
        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("ok".to_string()).await })
        };
        tokio::task::yield_now().await;
        let whip = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.offer_received("ok".to_string(), offer()).await })
        };
        assert!(whep.await.unwrap().is_ok());
        handle
            .answer_received("ok".to_string(), answer())
            .await
            .unwrap();
        assert!(whip.await.unwrap().is_ok());

        // Two more failures: still below threshold thanks to the reset.
        for i in 2..4 {
            let _ = handle.create_connection(format!("fail-{}", i)).await;
        }

        tokio::task::yield_now().await;
        assert_eq!(0, pipeline.snapshot().quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn reset_clears_the_watchdog_counter() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config()); // threshold 3

        // Two consecutive timeout failures.
        for i in 0..2 {
            let _ = handle.create_connection(format!("fail-{}", i)).await;
        }

        // Pipeline restarted: supervisor sends Reset.
        handle.reset().await.unwrap();

        // Two more failures on the fresh pipeline: still below threshold.
        for i in 2..4 {
            let _ = handle.create_connection(format!("fail-{}", i)).await;
        }

        tokio::task::yield_now().await;
        assert_eq!(0, pipeline.snapshot().quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn reset_fails_all_waiters_and_clears_state() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        tokio::task::yield_now().await; // register the in-flight waiter

        handle.reset().await.unwrap();

        assert!(matches!(whep.await.unwrap(), Err(SignalError::Unavailable)));
        assert!(list_ids(&handle).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn list_reports_ids_and_state_names() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());

        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        for _ in 0..5 {
            tokio::task::yield_now().await; // register the connection
        }

        let list = handle.list_connections().await.unwrap();
        assert_eq!(1, list.len());
        assert_eq!("a", list[0].id);
        assert_eq!("awaiting_offer", list[0].state);

        whep.abort(); // in-flight create no longer needed
    }

    #[tokio::test(start_paused = true)]
    async fn failed_add_registers_nothing_and_needs_no_coordinator_cleanup() {
        use crate::stream::PipelineError;

        let pipeline = ready_pipeline();
        pipeline.fail_next_add_branch(PipelineError::Fatal("attach blew up".into()));
        let handle = spawn_actor(pipeline.clone(), test_config());

        // The create fails and the id is never registered...
        assert!(handle.create_connection("a".to_string()).await.is_err());
        tokio::task::yield_now().await;

        // ...and the coordinator issues NO cleanup: add_branch owns detaching
        // its own half-attached branch now, so a spurious remove_branch here
        // would be a bug (and today's matches!(Fatal) block causes exactly one).
        assert!(pipeline.snapshot().removed.is_empty());
        assert!(list_ids(&handle).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn failed_delete_keeps_the_connection_retryable() {
        use crate::stream::PipelineError;

        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config());
        establish(&handle, "a").await;

        // First DELETE: the branch teardown fails transiently.
        pipeline.fail_next_remove_branch(PipelineError::Transient("lock timed out".into()));
        assert!(matches!(
            handle.remove_connection("a".to_string()).await,
            Err(SignalError::PipelineBusy(_))
        ));

        // The connection is kept, so a retried DELETE still finds it (no 404).
        assert_eq!(vec!["a".to_string()], list_ids(&handle).await);

        // Retried DELETE succeeds, removes the branch, and clears the entry.
        handle.remove_connection("a".to_string()).await.unwrap();
        assert!(pipeline.snapshot().removed.contains(&"a".to_string()));
        assert!(list_ids(&handle).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn established_connection_is_reaped_on_branch_failure() {
        // The fake shares its failure channel with the coordinator, mirroring
        // how the real pipeline's bus watch reaches the actor.
        let (fail_tx, fail_rx) = mpsc::channel(64);
        let pipeline = TestPipeline::new(fail_tx);
        pipeline.set_ready(true);
        let handle = spawn_actor_with_reaper(pipeline.clone(), test_config(), fail_rx);
        establish(&handle, "a").await;

        // The pipeline's bus watch reports the branch errored at runtime.
        pipeline.fail_branch("a");
        for _ in 0..5 {
            tokio::task::yield_now().await; // let the actor drain the reap channel
        }

        // The established connection is reaped: branch detached, entry gone.
        assert!(pipeline.snapshot().removed.contains(&"a".to_string()));
        assert!(list_ids(&handle).await.is_empty());
        // A dead peer is not a pipeline-health failure: no restart.
        assert_eq!(0, pipeline.snapshot().quit_count);
    }

    #[tokio::test(start_paused = true)]
    async fn wedged_teardown_does_not_stall_the_actor() {
        let pipeline = ready_pipeline();
        let handle = spawn_actor(pipeline.clone(), test_config()); // teardown_timeout 5s
        establish(&handle, "a").await;

        // Its teardown wedges and never completes.
        pipeline.block_remove_branch();
        assert!(matches!(
            handle.remove_connection("a".to_string()).await,
            Err(SignalError::PipelineBusy(_))
        ));

        // The actor is still responsive, and the connection stayed retryable.
        assert_eq!(vec!["a".to_string()], list_ids(&handle).await);
    }

    #[tokio::test(start_paused = true)]
    async fn wedged_add_branch_cleanup_times_out_to_a_retryable_error() {
        use actix_web::ResponseError;

        let pipeline = ready_pipeline();
        pipeline.block_add_branch(); // simulates a wedged cleanup detach
        let handle = spawn_actor(pipeline.clone(), test_config()); // teardown_timeout 5s

        let err = handle.create_connection("a".to_string()).await.unwrap_err();
        // The bound fires (auto-advanced paused clock) and the actor recovers
        // with a retryable error instead of hanging forever.
        assert_eq!(503, err.status_code().as_u16());
        assert!(err.error_response().headers().get("Retry-After").is_some());
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

    #[tokio::test]
    async fn transition_table_accepts_only_legal_events() {
        use super::{AnswerDelivery, OfferDelivery};

        // AwaitingOffer: an offer is legal, an answer is not.
        let (tx, _rx) = oneshot::channel();
        let s = ConnectionState::awaiting_offer(tx, Instant::now());
        assert!(matches!(
            s.deliver_offer(offer()),
            Ok(OfferDelivery::Delivered)
        ));

        let (tx, _rx) = oneshot::channel();
        let s = ConnectionState::awaiting_offer(tx, Instant::now());
        assert!(matches!(
            s.deliver_answer(answer()),
            Err(ConnectionState::AwaitingOffer { .. })
        ));

        // AwaitingAnswer: an answer is legal, an offer is not.
        let (tx, _rx) = oneshot::channel();
        let s = ConnectionState::awaiting_answer(tx, Instant::now());
        assert!(matches!(
            s.deliver_answer(answer()),
            Ok(AnswerDelivery::Established)
        ));

        let (tx, _rx) = oneshot::channel();
        let s = ConnectionState::awaiting_answer(tx, Instant::now());
        assert!(matches!(
            s.deliver_offer(offer()),
            Err(ConnectionState::AwaitingAnswer { .. })
        ));

        // Established: neither is legal.
        let s = ConnectionState::established(Instant::now());
        assert!(matches!(
            s.deliver_offer(offer()),
            Err(ConnectionState::Established { .. })
        ));
        let s = ConnectionState::established(Instant::now());
        assert!(matches!(
            s.deliver_answer(answer()),
            Err(ConnectionState::Established { .. })
        ));
    }
}
