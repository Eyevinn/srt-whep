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
    /// Upper bound on a single branch teardown (`remove_branch`). This runs
    /// inline in the actor's select loop; bounding it keeps one wedged
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
    /// Upper bound, in seconds, on a single branch teardown.
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

    /// Project this connection into its GET /list entry.
    fn info(&self, id: &ConnectionId) -> ConnectionInfo {
        ConnectionInfo {
            id: id.clone(),
            state: self.name().to_string(),
        }
    }

    /// Fail whichever reply waiter is parked on this connection, if any:
    /// the connection is going away, so a client still awaiting a reply
    /// must learn it now. An `Established` connection has no parked waiter,
    /// so this is a no-op.
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

    /// Deadline for the handshake leg this connection is parked on, if any;
    /// the sweep expires the connection once it passes. `Established` has
    /// no deadline and never expires.
    fn deadline(&self) -> Option<Instant> {
        match self {
            ConnectionState::AwaitingOffer { deadline, .. }
            | ConnectionState::AwaitingAnswer { deadline, .. } => Some(*deadline),
            ConnectionState::Established { .. } => None,
        }
    }

    /// Fail the parked waiter with its own leg's expiry error: the WHEP
    /// client was waiting for the SDP offer, the whipsink for the answer.
    fn expire(self) {
        match self {
            ConnectionState::AwaitingOffer { whep_reply, .. } => {
                let _ = whep_reply.send(Err(SignalError::Timeout("SDP offer")));
            }
            ConnectionState::AwaitingAnswer { whip_reply, .. } => {
                let _ = whip_reply.send(Err(SignalError::Timeout("SDP answer")));
            }
            ConnectionState::Established { .. } => {}
        }
    }

    /// Apply a `WaiterNotice` from the termination policy table: turn the
    /// notice into the concrete error the parked waiter receives.
    fn notify(self, notice: WaiterNotice, id: &ConnectionId) {
        match notice {
            WaiterNotice::Gone => self.fail_waiter(SignalError::NotFound(id.clone())),
            WaiterNotice::ExpiredLeg => self.expire(),
            WaiterNotice::Unavailable => self.fail_waiter(SignalError::Unavailable),
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

/// Why a connection is ending. Every death path in the coordinator names
/// one of these, and `policy` maps it to the single table saying what
/// termination does. A new way to die means a new variant and a new row —
/// there is no default.
#[derive(Clone, Copy, Debug)]
enum TerminateReason {
    /// A client DELETE (WHEP or loopback WHIP).
    Deleted,
    /// The sweep found the offer/answer deadline passed.
    Expired,
    /// The parked waiter's request died mid-handshake (actix dropped its
    /// handler future); the failed SDP delivery already consumed the entry.
    PeerGone,
    /// The pipeline's bus watch reported the branch failed at runtime.
    Reaped,
    /// The supervisor restarted the pipeline, or the watchdog tripped.
    Reset,
}

/// One row of the termination policy table.
struct TerminationPolicy {
    on_missing: MissingEntry,
    waiter: WaiterNotice,
    teardown: Teardown,
    feeds_watchdog: bool,
}

/// What `terminate` does when the id is no longer in the connection map.
#[derive(Clone, Copy)]
enum MissingEntry {
    /// The caller named an unknown connection: reply `NotFound`.
    Reject,
    /// The death raced another termination; everything is already done.
    Skip,
    /// Expected absence — the branch/watchdog consequences still apply.
    Proceed,
}

/// How a parked waiter learns its connection is terminating.
#[derive(Clone, Copy)]
enum WaiterNotice {
    /// The connection no longer exists: `NotFound`. (For `PeerGone` the
    /// waiter itself vanished, so there is nobody left to notify; the
    /// surviving leg's reply is the calling handler's to answer.)
    Gone,
    /// The handshake deadline passed: the waiter gets its leg's `Timeout`.
    ExpiredLeg,
    /// The signaling plane is resetting: `Unavailable`.
    Unavailable,
}

/// Whether termination detaches the connection's pipeline branch.
#[derive(Clone, Copy)]
enum Teardown {
    /// Teardown gates the death: on failure the entry is restored and the
    /// error propagates, so a retried DELETE still finds the connection.
    Required,
    /// The connection dies regardless; a teardown failure is only logged.
    BestEffort,
    /// Leave the branch alone: the supervisor is about to tear down or
    /// rebuild the whole pipeline around this reset.
    Keep,
}

impl TerminateReason {
    /// THE termination policy table (ADR-0001/0002 pin these semantics).
    /// Rows that used to live as prose comments in five separate death
    /// paths are values here — most notably that `Reaped` does NOT feed
    /// the watchdog (a dead peer is a fact about one viewer, not a
    /// pipeline-health signal), while an expired or abandoned handshake
    /// (`Expired`, `PeerGone`) counts toward a restart.
    fn policy(self) -> TerminationPolicy {
        match self {
            TerminateReason::Deleted => TerminationPolicy {
                on_missing: MissingEntry::Reject,
                waiter: WaiterNotice::Gone,
                teardown: Teardown::Required,
                feeds_watchdog: false,
            },
            TerminateReason::Expired => TerminationPolicy {
                on_missing: MissingEntry::Skip,
                waiter: WaiterNotice::ExpiredLeg,
                teardown: Teardown::BestEffort,
                feeds_watchdog: true,
            },
            TerminateReason::PeerGone => TerminationPolicy {
                on_missing: MissingEntry::Proceed,
                waiter: WaiterNotice::Gone,
                teardown: Teardown::BestEffort,
                feeds_watchdog: true,
            },
            TerminateReason::Reaped => TerminationPolicy {
                on_missing: MissingEntry::Skip,
                waiter: WaiterNotice::Gone,
                teardown: Teardown::BestEffort,
                feeds_watchdog: false,
            },
            TerminateReason::Reset => TerminationPolicy {
                on_missing: MissingEntry::Skip,
                waiter: WaiterNotice::Unavailable,
                teardown: Teardown::Keep,
                feeds_watchdog: false,
            },
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
    /// Watchdog restart requests to the supervisor. On a trip the coordinator
    /// fails all waiters and sends `()` here; the supervisor owns the actual
    /// force-quit + rerun. A non-blocking `try_send` (coalescing) so a wedged
    /// pipeline can never stall this mailbox.
    restart_tx: mpsc::Sender<()>,
}

impl<P: BranchControl> Coordinator<P> {
    pub fn new(
        pipeline: P,
        config: CoordinatorConfig,
        rx: mpsc::Receiver<Command>,
        branch_failures: mpsc::Receiver<BranchId>,
        restart_tx: mpsc::Sender<()>,
    ) -> Self {
        let watchdog = Watchdog::new(config.watchdog_threshold, config.watchdog_window);
        Self {
            pipeline,
            config,
            connections: HashMap::new(),
            watchdog,
            rx,
            branch_failures,
            restart_tx,
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
                let _ = reply.send(Ok(self.list_connections()));
            }
            Command::Reset { reply } => {
                self.reset();
                let _ = reply.send(Ok(()));
            }
        }
    }

    /// Snapshot every connection for GET /list.
    fn list_connections(&self) -> Vec<ConnectionInfo> {
        self.connections
            .iter()
            .map(|(id, state)| state.info(id))
            .collect()
    }

    /// The supervisor restarted the pipeline: fail every waiter, clear the
    /// map, and clear the watchdog — the fresh pipeline starts with a clean
    /// bill of health, so stale failures must not count toward its trip.
    fn reset(&mut self) {
        self.reset_all();
        self.watchdog.record_success();
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
                let _ = self.terminate(id, TerminateReason::PeerGone).await;
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
                let _ = self.terminate(id, TerminateReason::PeerGone).await;
            }
            Err(other) => {
                self.connections.insert(id.clone(), other);
                let _ = reply.send(Err(SignalError::WrongState(id)));
            }
        }
    }

    async fn remove_connection(&mut self, id: ConnectionId, reply: UnitReply) {
        let _ = reply.send(self.terminate(id, TerminateReason::Deleted).await);
    }

    async fn sweep_expired(&mut self) {
        let now = Instant::now();
        let expired: Vec<ConnectionId> = self
            .connections
            .iter()
            .filter(|(_, state)| state.deadline().is_some_and(|d| d <= now))
            .map(|(id, _)| id.clone())
            .collect();

        for id in expired {
            tracing::warn!("Handshake for {} timed out", id);
            let _ = self.terminate(id, TerminateReason::Expired).await;
        }
    }

    /// The single owner of "a connection is ending". Every death path names
    /// its `TerminateReason`; the policy row for that reason decides what
    /// actually happens — how a parked waiter is failed, whether the branch
    /// teardown gates the death, and whether the failure feeds the
    /// watchdog. Only `Deleted` can return an error: its failed teardown
    /// restores the entry so a retried DELETE still finds the connection.
    async fn terminate(
        &mut self,
        id: ConnectionId,
        reason: TerminateReason,
    ) -> Result<(), SignalError> {
        let policy = reason.policy();

        let state = match (self.connections.remove(&id), policy.on_missing) {
            (Some(state), _) => Some(state),
            (None, MissingEntry::Reject) => return Err(SignalError::NotFound(id)),
            (None, MissingEntry::Skip) => return Ok(()),
            (None, MissingEntry::Proceed) => None,
        };

        match policy.teardown {
            // Teardown first; the waiter learns the connection is gone only
            // once it really is. On failure the entry is restored and the
            // (retryable) error surfaces instead — dropping it here would
            // 404 a retried DELETE while leaking the branch.
            Teardown::Required => {
                if let Err(e) = self.remove_branch_bounded(id.clone()).await {
                    if let Some(state) = state {
                        self.connections.insert(id, state);
                    }
                    return Err(e);
                }
                if let Some(state) = state {
                    state.notify(policy.waiter, &id);
                }
            }
            // The connection dies regardless of what the detach says:
            // notify the waiter immediately, then detach, only logging a
            // teardown failure.
            Teardown::BestEffort => {
                if let Some(state) = state {
                    state.notify(policy.waiter, &id);
                }
                if let Err(e) = self.remove_branch_bounded(id.clone()).await {
                    tracing::error!("Failed to remove branch for {}: {}", id, e);
                }
            }
            Teardown::Keep => {
                if let Some(state) = state {
                    state.notify(policy.waiter, &id);
                }
            }
        }

        if policy.feeds_watchdog && self.watchdog.record_failure() {
            tracing::error!("Watchdog tripped: requesting a pipeline restart");
            self.reset_all();
            // Non-blocking: the supervisor owns the force-quit + rerun. A full
            // buffer means a restart is already pending, so dropping the extra
            // request is correct.
            let _ = self.restart_tx.try_send(());
        }
        Ok(())
    }

    /// A per-viewer branch failed at runtime (its whipsink errored, its peer
    /// went away), reported by the pipeline's bus watch. Terminate it so it
    /// can't linger as a ghost `/list` entry with orphaned elements.
    async fn reap_branch(&mut self, id: ConnectionId) {
        if !self.connections.contains_key(&id) {
            return; // already gone: raced a DELETE or an expiry sweep
        }
        tracing::warn!("Reaping branch for {} after a runtime failure", id);
        let _ = self.terminate(id, TerminateReason::Reaped).await;
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

    /// Apply the `Reset` termination row to the whole map at once: every
    /// parked waiter learns the signaling plane is unavailable, and the
    /// branches are deliberately kept (`Teardown::Keep`) — the supervisor
    /// tears down or rebuilds the pipeline wholesale around this call.
    /// `terminate` is per-connection; a reset is the one whole-map death,
    /// so it applies the same policy row without going through it.
    fn reset_all(&mut self) {
        let policy = TerminateReason::Reset.policy();
        for (id, state) in self.connections.drain() {
            state.notify(policy.waiter, &id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ConnectionState;
    use super::CoordinatorConfig;
    use crate::domain::{SdpAnswer, SdpOffer, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
    use crate::signal::{spawn_coordinator, ResetHandle, ResetSignal, SignalError, SignalHandle};
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

    /// Spawn the coordinator and return its `SignalHandle` plus the receiving
    /// end of the watchdog restart channel. No reaper wired (disconnected
    /// failure receiver). Tests that trip the watchdog assert on `restart_rx`.
    pub(super) fn spawn_actor(
        pipeline: TestPipeline,
        config: CoordinatorConfig,
    ) -> (SignalHandle, mpsc::Receiver<()>) {
        let (handle, _reset, restart_rx) = spawn_actor_with_reset(pipeline, config);
        (handle, restart_rx)
    }

    /// Like `spawn_actor`, but also returns the supervisor-side `ResetHandle`
    /// for tests that exercise the Reset command.
    pub(super) fn spawn_actor_with_reset(
        pipeline: TestPipeline,
        config: CoordinatorConfig,
    ) -> (SignalHandle, ResetHandle, mpsc::Receiver<()>) {
        let (_fail_tx, fail_rx) = mpsc::channel(1);
        let (restart_tx, restart_rx) = mpsc::channel(1);
        let (handle, reset) = spawn_coordinator(pipeline, config, fail_rx, restart_tx);
        (handle, reset, restart_rx)
    }

    /// Spawn the coordinator sharing `branch_failures` with a pipeline built via
    /// `TestPipeline::new`, so the fake's `fail_branch` reaches this coordinator.
    pub(super) fn spawn_actor_with_reaper(
        pipeline: TestPipeline,
        config: CoordinatorConfig,
        branch_failures: mpsc::Receiver<BranchId>,
    ) -> (SignalHandle, mpsc::Receiver<()>) {
        let (restart_tx, restart_rx) = mpsc::channel(1);
        let (handle, _reset) = spawn_coordinator(pipeline, config, branch_failures, restart_tx);
        (handle, restart_rx)
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
        let (handle, mut restart_rx) = spawn_actor(pipeline.clone(), test_config());

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
        assert!(restart_rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn offer_timeout_fails_only_that_connection() {
        let pipeline = ready_pipeline();
        let (handle, mut restart_rx) = spawn_actor(pipeline.clone(), test_config());

        // Awaiting create parks on its reply; the paused clock auto-advances
        // through sweep ticks until the offer deadline fires.
        let result = handle.create_connection("a".to_string()).await;
        assert!(matches!(result, Err(SignalError::Timeout("SDP offer"))));
        tokio::task::yield_now().await; // let the actor finish branch cleanup

        let snap = pipeline.snapshot();
        assert_eq!(vec!["a".to_string()], snap.removed);
        assert!(restart_rx.try_recv().is_err()); // one failure: watchdog not tripped
    }

    #[tokio::test(start_paused = true)]
    async fn answer_timeout_fails_the_whip_waiter() {
        let pipeline = ready_pipeline();
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());

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
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());

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
    async fn vanished_whep_client_fails_the_handshake_and_feeds_the_watchdog() {
        let pipeline = ready_pipeline();
        let (handle, mut restart_rx) = spawn_actor(pipeline.clone(), test_config()); // threshold 3

        // Three times over: the browser vanishes before the whipsink's offer
        // arrives, so delivering the offer finds the parked waiter gone.
        for i in 0..3 {
            let id = format!("conn-{}", i);
            let whep = {
                let handle = handle.clone();
                let id = id.clone();
                tokio::spawn(async move { handle.create_connection(id).await })
            };
            for _ in 0..5 {
                tokio::task::yield_now().await; // register + add the branch
            }
            whep.abort(); // browser disconnected; its reply receiver drops
            tokio::task::yield_now().await;

            // The handshake fails now: the whipsink's surviving leg learns
            // the connection is gone...
            assert!(matches!(
                handle.offer_received(id.clone(), offer()).await,
                Err(SignalError::NotFound(_))
            ));
            // ...and, once the actor finishes cleanup, the branch is detached.
            tokio::task::yield_now().await;
            assert!(pipeline.snapshot().removed.contains(&id));
        }

        // Three abandoned handshakes in a row are a pipeline-health signal:
        // the watchdog trips — unlike the reap path, which never feeds it.
        tokio::task::yield_now().await;
        assert!(restart_rx.try_recv().is_ok());
    }

    #[tokio::test(start_paused = true)]
    async fn vanished_whip_waiter_fails_the_answer_delivery() {
        let pipeline = ready_pipeline();
        let (handle, mut restart_rx) = spawn_actor(pipeline.clone(), test_config());

        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        tokio::task::yield_now().await; // connection registered
        let whip = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.offer_received("a".to_string(), offer()).await })
        };
        assert!(whep.await.unwrap().is_ok()); // offer delivered; whip now parked

        whip.abort(); // the whipsink's HTTP request died mid-wait
        tokio::task::yield_now().await;

        // The browser's PATCH finds the whipsink's waiter gone: the
        // handshake fails, the entry is dropped, the branch is detached.
        assert!(matches!(
            handle.answer_received("a".to_string(), answer()).await,
            Err(SignalError::NotFound(_))
        ));
        tokio::task::yield_now().await; // let the actor finish branch cleanup
        assert_eq!(vec!["a".to_string()], pipeline.snapshot().removed);
        assert!(list_ids(&handle).await.is_empty());
        // One vanished peer is below the trip threshold: no restart.
        assert!(restart_rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn transient_pipeline_failure_stays_retryable() {
        use crate::stream::PipelineError;
        use actix_web::ResponseError;

        let pipeline = ready_pipeline();
        pipeline.fail_next_add_branch(PipelineError::Transient("state lock timed out".into()));
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());

        let err = handle.create_connection("a".to_string()).await.unwrap_err();
        // Retryable at the seam stays retryable on the wire: 503 + Retry-After.
        assert_eq!(503, err.status_code().as_u16());
        assert!(err.error_response().headers().get("Retry-After").is_some());
    }

    #[tokio::test(start_paused = true)]
    async fn not_ready_pipeline_rejects_creation() {
        let pipeline = TestPipeline::default(); // ready = false
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());

        assert!(matches!(
            handle.create_connection("a".to_string()).await,
            Err(SignalError::NotReady)
        ));
        assert!(pipeline.snapshot().added.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn unknown_id_is_not_found_for_every_command() {
        let pipeline = ready_pipeline();
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());

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
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());

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
        let (handle, mut restart_rx) = spawn_actor(pipeline.clone(), test_config()); // threshold 3

        for i in 0..3 {
            // Each handshake times out via auto-advance.
            assert!(matches!(
                handle.create_connection(format!("conn-{}", i)).await,
                Err(SignalError::Timeout(_))
            ));
        }

        tokio::task::yield_now().await; // let the actor finish the trip handling
        assert!(restart_rx.try_recv().is_ok()); // one restart requested
    }

    #[tokio::test(start_paused = true)]
    async fn success_between_failures_prevents_the_trip() {
        let pipeline = ready_pipeline();
        let (handle, mut restart_rx) = spawn_actor(pipeline.clone(), test_config());

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
        assert!(restart_rx.try_recv().is_err()); // no restart requested
    }

    #[tokio::test(start_paused = true)]
    async fn reset_clears_the_watchdog_counter() {
        let pipeline = ready_pipeline();
        let (handle, reset, mut restart_rx) =
            spawn_actor_with_reset(pipeline.clone(), test_config()); // threshold 3

        // Two consecutive timeout failures.
        for i in 0..2 {
            let _ = handle.create_connection(format!("fail-{}", i)).await;
        }

        // Pipeline restarted: supervisor sends Reset.
        reset.reset().await.unwrap();

        // Two more failures on the fresh pipeline: still below threshold.
        for i in 2..4 {
            let _ = handle.create_connection(format!("fail-{}", i)).await;
        }

        tokio::task::yield_now().await;
        assert!(restart_rx.try_recv().is_err()); // no restart requested
    }

    #[tokio::test(start_paused = true)]
    async fn reset_fails_all_waiters_and_clears_state() {
        let pipeline = ready_pipeline();
        let (handle, reset, _restart_rx) = spawn_actor_with_reset(pipeline.clone(), test_config());

        let whep = {
            let handle = handle.clone();
            tokio::spawn(async move { handle.create_connection("a".to_string()).await })
        };
        tokio::task::yield_now().await; // register the in-flight waiter

        reset.reset().await.unwrap();

        assert!(matches!(whep.await.unwrap(), Err(SignalError::Unavailable)));
        assert!(list_ids(&handle).await.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn list_reports_ids_and_state_names() {
        let pipeline = ready_pipeline();
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());

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
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());

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
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());
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
        let (handle, mut restart_rx) =
            spawn_actor_with_reaper(pipeline.clone(), test_config(), fail_rx);
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
        assert!(restart_rx.try_recv().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn wedged_teardown_does_not_stall_the_actor() {
        let pipeline = ready_pipeline();
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config()); // teardown_timeout 5s
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
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config()); // teardown_timeout 5s

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
