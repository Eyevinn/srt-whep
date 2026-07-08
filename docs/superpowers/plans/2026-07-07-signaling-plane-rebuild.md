# Signaling-Plane Rebuild Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the `AppState` SDP rendezvous with a coordinator actor (`src/signal/`) giving per-connection failure isolation, a watchdog fallback, and a three-layer test suite.

**Architecture:** One tokio task (the coordinator) owns all connection state and all pipeline `add_connection`/`remove_connection` calls, serialized through an mpsc mailbox. HTTP handlers become thin adapters that send a command and await a oneshot reply. Timeouts are enforced by a sweep inside the actor, not by handlers.

**Tech Stack:** Rust 1.84, actix-web 4, tokio (mpsc/oneshot/interval), thiserror, existing `PipelineBase` trait. Spec: `docs/superpowers/specs/2026-07-07-signaling-plane-rebuild-design.md`.

## Global Constraints

- Branch: `signaling-plane-rebuild`. Repo root: `/Users/kunwu/Workspace/srt/srt-whep`.
- Pre-commit hooks run `cargo fmt`, `cargo check`, `cargo clippy` on every commit — run `cargo fmt` before each commit; a failing hook means the step is not done.
- `cargo test` must pass at the end of every task (e2e tests are `#[ignore]`d and excluded from this rule).
- Do NOT modify `src/stream/gst_pipeline.rs` or any GStreamer element logic.
- Config defaults, copied from spec: `offer_timeout` 10s, `answer_timeout` 10s, `watchdog_threshold` 3, `sweep_interval` 1s.
- Status-code contract (spec): invalid SDP → 400, unknown id → 404, wrong state → 409, timeout/not-ready → 503 with `Retry-After: 3`, coordinator gone/pipeline failure → 500. Happy paths unchanged: POST /channel → 201+Location+offer, PATCH → 204, whip POST → 201+Location+answer, DELETE → 200.
- Unit tests for the actor use `#[tokio::test(start_paused = true)]` (single-threaded, paused clock). Two rules for determinism: (1) after `tx.send(...)`, call `tokio::task::yield_now().await` to let the actor process the message before you advance time; (2) a test that awaits a reply while nothing else can run will auto-advance the clock to the next timer — that is how timeout tests fire instantly.

---

### Task 1: Signal module scaffolding + watchdog

**Files:**
- Modify: `src/lib.rs`
- Modify: `Cargo.toml:22` (tokio features), `Cargo.toml:23` (serde derive), `Cargo.toml:58-61` (dev-deps)
- Create: `src/signal/mod.rs`, `src/signal/watchdog.rs`

**Interfaces:**
- Produces: `signal::watchdog::Watchdog` — `new(threshold: u32)`, `record_failure(&mut self) -> bool` (true = tripped, counter resets), `record_success(&mut self)`.

- [ ] **Step 1: Declare dependencies explicitly**

In `Cargo.toml` `[dependencies]`, change the tokio and serde lines to:

```toml
tokio = { version = "1", features = ["macros", "rt-multi-thread", "sync", "time"] }
serde = { version = "1.0.217", features = ["derive"] }
```

In `[dev-dependencies]`, add:

```toml
tokio = { version = "1", features = ["test-util"] }
```

(`sync`/`time` are used by the new module directly; `test-util` provides the paused clock. Both may already be enabled transitively — declaring them makes the crate honest.)

- [ ] **Step 2: Write the failing watchdog test**

Create `src/signal/watchdog.rs`:

```rust
/// Counts consecutive handshake failures. When the count reaches the
/// threshold the watchdog "trips": record_failure returns true and the
/// counter resets, so the caller restarts the pipeline exactly once.
pub struct Watchdog {
    consecutive_failures: u32,
    threshold: u32,
}

#[cfg(test)]
mod tests {
    use super::Watchdog;

    #[test]
    fn trips_exactly_at_threshold_and_resets() {
        let mut dog = Watchdog::new(3);
        assert!(!dog.record_failure());
        assert!(!dog.record_failure());
        assert!(dog.record_failure()); // third consecutive failure trips
        assert!(!dog.record_failure()); // counter restarted after trip
    }

    #[test]
    fn success_resets_the_counter() {
        let mut dog = Watchdog::new(2);
        assert!(!dog.record_failure());
        dog.record_success();
        assert!(!dog.record_failure()); // would have tripped without the success
        assert!(dog.record_failure());
    }
}
```

Create `src/signal/mod.rs`:

```rust
mod watchdog;
```

Add to `src/lib.rs` (after `pub mod routes;`):

```rust
pub mod signal;
```

- [ ] **Step 3: Run test to verify it fails**

Run: `cargo test signal::watchdog -- --nocapture`
Expected: compile error — `Watchdog::new` and methods not defined.

- [ ] **Step 4: Implement Watchdog**

Append to `src/signal/watchdog.rs` (above the tests module):

```rust
impl Watchdog {
    pub fn new(threshold: u32) -> Self {
        Self {
            consecutive_failures: 0,
            threshold,
        }
    }

    pub fn record_failure(&mut self) -> bool {
        self.consecutive_failures += 1;
        if self.consecutive_failures >= self.threshold {
            self.consecutive_failures = 0;
            true
        } else {
            false
        }
    }

    pub fn record_success(&mut self) {
        self.consecutive_failures = 0;
    }
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test signal::watchdog`
Expected: 2 passed.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add Cargo.toml Cargo.lock src/lib.rs src/signal/
git commit -m "feat(signal): add module scaffolding and failure watchdog"
```

---

### Task 2: SignalError and command types

**Files:**
- Create: `src/signal/errors.rs`, `src/signal/messages.rs`
- Modify: `src/signal/mod.rs`

**Interfaces:**
- Consumes: `crate::domain::SessionDescription` (existing).
- Produces:
  - `signal::SignalError` — variants `InvalidSdp(String)`, `NotFound(String)`, `WrongState(String)`, `Timeout(&'static str)`, `NotReady`, `Unavailable`, `Pipeline(String)`; implements `actix_web::ResponseError`.
  - `signal::messages::{Command, ConnectionId, ConnectionInfo, SdpReply, UnitReply}` with exactly the shapes below.

- [ ] **Step 1: Write the failing error-contract tests**

Create `src/signal/errors.rs`:

```rust
use actix_web::http::StatusCode;
use actix_web::{HttpResponse, ResponseError};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SignalError {
    #[error("Invalid SDP: {0}")]
    InvalidSdp(String),
    #[error("Connection {0} not found")]
    NotFound(String),
    #[error("Connection {0} is in the wrong state for this operation")]
    WrongState(String),
    #[error("Timed out waiting for the {0}")]
    Timeout(&'static str),
    #[error("Input stream is not ready")]
    NotReady,
    #[error("Signaling coordinator is unavailable")]
    Unavailable,
    #[error("Pipeline operation failed: {0}")]
    Pipeline(String),
}

#[cfg(test)]
mod tests {
    use super::SignalError;
    use actix_web::http::StatusCode;
    use actix_web::ResponseError;

    #[test]
    fn status_codes_match_the_api_contract() {
        assert_eq!(
            StatusCode::BAD_REQUEST,
            SignalError::InvalidSdp("x".into()).status_code()
        );
        assert_eq!(
            StatusCode::NOT_FOUND,
            SignalError::NotFound("x".into()).status_code()
        );
        assert_eq!(
            StatusCode::CONFLICT,
            SignalError::WrongState("x".into()).status_code()
        );
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE,
            SignalError::Timeout("SDP offer").status_code()
        );
        assert_eq!(
            StatusCode::SERVICE_UNAVAILABLE,
            SignalError::NotReady.status_code()
        );
        assert_eq!(
            StatusCode::INTERNAL_SERVER_ERROR,
            SignalError::Unavailable.status_code()
        );
        assert_eq!(
            StatusCode::INTERNAL_SERVER_ERROR,
            SignalError::Pipeline("x".into()).status_code()
        );
    }

    #[test]
    fn retriable_errors_carry_retry_after() {
        let resp = SignalError::NotReady.error_response();
        assert_eq!("3", resp.headers().get("Retry-After").unwrap());

        let resp = SignalError::Timeout("SDP answer").error_response();
        assert_eq!("3", resp.headers().get("Retry-After").unwrap());

        let resp = SignalError::NotFound("x".into()).error_response();
        assert!(resp.headers().get("Retry-After").is_none());
    }
}
```

Update `src/signal/mod.rs`:

```rust
mod errors;
mod messages;
mod watchdog;

pub use errors::SignalError;
pub use messages::{Command, ConnectionId, ConnectionInfo};
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test signal::errors`
Expected: compile error — no `ResponseError` impl (and `messages` missing).

- [ ] **Step 3: Implement ResponseError and the command types**

Append to `src/signal/errors.rs` (above the tests module):

```rust
impl ResponseError for SignalError {
    fn status_code(&self) -> StatusCode {
        match self {
            SignalError::InvalidSdp(_) => StatusCode::BAD_REQUEST,
            SignalError::NotFound(_) => StatusCode::NOT_FOUND,
            SignalError::WrongState(_) => StatusCode::CONFLICT,
            SignalError::Timeout(_) | SignalError::NotReady => StatusCode::SERVICE_UNAVAILABLE,
            SignalError::Unavailable | SignalError::Pipeline(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        }
    }

    fn error_response(&self) -> HttpResponse {
        let mut builder = HttpResponse::build(self.status_code());
        if matches!(self, SignalError::Timeout(_) | SignalError::NotReady) {
            builder.append_header(("Retry-After", "3"));
        }
        builder.body(self.to_string())
    }
}
```

Create `src/signal/messages.rs`:

```rust
use super::errors::SignalError;
use crate::domain::SessionDescription;
use serde::Serialize;
use tokio::sync::oneshot;

pub type ConnectionId = String;
pub type SdpReply = oneshot::Sender<Result<SessionDescription, SignalError>>;
pub type UnitReply = oneshot::Sender<Result<(), SignalError>>;

/// One entry in GET /list output.
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub id: String,
    pub state: String,
}

#[derive(Debug)]
pub enum Command {
    /// WHEP POST: create the connection; reply carries the SDP offer once the
    /// whipsink delivers it (or an error on timeout/failure).
    CreateConnection { id: ConnectionId, reply: SdpReply },
    /// Loopback WHIP POST: the whipsink's offer; reply carries the SDP answer
    /// once the browser PATCHes it (or an error on timeout/failure).
    OfferReceived {
        id: ConnectionId,
        sdp: SessionDescription,
        reply: SdpReply,
    },
    /// WHEP PATCH: the browser's answer; replied to immediately.
    AnswerReceived {
        id: ConnectionId,
        sdp: SessionDescription,
        reply: UnitReply,
    },
    /// WHEP DELETE (or internal cleanup).
    RemoveConnection { id: ConnectionId, reply: UnitReply },
    ListConnections {
        reply: oneshot::Sender<Vec<ConnectionInfo>>,
    },
    /// Supervisor: the pipeline restarted; fail all waiters, clear the map.
    Reset { reply: oneshot::Sender<()> },
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test signal::`
Expected: 4 passed (2 watchdog + 2 errors).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/signal/
git commit -m "feat(signal): add SignalError with HTTP mapping and command types"
```

---

### Task 3: TestPipeline recording fake

**Files:**
- Modify: `src/stream/pipeline.rs` (append; do not remove `DumpPipeline` yet — the old integration test still compiles against it until Task 9)

**Interfaces:**
- Consumes: `PipelineBase` trait (`src/stream/pipeline.rs:69-81`).
- Produces: `stream::TestPipeline` — `Default + Clone`, `set_ready(&self, bool)`, `snapshot(&self) -> TestPipelineState`; `TestPipelineState { ready: bool, added: Vec<String>, removed: Vec<String>, quit_count: u32 }`. Exported via the existing `pub use pipeline::*;` in `src/stream/mod.rs`.

- [ ] **Step 1: Write the failing test**

Append to `src/stream/pipeline.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::{PipelineBase, TestPipeline};

    #[tokio::test]
    async fn test_pipeline_records_calls() {
        let pipeline = TestPipeline::default();
        assert!(!pipeline.ready().await.unwrap());

        pipeline.set_ready(true);
        assert!(pipeline.ready().await.unwrap());

        pipeline.add_connection("a".to_string()).await.unwrap();
        pipeline.remove_connection("a".to_string()).await.unwrap();
        pipeline.quit().await.unwrap();

        let snap = pipeline.snapshot();
        assert_eq!(vec!["a".to_string()], snap.added);
        assert_eq!(vec!["a".to_string()], snap.removed);
        assert_eq!(1, snap.quit_count);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test stream::pipeline`
Expected: compile error — `TestPipeline` not defined.

- [ ] **Step 3: Implement TestPipeline**

Append to `src/stream/pipeline.rs` (above the new tests module), adding `use std::sync::Arc;` to the file's imports:

```rust
/// Snapshot of everything a test pipeline has recorded.
#[derive(Clone, Debug, Default)]
pub struct TestPipelineState {
    pub ready: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub quit_count: u32,
}

/// A recording fake for unit and integration tests: `ready` is settable and
/// every connection add/remove and quit call is recorded for assertions.
#[derive(Clone, Default)]
pub struct TestPipeline(Arc<std::sync::Mutex<TestPipelineState>>);

impl TestPipeline {
    pub fn set_ready(&self, ready: bool) {
        self.0.lock().unwrap().ready = ready;
    }

    pub fn snapshot(&self) -> TestPipelineState {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait]
impl PipelineBase for TestPipeline {
    async fn add_connection(&self, id: String) -> Result<(), Error> {
        self.0.lock().unwrap().added.push(id);
        Ok(())
    }

    async fn remove_connection(&self, id: String) -> Result<(), Error> {
        self.0.lock().unwrap().removed.push(id);
        Ok(())
    }

    async fn init(&mut self, _args: &Args) -> Result<(), Error> {
        Ok(())
    }

    async fn run(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn ready(&self) -> Result<bool, Error> {
        Ok(self.0.lock().unwrap().ready)
    }

    async fn end(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn quit(&self) -> Result<(), Error> {
        self.0.lock().unwrap().quit_count += 1;
        Ok(())
    }

    async fn clean_up(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn print(&self) -> Result<(), Error> {
        Ok(())
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test stream::pipeline`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/stream/pipeline.rs
git commit -m "feat(stream): add TestPipeline recording fake for signal tests"
```

---

### Task 4: Coordinator — happy path

**Files:**
- Create: `src/signal/coordinator.rs`
- Modify: `src/signal/mod.rs`

**Interfaces:**
- Consumes: `Command`, `SignalError`, `Watchdog`, `TestPipeline`, `PipelineBase`.
- Produces: `signal::coordinator::{Coordinator, CoordinatorConfig}` — `Coordinator::<P: PipelineBase>::new(pipeline: P, config: CoordinatorConfig, rx: mpsc::Receiver<Command>)`, `async fn run(self)`. `CoordinatorConfig { offer_timeout, answer_timeout: Duration, watchdog_threshold: u32, sweep_interval: Duration }` with `Default` = 10s/10s/3/1s.

- [ ] **Step 1: Write the failing happy-path test**

Create `src/signal/coordinator.rs` with the test module first (types referenced are implemented in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::{Coordinator, CoordinatorConfig};
    use crate::domain::{SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
    use crate::signal::messages::Command;
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
```

Update `src/signal/mod.rs`:

```rust
mod coordinator;
mod errors;
mod messages;
mod watchdog;

pub use coordinator::{Coordinator, CoordinatorConfig};
pub use errors::SignalError;
pub use messages::{Command, ConnectionId, ConnectionInfo};
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test signal::coordinator`
Expected: compile error — `Coordinator` not defined.

- [ ] **Step 3: Implement the coordinator core**

Add above the tests module in `src/signal/coordinator.rs`:

```rust
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
    AwaitingOffer { whep_reply: SdpReply, deadline: Instant },
    AwaitingAnswer { whip_reply: SdpReply, deadline: Instant },
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
                self.connections
                    .insert(id, ConnectionState::Established { since: Instant::now() });
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
```

- [ ] **Step 4: Run tests**

Run: `cargo test signal::coordinator`
Expected: 1 passed (`happy_path_create_offer_answer`).

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add src/signal/
git commit -m "feat(signal): add coordinator actor with happy-path handshake"
```

---

### Task 5: Coordinator — timeouts and abandoned clients

**Files:**
- Modify: `src/signal/coordinator.rs` (tests module only — the sweep already exists; these tests pin its behavior)

**Interfaces:**
- Consumes: everything from Task 4 (including the `pub(super)` test helpers).

- [ ] **Step 1: Add the timeout tests**

Append inside the `tests` module of `src/signal/coordinator.rs`:

```rust
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test signal::coordinator`
Expected: 4 passed. If a timeout test hangs, the sweep isn't firing — check that `run()` selects on `sweep.tick()`.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add src/signal/coordinator.rs
git commit -m "test(signal): pin timeout sweep and abandoned-client reaping"
```

---

### Task 6: Coordinator — rejection paths

**Files:**
- Modify: `src/signal/coordinator.rs` (tests module only)

- [ ] **Step 1: Add the rejection tests**

Append inside the `tests` module:

```rust
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

        assert!(matches!(
            whep_rx.await.unwrap(),
            Err(SignalError::NotReady)
        ));
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
        assert!(matches!(
            r2.await.unwrap(),
            Err(SignalError::WrongState(_))
        ));

        // PATCH before the offer exists is a wrong-state command.
        let (t3, r3) = oneshot::channel();
        tx.send(Command::AnswerReceived {
            id: "a".into(),
            sdp: answer(),
            reply: t3,
        })
        .await
        .unwrap();
        assert!(matches!(
            r3.await.unwrap(),
            Err(SignalError::WrongState(_))
        ));

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
```

- [ ] **Step 2: Run tests**

Run: `cargo test signal::coordinator`
Expected: 7 passed. These should pass without implementation changes (the logic landed in Task 4); if one fails, fix the coordinator, not the test.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add src/signal/coordinator.rs
git commit -m "test(signal): pin rejection paths for unknown ids and wrong states"
```

---

### Task 7: Coordinator — watchdog trip, Reset, List

**Files:**
- Modify: `src/signal/coordinator.rs` (tests module only)

- [ ] **Step 1: Add the tests**

Append inside the `tests` module:

```rust
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
```

- [ ] **Step 2: Run tests**

Run: `cargo test signal::`
Expected: 13 passed across the signal module (2 watchdog + 2 errors + 9 coordinator). Again: these pin Task 4 logic; fix the coordinator if one fails.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add src/signal/coordinator.rs
git commit -m "test(signal): pin watchdog trip, reset, and list behavior"
```

---

### Task 8: SignalHandle public API

**Files:**
- Modify: `src/signal/mod.rs`

**Interfaces:**
- Produces (used by routes, main, PipelineGuard, and all integration tests):
  - `signal::spawn_coordinator<P: PipelineBase + 'static>(pipeline: P, config: CoordinatorConfig) -> SignalHandle`
  - `SignalHandle` (Clone) with methods: `create_connection(id: String) -> Result<SessionDescription, SignalError>`, `offer_received(id, sdp) -> Result<SessionDescription, SignalError>`, `answer_received(id, sdp) -> Result<(), SignalError>`, `remove_connection(id) -> Result<(), SignalError>`, `list_connections() -> Result<Vec<ConnectionInfo>, SignalError>`, `reset() -> Result<(), SignalError>` (all `async`).

- [ ] **Step 1: Write the failing test**

Replace `src/signal/mod.rs` with:

```rust
mod coordinator;
mod errors;
mod messages;
mod watchdog;

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
```

- [ ] **Step 2: Run tests**

Run: `cargo test signal::`
Expected: 14 passed.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add src/signal/mod.rs
git commit -m "feat(signal): add SignalHandle public API and spawn_coordinator"
```

---

### Task 9: Rewire the HTTP layer, delete the old rendezvous

This task swaps the app onto the new module. It compiles only when all edits land, so the test cycle is at the end.

**Files:**
- Modify: `src/routes/whep_handler.rs`, `src/routes/whip_handler.rs`, `src/routes/list.rs`, `src/routes/remove.rs`, `src/startup.rs`, `src/main.rs`, `src/utils.rs`, `src/domain/mod.rs`, `src/domain/errors.rs`, `src/domain/session_description.rs`, `src/stream/pipeline.rs`, `Cargo.toml`
- Delete: `src/domain/app_state.rs`, `tests/sdp_exchange.rs`
- Unchanged: `src/routes/options.rs`, `src/routes/mod.rs`, `src/stream/gst_pipeline.rs`

- [ ] **Step 1: Replace the route handlers**

`src/routes/whep_handler.rs` — full new contents:

```rust
use crate::domain::SessionDescription;
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};
use uuid::Uuid;

#[tracing::instrument(name = "WHEP", skip(form, signal))]
pub async fn whep_handler(
    form: String,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    if !form.is_empty() {
        return Err(SignalError::InvalidSdp(
            "Empty body expected. Client initialization not supported.".to_string(),
        ));
    }

    let id = Uuid::new_v4().to_string();
    tracing::info!("Creating connection {}", id);

    let offer = signal.create_connection(id.clone()).await?;

    Ok(HttpResponse::Created()
        .append_header(("Location", format!("/channel/{}", id)))
        .content_type("application/sdp")
        .body(offer.as_ref().to_string()))
}

#[tracing::instrument(name = "WHEP PATCH", skip(form, signal))]
pub async fn whep_patch_handler(
    form: String,
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    let sdp = SessionDescription::parse(form).map_err(|e| SignalError::InvalidSdp(e.to_string()))?;
    if sdp.is_sendonly() {
        return Err(SignalError::InvalidSdp(
            "Received a send-only SDP from client; expected recvonly.".to_string(),
        ));
    }

    signal.answer_received(id, sdp).await?;

    Ok(HttpResponse::NoContent().finish())
}
```

`src/routes/whip_handler.rs` — full new contents:

```rust
use crate::domain::SessionDescription;
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};
use uuid::Uuid;

#[tracing::instrument(name = "WHIP SINK", skip(form, signal))]
pub async fn whip_handler(
    form: String,
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let conn_id = path.into_inner();
    let sdp = SessionDescription::parse(form).map_err(|e| SignalError::InvalidSdp(e.to_string()))?;
    if !sdp.is_sendonly() {
        return Err(SignalError::InvalidSdp(
            "Received a recv-only SDP from whipsink; expected sendonly.".to_string(),
        ));
    }

    tracing::info!("Received SDP offer for connection {}", conn_id);
    let answer = signal.offer_received(conn_id.clone(), sdp).await?;

    let resource_id = Uuid::new_v4().to_string();
    Ok(HttpResponse::Created()
        .append_header((
            "Location",
            format!("/whip_sink/{}/{}", conn_id, resource_id),
        ))
        .content_type("application/sdp")
        .body(answer.as_ref().to_string()))
}
```

`src/routes/list.rs` — full new contents (note: the old handler dumped a GraphViz file of the pipeline on every call; that debug side effect goes away with the pipeline access):

```rust
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};

#[tracing::instrument(name = "LIST", skip(signal))]
pub async fn list(signal: web::Data<SignalHandle>) -> Result<HttpResponse, SignalError> {
    let connections = signal.list_connections().await?;
    Ok(HttpResponse::Ok().json(connections))
}
```

`src/routes/remove.rs` — full new contents:

```rust
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};

#[tracing::instrument(name = "REMOVE", skip(signal))]
pub async fn remove_connection(
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    signal.remove_connection(id).await?;
    Ok(HttpResponse::Ok().finish())
}
```

- [ ] **Step 2: Replace startup.rs**

`src/startup.rs` — full new contents (no more generic `T: PipelineBase`; handlers only need the `SignalHandle`):

```rust
use crate::routes::*;
use crate::signal::SignalHandle;
use actix_cors::Cors;
use actix_web::dev::Server;
use actix_web::{guard, web, App, HttpServer};
use std::net::TcpListener;
use tracing_actix_web::TracingLogger;

pub fn run(listener: TcpListener, signal: SignalHandle) -> Result<Server, std::io::Error> {
    let server = HttpServer::new(move || {
        let cors = Cors::permissive();
        App::new()
            .wrap(TracingLogger::default())
            .wrap(cors)
            .route("/list", web::get().to(list))
            .route("/channel", web::post().to(whep_handler))
            .route("/channel", web::route().guard(guard::Options()).to(options))
            .route("/channel/{id}", web::patch().to(whep_patch_handler))
            .route("/channel/{id}", web::delete().to(remove_connection))
            .route("/whip_sink/{id}", web::post().to(whip_handler))
            .app_data(web::Data::new(signal.clone()))
    })
    .listen(listener)?
    .run();

    Ok(server)
}
```

- [ ] **Step 3: Rewire main.rs and PipelineGuard**

`src/main.rs` — full new contents:

```rust
use clap::Parser;
use srt_whep::signal::{spawn_coordinator, CoordinatorConfig};
use srt_whep::startup::run;
use srt_whep::stream::{Args, PipelineBase, SharablePipeline};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use srt_whep::utils::PipelineGuard;
use std::error::Error;
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::signal;
use tokio::task;
use tokio::time::{sleep, Duration};

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();

    let subscriber = get_subscriber("srt_whep".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);

    let pipeline = SharablePipeline::new(args.clone());
    let signal_handle = spawn_coordinator(pipeline.clone(), CoordinatorConfig::default());
    let listener =
        TcpListener::bind(format!("0.0.0.0:{}", args.port)).expect("WHEP port is already in use");
    let should_stop = Arc::new(AtomicBool::new(false));

    let should_stop_clone = should_stop.clone();
    let signal_clone = signal_handle.clone();
    let pipeline_clone = pipeline.clone();

    // Run the pipeline in a separate thread
    let pipeline_thread = task::spawn(async move {
        let mut loops = 0;
        while !should_stop_clone.load(Ordering::Relaxed) {
            tracing::debug!("Looping pipeline: {}", loops);
            loops += 1;

            let mut pipeline_guard =
                PipelineGuard::new(pipeline_clone.clone(), args.clone(), signal_clone.clone());

            if let Err(err) = pipeline_guard.run().await {
                tracing::error!("Pipeline runs into error: {}", err);
            } else {
                tracing::info!("Pipeline reaches EOS. Reset and rerun the pipeline.");
            }

            sleep(Duration::from_secs(1)).await;
        }
    });

    // Start the http server
    run(listener, signal_handle)?.await?;

    signal::ctrl_c().await?;
    tracing::debug!("Received Ctrl-C signal");

    // Manually stop the pipeline thread
    should_stop.store(true, Ordering::Relaxed);
    pipeline.end().await?;
    pipeline_thread.await?;

    Ok(())
}
```

`src/utils.rs` — full new contents:

```rust
use crate::signal::SignalHandle;
use crate::stream::{Args, PipelineBase, SharablePipeline};
use tokio_async_drop::tokio_async_drop;

// Run the pipe and clean up when it finishes
pub struct PipelineGuard {
    pipeline: SharablePipeline,
    args: Args,
    signal: SignalHandle,
}

impl PipelineGuard {
    pub fn new(pipeline: SharablePipeline, args: Args, signal: SignalHandle) -> Self {
        Self {
            pipeline,
            args,
            signal,
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
```

- [ ] **Step 4: Delete the old rendezvous and dead code**

```bash
git rm src/domain/app_state.rs tests/sdp_exchange.rs
```

`src/domain/mod.rs` — full new contents:

```rust
mod errors;
mod session_description;

pub use errors::{error_chain_fmt, MyError};
pub use session_description::{SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
```

In `src/domain/errors.rs`: delete the `SubscribeError` enum, its `Debug` impl, and its `ResponseError` impl (lines 49-73), plus the now-unused `use actix_web::http::StatusCode;` and `use actix_web::ResponseError;` imports. Keep `MyError`, its `Debug` impl, and `error_chain_fmt`. Also delete the now-unused variants `RepeatedConnection`, `ConnectionNotFound`, `EmptyConnection`, `LockTimeout`, `OfferMissing`, and `AnswerMissing` from `MyError` **only if** `cargo check` confirms nothing references them — `LockTimeout` is still used by the pipeline's `lock_err()` (via `timed_locks`), so expect to keep `LockTimeout`, `MissingElement`, `FailedOperation`, and `InvalidSDP` (used by `SessionDescription::parse`).

In `src/domain/session_description.rs`: delete `set_as_active` (lines 38-40) and `set_as_passive` (lines 42-44).

In `src/stream/pipeline.rs`: delete the `DumpPipeline` struct and its `PipelineBase` impl (lines 83-129 of the pre-task file).

In `Cargo.toml`: delete the line `event-listener = "5.4.0"`.

- [ ] **Step 5: Compile, fix leftovers, run all tests**

Run: `cargo check --all-targets`
Expected: clean after removing any leftover imports the compiler names (e.g. `SharableAppState` references, unused `chrono`/`anyhow::Context` imports in handlers).

Run: `cargo test`
Expected: all signal/stream/domain unit tests pass; no integration tests exist right now (deleted this task, rebuilt next task).

Run: `cargo clippy --all-targets`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt
git add -A
git commit -m "refactor: route all signaling through the coordinator actor

Replaces the AppState rendezvous (event-listener + timed locks) with
SignalHandle commands; deletes SubscribeError, DumpPipeline, dead SDP
helpers, and the stale sdp_exchange integration test."
```

---

### Task 10: Integration tests — contract and happy path

**Files:**
- Create: `tests/signaling.rs`

**Interfaces:**
- Consumes: `spawn_coordinator`, `CoordinatorConfig`, `SignalHandle`, `run`, `TestPipeline`, `VALID_WHIP_OFFER`, `VALID_WHEP_ANSWER`.

- [ ] **Step 1: Write the test scaffolding and happy-path test**

Create `tests/signaling.rs`:

```rust
use actix_web::http::StatusCode;
use once_cell::sync::Lazy;
use srt_whep::domain::{VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
use srt_whep::signal::{spawn_coordinator, CoordinatorConfig};
use srt_whep::startup::run;
use srt_whep::stream::TestPipeline;
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::net::TcpListener;
use std::time::Duration;

static TRACING: Lazy<()> = Lazy::new(|| {
    let subscriber = get_subscriber("test".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);
});

/// Comfortable timeouts for functional tests: long enough that a slow CI
/// machine never trips them accidentally.
fn functional_config() -> CoordinatorConfig {
    CoordinatorConfig {
        offer_timeout: Duration::from_secs(5),
        answer_timeout: Duration::from_secs(5),
        watchdog_threshold: 3,
        sweep_interval: Duration::from_millis(50),
    }
}

/// Short timeouts for tests that deliberately let handshakes expire.
fn expiring_config(watchdog_threshold: u32) -> CoordinatorConfig {
    CoordinatorConfig {
        offer_timeout: Duration::from_millis(300),
        answer_timeout: Duration::from_millis(300),
        watchdog_threshold,
        sweep_interval: Duration::from_millis(50),
    }
}

fn spawn_app(config: CoordinatorConfig) -> (String, TestPipeline) {
    Lazy::force(&TRACING);
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let port = listener.local_addr().unwrap().port();
    let pipeline = TestPipeline::default();
    pipeline.set_ready(true);
    let signal = spawn_coordinator(pipeline.clone(), config);
    let server = run(listener, signal).expect("Failed to start server");
    tokio::spawn(server);
    (format!("http://127.0.0.1:{}", port), pipeline)
}

/// In production the coordinator's add_connection points a whipclientsink at
/// /whip_sink/{id}. Tests learn the id the same way: from the pipeline.
async fn wait_for_added_connection(pipeline: &TestPipeline, index: usize) -> String {
    for _ in 0..200 {
        let added = pipeline.snapshot().added;
        if added.len() > index {
            return added[index].clone();
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    panic!("connection {} was never added to the pipeline", index);
}

/// Drives one full WHEP<->WHIP exchange and returns the connection id.
async fn complete_exchange(address: &str, pipeline: &TestPipeline, index: usize) -> String {
    let whep_task = {
        let address = address.to_string();
        tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("{}/channel", address))
                .header("Content-Type", "application/sdp")
                .send()
                .await
                .expect("whep post failed")
        })
    };

    let id = wait_for_added_connection(pipeline, index).await;

    let whip_task = {
        let address = address.to_string();
        let id = id.clone();
        tokio::spawn(async move {
            reqwest::Client::new()
                .post(format!("{}/whip_sink/{}", address, id))
                .header("Content-Type", "application/sdp")
                .body(VALID_WHIP_OFFER)
                .send()
                .await
                .expect("whip post failed")
        })
    };

    let whep_response = whep_task.await.unwrap();
    assert_eq!(StatusCode::CREATED, whep_response.status());
    assert_eq!(
        "application/sdp",
        whep_response.headers()["content-type"].to_str().unwrap()
    );
    let location = whep_response.headers()["Location"]
        .to_str()
        .unwrap()
        .to_string();
    assert_eq!(format!("/channel/{}", id), location);
    let offer = whep_response.text().await.unwrap();
    assert!(offer.contains("a=sendonly"));

    let patch_response = reqwest::Client::new()
        .patch(format!("{}{}", address, location))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHEP_ANSWER)
        .send()
        .await
        .expect("patch failed");
    assert_eq!(StatusCode::NO_CONTENT, patch_response.status());

    let whip_response = whip_task.await.unwrap();
    assert_eq!(StatusCode::CREATED, whip_response.status());
    let answer = whip_response.text().await.unwrap();
    assert!(answer.contains("a=recvonly"));

    id
}

#[tokio::test]
async fn full_sdp_exchange_succeeds() {
    let (address, pipeline) = spawn_app(functional_config());
    complete_exchange(&address, &pipeline, 0).await;
    assert_eq!(0, pipeline.snapshot().quit_count);
}
```

- [ ] **Step 2: Run it**

Run: `cargo test --test signaling`
Expected: 1 passed.

- [ ] **Step 3: Add the contract tests**

Append to `tests/signaling.rs`:

```rust
#[tokio::test]
async fn non_empty_channel_post_is_rejected() {
    let (address, _pipeline) = spawn_app(functional_config());
    let response = reqwest::Client::new()
        .post(format!("{}/channel", address))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHIP_OFFER)
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::BAD_REQUEST, response.status());
}

#[tokio::test]
async fn not_ready_pipeline_returns_503_with_retry_after() {
    let (address, pipeline) = spawn_app(functional_config());
    pipeline.set_ready(false);

    let response = reqwest::Client::new()
        .post(format!("{}/channel", address))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
    assert_eq!("3", response.headers()["Retry-After"].to_str().unwrap());
}

#[tokio::test]
async fn invalid_sdps_are_rejected_with_400() {
    let (address, _pipeline) = spawn_app(functional_config());
    let client = reqwest::Client::new();

    let test_cases = vec![
        ("v=1", "invalid version"),
        ("v=0", "missing the sendonly/recvonly attribute"),
        ("", "empty string"),
        (" ", "whitespace only"),
    ];

    for (invalid_body, description) in test_cases {
        let response = client
            .post(format!("{}/whip_sink/some-id", address))
            .header("Content-Type", "application/sdp")
            .body(invalid_body)
            .send()
            .await
            .unwrap();
        assert_eq!(
            StatusCode::BAD_REQUEST,
            response.status(),
            "expected 400 for {}",
            description
        );
    }
}

#[tokio::test]
async fn unknown_ids_return_404() {
    let (address, _pipeline) = spawn_app(functional_config());
    let client = reqwest::Client::new();

    // Valid offer, but nobody created this connection.
    let response = client
        .post(format!("{}/whip_sink/ghost", address))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHIP_OFFER)
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::NOT_FOUND, response.status());

    let response = client
        .patch(format!("{}/channel/ghost", address))
        .header("Content-Type", "application/sdp")
        .body(VALID_WHEP_ANSWER)
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::NOT_FOUND, response.status());

    let response = client
        .delete(format!("{}/channel/ghost", address))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::NOT_FOUND, response.status());
}

#[tokio::test]
async fn options_reports_cors_and_accept_post() {
    let (address, _pipeline) = spawn_app(functional_config());
    let response = reqwest::Client::new()
        .request(reqwest::Method::OPTIONS, format!("{}/channel", address))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::NO_CONTENT, response.status());
    assert_eq!(
        "application/sdp",
        response.headers()["ACCEPT-POST"].to_str().unwrap()
    );
}
```

- [ ] **Step 4: Run all integration tests**

Run: `cargo test --test signaling`
Expected: 6 passed.

- [ ] **Step 5: Commit**

```bash
cargo fmt
git add tests/signaling.rs
git commit -m "test: add HTTP integration tests for the signaling contract"
```

---

### Task 11: Integration tests — failure isolation, watchdog, lifecycle

**Files:**
- Modify: `tests/signaling.rs`

- [ ] **Step 1: Add the tests**

Append to `tests/signaling.rs`:

```rust
#[tokio::test]
async fn failed_handshake_does_not_affect_the_next_one() {
    let (address, pipeline) = spawn_app(expiring_config(3));

    // First viewer: nothing ever answers the whipsink leg -> offer times out.
    let response = reqwest::Client::new()
        .post(format!("{}/channel", address))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());

    let first_id = wait_for_added_connection(&pipeline, 0).await;
    let snap = pipeline.snapshot();
    assert!(snap.removed.contains(&first_id), "branch not cleaned up");
    assert_eq!(0, snap.quit_count, "single failure must not restart pipeline");

    // Second viewer: full exchange succeeds on the same server.
    complete_exchange(&address, &pipeline, 1).await;
    assert_eq!(0, pipeline.snapshot().quit_count);
}

#[tokio::test]
async fn watchdog_restarts_pipeline_after_consecutive_failures() {
    let (address, pipeline) = spawn_app(expiring_config(2));
    let client = reqwest::Client::new();

    for _ in 0..2 {
        let response = client
            .post(format!("{}/channel", address))
            .send()
            .await
            .unwrap();
        assert_eq!(StatusCode::SERVICE_UNAVAILABLE, response.status());
    }

    // Threshold 2: the second consecutive failure quits the pipeline.
    assert_eq!(1, pipeline.snapshot().quit_count);
}

#[tokio::test]
async fn list_and_delete_manage_the_connection_lifecycle() {
    let (address, pipeline) = spawn_app(functional_config());
    let client = reqwest::Client::new();

    let id = complete_exchange(&address, &pipeline, 0).await;

    // Established connection is listed with its state.
    let list: Vec<serde_json::Value> = client
        .get(format!("{}/list", address))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(1, list.len());
    assert_eq!(id, list[0]["id"]);
    assert_eq!("established", list[0]["state"]);

    // DELETE removes it from the pipeline and the list.
    let response = client
        .delete(format!("{}/channel/{}", address, id))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, response.status());
    assert!(pipeline.snapshot().removed.contains(&id));

    let list: Vec<serde_json::Value> = client
        .get(format!("{}/list", address))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(list.is_empty());
}
```

- [ ] **Step 2: Run the full suite**

Run: `cargo test`
Expected: everything passes — signal unit tests, stream/domain tests, and 9 integration tests.

- [ ] **Step 3: Commit**

```bash
cargo fmt
git add tests/signaling.rs
git commit -m "test: cover failure isolation, watchdog restart, and lifecycle"
```

---

### Task 12: GStreamer-in-the-loop e2e test (`#[ignore]`)

**Files:**
- Create: `tests/e2e_gstreamer.rs`

**Interfaces:**
- Consumes: `SharablePipeline`, `PipelineGuard`, `spawn_coordinator`, `run`, `Args`, `SRTMode`.

- [ ] **Step 1: Write the e2e test**

Create `tests/e2e_gstreamer.rs`:

```rust
//! End-to-end test against a real GStreamer pipeline. Requires GStreamer
//! (with x264enc and srt plugins) to be installed — see README for setup.
//!
//! Run with: cargo test --test e2e_gstreamer -- --ignored --nocapture
//!
//! Scope: the "wedge risk" — proving that repeatedly hot-plugging and
//! removing whipsink branches does not stall the pipeline. Feeding canned
//! SDP answers to a real whipclientsink would trigger DTLS/ICE against a
//! nonexistent peer and can error the pipeline, so handshakes here are
//! driven to offer receipt and then deliberately abandoned. Media playout
//! is verified manually with the WHEP player.
use gst::prelude::*;
use gstreamer as gst;
use srt_whep::signal::{spawn_coordinator, CoordinatorConfig};
use srt_whep::startup::run;
use srt_whep::stream::{Args, PipelineBase, SRTMode, SharablePipeline};
use srt_whep::utils::PipelineGuard;
use std::net::TcpListener;
use std::time::Duration;

const SRT_PORT: u16 = 9911;
const HTTP_PORT: u16 = 8199;

fn start_srt_source() -> gst::Pipeline {
    gst::init().unwrap();
    let pipeline = gst::parse::launch(&format!(
        "videotestsrc is-live=true \
         ! video/x-raw,width=320,height=240,framerate=25/1 \
         ! x264enc tune=zerolatency key-int-max=25 bitrate=500 \
         ! mpegtsmux ! srtsink uri=srt://127.0.0.1:{}?mode=listener wait-for-connection=false",
        SRT_PORT
    ))
    .unwrap()
    .downcast::<gst::Pipeline>()
    .unwrap();
    pipeline.set_state(gst::State::Playing).unwrap();
    pipeline
}

#[tokio::test]
#[ignore]
async fn pipeline_survives_repeated_handshake_failures() {
    let source = start_srt_source();

    let args = Args {
        input_address: format!("127.0.0.1:{}", SRT_PORT),
        output_address: "127.0.0.1:9912".to_string(),
        srt_mode: SRTMode::Caller,
        srt_latency: 100,
        tsdemux_latency: 100,
        run_discoverer: false,
        discoverer_timeout_sec: 5,
        port: HTTP_PORT as u32, // whipclientsink posts back to this port
    };

    let pipeline = SharablePipeline::new(args.clone());
    let config = CoordinatorConfig {
        offer_timeout: Duration::from_secs(10),
        answer_timeout: Duration::from_secs(3),
        watchdog_threshold: 10, // deliberate failures below must not trip it
        sweep_interval: Duration::from_millis(200),
    };
    let signal = spawn_coordinator(pipeline.clone(), config);

    let listener = TcpListener::bind(format!("127.0.0.1:{}", HTTP_PORT))
        .expect("e2e HTTP port in use");
    let server = run(listener, signal.clone()).unwrap();
    tokio::spawn(server);

    // Supervise the pipeline exactly like main.rs does.
    let pipeline_clone = pipeline.clone();
    let args_clone = args.clone();
    let signal_clone = signal.clone();
    tokio::spawn(async move {
        loop {
            let mut guard =
                PipelineGuard::new(pipeline_clone.clone(), args_clone.clone(), signal_clone.clone());
            if let Err(e) = guard.run().await {
                eprintln!("pipeline error: {}", e);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });

    // Wait for the SRT input to be demuxed.
    let mut ready = false;
    for _ in 0..100 {
        if pipeline.ready().await.unwrap_or(false) {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        ready,
        "pipeline never became ready — is GStreamer installed and port {} free?",
        SRT_PORT
    );

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", HTTP_PORT);

    // Three cycles: receive a real offer from a real whipclientsink, then
    // abandon the handshake. Branch cleanup must not wedge the pipeline.
    for round in 0..3 {
        let response = client
            .post(format!("{}/channel", base))
            .header("Content-Type", "application/sdp")
            .send()
            .await
            .expect("POST /channel failed");
        assert_eq!(
            201,
            response.status().as_u16(),
            "round {}: no offer received",
            round
        );
        let offer = response.text().await.unwrap();
        assert!(offer.starts_with("v=0"), "round {}: not an SDP offer", round);

        // No PATCH: the answer times out (3s) and the branch is removed.
        tokio::time::sleep(Duration::from_secs(4)).await;
    }

    // After the add/remove cycles the pipeline must still hand out offers.
    let response = client
        .post(format!("{}/channel", base))
        .header("Content-Type", "application/sdp")
        .send()
        .await
        .expect("final POST /channel failed");
    assert_eq!(
        201,
        response.status().as_u16(),
        "pipeline wedged after failure cycles"
    );

    source.set_state(gst::State::Null).unwrap();
}
```

- [ ] **Step 2: Verify it compiles and stays out of the default suite**

Run: `cargo test --test e2e_gstreamer`
Expected: `1 ignored`, 0 failed.

- [ ] **Step 3: Run it for real (requires GStreamer, ~1 min)**

Run: `cargo test --test e2e_gstreamer -- --ignored --nocapture`
Expected: 1 passed. If `x264enc` is missing, install `gstreamer1.0-plugins-ugly` (Linux) or the full GStreamer framework (macOS) — see README.

- [ ] **Step 4: Commit**

```bash
cargo fmt
git add tests/e2e_gstreamer.rs
git commit -m "test: add ignored GStreamer e2e test for branch add/remove stability"
```

---

## Verification after all tasks

1. `cargo test` — full green.
2. `cargo test --test e2e_gstreamer -- --ignored` — green on a machine with GStreamer.
3. Manual smoke test (per README): `docker run --rm -p 1234:1234/udp eyevinntechnology/testsrc`, then `cargo run --release -- -i 127.0.0.1:1234 -o 127.0.0.1:8888 -p 8000 -s caller | bunyan`, open the WHEP player at `https://webrtc.player.eyevinn.technology/?type=whep` against `http://localhost:8000/channel` — video plays; kill and reload the player repeatedly — other sessions keep playing.
