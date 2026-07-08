# Signal-Plane Legibility & Config Hygiene Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the signaling connection state machine explicit, unify the pipeline-readiness contract across the real and fake pipelines, and close the loopback-WHIP port and error-layer hygiene gaps — all behavior-preserving except where a step explicitly notes an API-message change.

**Architecture:** An SRT→WHEP gateway. A single coordinator actor (`src/signal`) owns connection state and is the sole caller of pipeline branch add/remove across the `BranchControl` trait seam. A supervisor drives the `PipelineLifecycle` seam. Both talk to one `SharablePipeline` (`src/stream`) guarded by a 1s timed lock. HTTP handlers (`src/routes`) are thin adapters over a `SignalHandle` (mpsc + oneshot). `src/stream` must never depend on `src/signal`.

**Tech Stack:** Rust 2021 (rust-version 1.84), actix-web 4, tokio (multi-thread), GStreamer 0.23 bindings, thiserror 2, clap 4, async-trait, timed-locks.

**Source of truth for rationale:** `docs/proposals/2026-07-08-signal-plane-and-config-hygiene.md`. This plan is the executable form of that proposal.

## Global Constraints

- **Never implement on `main`.** Work in an isolated git worktree/branch created via `superpowers:using-git-worktrees` before Task 1.
- **Layering rule (do not violate):** `src/stream` must not import `src/signal`. `CoordinatorConfig` lives in `signal`; `Args` lives in `stream`. Anything that needs both is composed in `src/main.rs` (the crate root binary) or in a crate-root module — never by `stream` importing `signal`.
- **Naming:** keep the three deliberate names — *channel* (HTTP) / *connection* (signal) / *branch* (stream). Do not introduce a fourth.
- **Wire format is frozen:** HTTP paths, methods, status codes, `Retry-After: 3`, the `GET /list` array-of-`{id,state}` shape, and `Location` headers must not change. SDP request/response *bodies* are unchanged strings.
- **GStreamer env is required for `cargo test`/`build` on this Mac** (else SIGABRT: `libgstpbutils` not loaded). Source this block in every shell (and every subagent dispatch) that builds or tests:
  ```sh
  export PKG_CONFIG_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib/pkgconfig
  export PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/bin:$PATH
  export GST_PLUGIN_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
  export DYLD_LIBRARY_PATH=$DYLD_LIBRARY_PATH:$GST_PLUGIN_PATH
  export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
  ```
  Do **not** prepend a local `gst-plugins-rs` build (it shadows the framework's stable `rswebrtc`).
- **Test gate per commit:** `cargo test` (lib unit tests + `tests/signaling.rs` integration) stays green. Baseline before Task 1: ~23 lib + ~10 integration tests pass; `tests/e2e_gstreamer.rs` stays `#[ignore]`d.
- **The e2e (`tests/e2e_gstreamer.rs`) is the only coverage for real GStreamer code.** Only Task 9 touches real-element control flow; it is verified by one isolated manual e2e run (recipe in Task 9). Task 13 changes e2e wiring; re-run once after Task 13.
- **Commit style:** end messages with the repo's `Co-Authored-By`/`Claude-Session` trailer. One commit per task; each leaves the tree compiling and green.

---

## Task 1: Relocate `error_chain_fmt` to a crate-level utility

**Files:**
- Create: `src/errors.rs`
- Modify: `src/lib.rs` (add `mod errors;`)
- Modify: `src/domain/errors.rs` (remove the helper, import it)
- Modify: `src/domain/mod.rs` (drop the `pub(crate) use errors::error_chain_fmt;` re-export)
- Modify: `src/stream/errors.rs:1` (import from the new location)

**Interfaces:**
- Produces: `pub(crate) fn crate::errors::error_chain_fmt(e: &impl std::error::Error, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result` — a straight move; identical signature and body.

- [ ] **Step 1: Create the crate-level module.** Write `src/errors.rs`:

```rust
//! Cross-cutting error utilities shared by every layer's error type.

/// Format an error together with its `source()` chain, e.g. for a bespoke
/// `Debug` impl that reports the whole causal chain rather than one line.
pub(crate) fn error_chain_fmt(
    e: &impl std::error::Error,
    f: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    writeln!(f, "{}\n", e)?;
    let mut current = e.source();
    while let Some(cause) = current {
        writeln!(f, "Caused by:\n\t{}", cause)?;
        current = cause.source();
    }
    Ok(())
}
```

- [ ] **Step 2: Register the module.** In `src/lib.rs`, add `pub(crate) mod errors;` (place it before `pub mod domain;` to keep alphabetical-ish grouping):

```rust
pub(crate) mod errors;
pub mod domain;
pub mod routes;
pub mod signal;
pub mod startup;
pub mod stream;
pub(crate) mod supervisor;
pub mod telemetry;
```

- [ ] **Step 3: Strip the helper from the domain and import it.** In `src/domain/errors.rs`, delete the `pub(crate) fn error_chain_fmt(...)` definition (the whole block after the `Debug` impl) and add an import at the top so the `Debug` impl still resolves it:

```rust
use crate::errors::error_chain_fmt;
use std::fmt::Debug;
use thiserror::Error;

/// SDP validation failures — the domain's only error language.
#[derive(Error)]
pub enum SdpError {
    #[error("Invalid SDP: {0}")]
    InvalidSdp(String),
}

// Bespoke `Debug` to report the error source chain.
impl Debug for SdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        error_chain_fmt(self, f)
    }
}
```

- [ ] **Step 4: Drop the domain re-export.** In `src/domain/mod.rs`, remove the line `pub(crate) use errors::error_chain_fmt;`. Result:

```rust
mod errors;
mod session_description;

pub use errors::SdpError;
pub use session_description::{SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};
```

- [ ] **Step 5: Point the stream layer at the new location.** In `src/stream/errors.rs`, change line 1 from `use crate::domain::error_chain_fmt;` to `use crate::errors::error_chain_fmt;`.

- [ ] **Step 6: Verify.** With the GStreamer env sourced:

Run: `cargo test`
Expected: all tests pass; no warnings about an unused `error_chain_fmt` re-export.

- [ ] **Step 7: Commit.**

```bash
git add src/errors.rs src/lib.rs src/domain/errors.rs src/domain/mod.rs src/stream/errors.rs
git commit -m "refactor: move error_chain_fmt to a crate-level errors module"
```

---

## Task 2: Make `SignalError` carry the domain SDP error as a source

**Files:**
- Modify: `src/signal/errors.rs`

**Interfaces:**
- Produces: a new variant `SignalError::Sdp(#[from] SdpError)` (transparent Display, 400). The manual `impl From<SdpError> for SignalError` is deleted (derived by `#[from]`).
- Unchanged: `SignalError::InvalidSdp(String)` stays — the HTTP handlers use it for request-level validation messages ("Empty body expected…", direction mismatches) that are not domain parse failures.

- [ ] **Step 1: Add the source-carrying variant and delete the manual conversion.** In `src/signal/errors.rs`, add the variant and remove the hand-written `From<SdpError>`:

```rust
#[derive(Error, Debug)]
pub enum SignalError {
    #[error("Invalid SDP: {0}")]
    InvalidSdp(String),
    // Parse failures from the domain carry the domain error as their source;
    // its Display is already "Invalid SDP: …", so this does not double-prefix.
    #[error(transparent)]
    Sdp(#[from] SdpError),
    #[error("Connection {0} not found")]
    NotFound(String),
    #[error("Connection {0} is in the wrong state for this operation")]
    WrongState(String),
    #[error("Timed out waiting for the {0}")]
    Timeout(&'static str),
    #[error("Input stream is not ready")]
    NotReady,
    #[error("Pipeline is busy: {0}")]
    PipelineBusy(String),
    #[error("Signaling coordinator is unavailable")]
    Unavailable,
    #[error("Pipeline operation failed: {0}")]
    Pipeline(String),
}
```

Delete this block entirely (its behavior is now the derived `#[from]`):

```rust
impl From<SdpError> for SignalError {
    fn from(e: SdpError) -> Self {
        match e {
            SdpError::InvalidSdp(msg) => SignalError::InvalidSdp(msg),
        }
    }
}
```

- [ ] **Step 2: Map the new variant to 400.** In `status_code`, add `SignalError::Sdp(_)` to the `BAD_REQUEST` arm:

```rust
fn status_code(&self) -> StatusCode {
    match self {
        SignalError::InvalidSdp(_) | SignalError::Sdp(_) => StatusCode::BAD_REQUEST,
        SignalError::NotFound(_) => StatusCode::NOT_FOUND,
        SignalError::WrongState(_) => StatusCode::CONFLICT,
        SignalError::Timeout(_) | SignalError::NotReady | SignalError::PipelineBusy(_) => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        SignalError::Unavailable | SignalError::Pipeline(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}
```

The `error_response` `Retry-After` `matches!` is unchanged (Sdp is not retryable).

- [ ] **Step 3: Verify the existing error tests still hold.** The existing test `sdp_parse_errors_map_without_double_prefix` constructs `SignalError::from(SdpError::InvalidSdp("v=1 unsupported".into()))`, which now yields `SignalError::Sdp(_)`. Its assertions (`status_code() == BAD_REQUEST`, `to_string() == "Invalid SDP: v=1 unsupported"`) hold because `#[error(transparent)]` delegates Display to `SdpError`. No test edit needed. With the env sourced:

Run: `cargo test --lib signal::errors`
Expected: PASS (all four error tests).

- [ ] **Step 4: Verify the whole suite.**

Run: `cargo test`
Expected: green. The handlers' `SessionDescription::parse(...).map_err(SignalError::from)` still compiles (derived `From<SdpError>`).

- [ ] **Step 5: Commit.**

```bash
git add src/signal/errors.rs
git commit -m "refactor: SignalError wraps SdpError as a source instead of copying its message"
```

---

## Task 3: Extract the waiter-failure logic into one method on `ConnectionState`

**Files:**
- Modify: `src/signal/coordinator.rs` (add `fail_waiter`, use it in `remove_connection`, `reap_branch`, `reset_all`; add unit tests)

**Interfaces:**
- Produces: `fn ConnectionState::fail_waiter(self, err: SignalError)` — consuming; sends `Err(err)` to whichever reply waiter is parked (`AwaitingOffer` → whep, `AwaitingAnswer` → whip, `Established` → no-op).

- [ ] **Step 1: Write the failing tests.** In the `#[cfg(test)] mod tests` block of `src/signal/coordinator.rs`, add to the imports `use super::ConnectionState;` and `use tokio::time::Instant;`, then add:

```rust
#[tokio::test]
async fn fail_waiter_notifies_the_awaiting_offer_waiter() {
    let (tx, rx) = oneshot::channel();
    let state = ConnectionState::AwaitingOffer { whep_reply: tx, deadline: Instant::now() };
    state.fail_waiter(SignalError::Unavailable);
    assert!(matches!(rx.await.unwrap(), Err(SignalError::Unavailable)));
}

#[tokio::test]
async fn fail_waiter_notifies_the_awaiting_answer_waiter() {
    let (tx, rx) = oneshot::channel();
    let state = ConnectionState::AwaitingAnswer { whip_reply: tx, deadline: Instant::now() };
    state.fail_waiter(SignalError::NotFound("a".into()));
    assert!(matches!(rx.await.unwrap(), Err(SignalError::NotFound(_))));
}
```

The `Established` no-op branch is intentionally not unit-tested here — a
no-op has nothing to assert, and that path is already exercised end-to-end by
`established_connection_is_reaped_on_branch_failure`,
`list_and_delete_manage_the_connection_lifecycle`, and
`reset_fails_all_waiters_and_clears_state`.

- [ ] **Step 2: Run the tests to confirm they fail to compile.**

Run: `cargo test --lib signal::coordinator::tests::fail_waiter`
Expected: FAIL — `no method named fail_waiter`.

- [ ] **Step 3: Add the method.** In the `impl ConnectionState` block (next to `name`):

```rust
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
```

Add `use super::errors::SignalError;` only if not already in scope at the top of the file — it is (`use super::errors::SignalError;` is already imported).

- [ ] **Step 4: Replace the three duplicated blocks.**

In `remove_connection`, the `Ok(())` arm becomes:

```rust
Ok(()) => {
    // The connection is really gone now; let any pending waiter learn it.
    state.fail_waiter(SignalError::NotFound(id.clone()));
    let _ = reply.send(Ok(()));
}
```

In `reap_branch`, replace the `match state { … }` block with:

```rust
state.fail_waiter(SignalError::NotFound(id.clone()));
if let Err(e) = self.remove_branch_bounded(id.clone()).await {
    tracing::error!("Failed to remove branch for {}: {}", id, e);
}
```

In `reset_all`, the loop body becomes:

```rust
fn reset_all(&mut self) {
    for (_, state) in self.connections.drain() {
        state.fail_waiter(SignalError::Unavailable);
    }
}
```

- [ ] **Step 5: Run the tests.**

Run: `cargo test`
Expected: PASS — the three new `fail_waiter` tests plus all ~20 existing actor tests (`reset_fails_all_waiters_and_clears_state`, `failed_delete_keeps_the_connection_retryable`, `established_connection_is_reaped_on_branch_failure`, etc.).

- [ ] **Step 6: Commit.**

```bash
git add src/signal/coordinator.rs
git commit -m "refactor: collapse the triplicated waiter-failure logic into ConnectionState::fail_waiter"
```

---

## Task 4: Move the offer transition onto `ConnectionState`

**Files:**
- Modify: `src/signal/coordinator.rs` (`OfferDelivery` enum, `deliver_offer` method, rewrite `offer_received` handler)

**Interfaces:**
- Produces: `enum OfferDelivery { Delivered, WaiterGone }` and `fn ConnectionState::deliver_offer(self, sdp: SessionDescription) -> Result<OfferDelivery, ConnectionState>`. `Ok(_)` means the offer arrived in the legal `AwaitingOffer` state (the variant says whether the parked WHEP waiter was still there); `Err(self)` returns the unchanged state so the handler can restore it and reject with `WrongState`.

- [ ] **Step 1: Add the outcome enum and the method.** Above (or below) the `impl ConnectionState` block in `src/signal/coordinator.rs`:

```rust
/// Outcome of delivering the whipsink's SDP offer to the parked WHEP waiter.
enum OfferDelivery {
    /// The WHEP client received the offer; advance to awaiting the answer.
    Delivered,
    /// The WHEP client had vanished; the handshake must be failed.
    WaiterGone,
}
```

In `impl ConnectionState`:

```rust
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
```

- [ ] **Step 2: Rewrite the handler to delegate.** Replace the body of `async fn offer_received`:

```rust
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
                ConnectionState::AwaitingAnswer { whip_reply: reply, deadline },
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
```

- [ ] **Step 3: Verify behavior is preserved by the existing suite.** No new test — `happy_path_create_offer_answer`, `unknown_id_is_not_found_for_every_command`, `wrong_state_commands_are_rejected_without_corruption`, and `abandoned_whep_client_is_reaped_by_the_sweep` already pin every branch of this handler. With the env sourced:

Run: `cargo test`
Expected: green.

- [ ] **Step 4: Commit.**

```bash
git add src/signal/coordinator.rs
git commit -m "refactor: move the offer transition into ConnectionState::deliver_offer"
```

---

## Task 5: Move the answer transition onto `ConnectionState`

**Files:**
- Modify: `src/signal/coordinator.rs` (`AnswerDelivery` enum, `deliver_answer` method, rewrite `answer_received` handler)

**Interfaces:**
- Produces: `enum AnswerDelivery { Established, WaiterGone }` and `fn ConnectionState::deliver_answer(self, sdp: SessionDescription) -> Result<AnswerDelivery, ConnectionState>`. The coordinator-level side effect (`watchdog.record_success`) stays in the handler.

- [ ] **Step 1: Add the outcome enum and the method.**

```rust
/// Outcome of delivering the browser's SDP answer to the parked WHIP waiter.
enum AnswerDelivery {
    /// The whipsink received the answer; the connection is established.
    Established,
    /// The whipsink's request had died; the handshake must be failed.
    WaiterGone,
}
```

In `impl ConnectionState`:

```rust
/// Deliver the browser's SDP answer to the parked WHIP waiter.
/// `Ok(..)` means this was the legal `AwaitingAnswer` state; the variant
/// reports whether the waiter was still there. `Err(self)` returns the
/// unchanged state for the caller to restore and reject.
fn deliver_answer(self, sdp: SessionDescription) -> Result<AnswerDelivery, ConnectionState> {
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
```

- [ ] **Step 2: Rewrite the handler.** Replace the body of `async fn answer_received`:

```rust
async fn answer_received(&mut self, id: ConnectionId, sdp: SessionDescription, reply: UnitReply) {
    let Some(state) = self.connections.remove(&id) else {
        let _ = reply.send(Err(SignalError::NotFound(id)));
        return;
    };
    match state.deliver_answer(sdp) {
        Ok(AnswerDelivery::Established) => {
            self.watchdog.record_success();
            self.connections.insert(
                id,
                ConnectionState::Established { since: Instant::now() },
            );
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
```

- [ ] **Step 3: Verify.** Existing tests (`happy_path…`, `wrong_state…`, `watchdog_*`, `success_between_failures_prevents_the_trip`) cover every arm. With the env sourced:

Run: `cargo test`
Expected: green.

- [ ] **Step 4: Commit.**

```bash
git add src/signal/coordinator.rs
git commit -m "refactor: move the answer transition into ConnectionState::deliver_answer"
```

---

## Task 6: Add state constructors and a transition-table test

**Files:**
- Modify: `src/signal/coordinator.rs` (constructors, use them in handlers, add the table test)

**Interfaces:**
- Produces: `fn ConnectionState::awaiting_offer(whep_reply, deadline)`, `awaiting_answer(whip_reply, deadline)`, `established(since)` — the only constructors for each variant. After this task, `impl ConnectionState` is the whole legal-transition table.

- [ ] **Step 1: Add the constructors.** In `impl ConnectionState`:

```rust
fn awaiting_offer(whep_reply: SdpReply, deadline: Instant) -> Self {
    ConnectionState::AwaitingOffer { whep_reply, deadline }
}

fn awaiting_answer(whip_reply: SdpReply, deadline: Instant) -> Self {
    ConnectionState::AwaitingAnswer { whip_reply, deadline }
}

fn established(since: Instant) -> Self {
    ConnectionState::Established { since }
}
```

- [ ] **Step 2: Use them at the three construction sites.**
  - In `create_connection`, replace the `ConnectionState::AwaitingOffer { whep_reply: reply, deadline }` literal with `ConnectionState::awaiting_offer(reply, deadline)`.
  - In `offer_received` (Delivered arm), replace `ConnectionState::AwaitingAnswer { whip_reply: reply, deadline }` with `ConnectionState::awaiting_answer(reply, deadline)`.
  - In `answer_received` (Established arm), replace `ConnectionState::Established { since: Instant::now() }` with `ConnectionState::established(Instant::now())`.

- [ ] **Step 3: Write the transition-table test.** In `mod tests`:

```rust
#[tokio::test]
async fn transition_table_accepts_only_legal_events() {
    use super::{AnswerDelivery, OfferDelivery};

    // AwaitingOffer: an offer is legal, an answer is not.
    let (tx, _rx) = oneshot::channel();
    let s = ConnectionState::awaiting_offer(tx, Instant::now());
    assert!(matches!(s.deliver_offer(offer()), Ok(OfferDelivery::Delivered)));

    let (tx, _rx) = oneshot::channel();
    let s = ConnectionState::awaiting_offer(tx, Instant::now());
    assert!(matches!(
        s.deliver_answer(answer()),
        Err(ConnectionState::AwaitingOffer { .. })
    ));

    // AwaitingAnswer: an answer is legal, an offer is not.
    let (tx, _rx) = oneshot::channel();
    let s = ConnectionState::awaiting_answer(tx, Instant::now());
    assert!(matches!(s.deliver_answer(answer()), Ok(AnswerDelivery::Established)));

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
```

Note: each waiter case binds `_rx` (not `_`) so the receiver stays alive and the `send` succeeds — a bare `_` would drop it and make the offer/answer report `WaiterGone`.

- [ ] **Step 4: Run the tests.**

Run: `cargo test`
Expected: PASS including `transition_table_accepts_only_legal_events`.

- [ ] **Step 5: Commit.**

```bash
git add src/signal/coordinator.rs
git commit -m "refactor: add ConnectionState constructors and a transition-table test"
```

---

## Task 7: Introduce `SdpOffer` / `SdpAnswer` direction newtypes

**Files:**
- Modify: `src/domain/session_description.rs` (add the two newtypes + tests)
- Modify: `src/domain/mod.rs` (export them)

**Interfaces:**
- Produces:
  - `SdpOffer::parse(String) -> Result<SdpOffer, SdpError>` (requires `a=sendonly`), `SdpOffer::as_ref() -> &str`, `SdpOffer::is_sendonly() -> bool` (always true), `Display`.
  - `SdpAnswer::parse(String) -> Result<SdpAnswer, SdpError>` (requires not-sendonly, i.e. recvonly), `SdpAnswer::as_ref() -> &str`, `SdpAnswer::is_sendonly() -> bool` (always false), `Display`.
- Not consumed anywhere yet (Task 8 threads them through).

- [ ] **Step 1: Write the failing tests.** In the `#[cfg(test)] mod tests` of `src/domain/session_description.rs`, extend the imports to `use super::{SdpAnswer, SdpOffer, SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};` and add:

```rust
#[test]
fn offer_requires_sendonly() {
    assert_ok!(SdpOffer::parse(VALID_WHIP_OFFER.to_string()));
    // A recvonly answer is not a valid offer.
    assert_err!(SdpOffer::parse(VALID_WHEP_ANSWER.to_string()));
}

#[test]
fn answer_requires_recvonly() {
    assert_ok!(SdpAnswer::parse(VALID_WHEP_ANSWER.to_string()));
    // A sendonly offer is not a valid answer.
    assert_err!(SdpAnswer::parse(VALID_WHIP_OFFER.to_string()));
}

#[test]
fn direction_newtypes_reject_malformed_sdp() {
    assert_err!(SdpOffer::parse("v=1".to_string()));
    assert_err!(SdpAnswer::parse("".to_string()));
}
```

- [ ] **Step 2: Run to confirm failure.**

Run: `cargo test --lib domain::session_description`
Expected: FAIL — `SdpOffer`/`SdpAnswer` not found.

- [ ] **Step 3: Add the newtypes.** After the `impl Display for SessionDescription` block (before the `VALID_WHIP_OFFER` const):

```rust
/// A WHIP/WHEP **offer**: an SDP proven to advertise `a=sendonly`. Distinct
/// from [`SdpAnswer`] so an offer and an answer can never be swapped by type.
#[derive(Debug, Clone)]
pub struct SdpOffer(SessionDescription);

impl SdpOffer {
    /// Validate `s` as an SDP and require the sendonly (offer) direction.
    pub fn parse(s: String) -> Result<SdpOffer, SdpError> {
        let sdp = SessionDescription::parse(s)?;
        if !sdp.is_sendonly() {
            return Err(SdpError::InvalidSdp(
                "expected a sendonly offer, got a recvonly SDP".to_string(),
            ));
        }
        Ok(SdpOffer(sdp))
    }

    /// Always `true` — an offer is sendonly by construction.
    pub fn is_sendonly(&self) -> bool {
        self.0.is_sendonly()
    }
}

impl AsRef<str> for SdpOffer {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl Display for SdpOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A WHEP **answer**: an SDP proven to advertise the recvonly direction.
#[derive(Debug, Clone)]
pub struct SdpAnswer(SessionDescription);

impl SdpAnswer {
    /// Validate `s` as an SDP and require the recvonly (answer) direction.
    pub fn parse(s: String) -> Result<SdpAnswer, SdpError> {
        let sdp = SessionDescription::parse(s)?;
        if sdp.is_sendonly() {
            return Err(SdpError::InvalidSdp(
                "expected a recvonly answer, got a sendonly SDP".to_string(),
            ));
        }
        Ok(SdpAnswer(sdp))
    }

    /// Always `false` — an answer is recvonly by construction.
    pub fn is_sendonly(&self) -> bool {
        self.0.is_sendonly()
    }
}

impl AsRef<str> for SdpAnswer {
    fn as_ref(&self) -> &str {
        self.0.as_ref()
    }
}

impl Display for SdpAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
```

- [ ] **Step 4: Export them.** In `src/domain/mod.rs`:

```rust
pub use session_description::{
    SdpAnswer, SdpOffer, SessionDescription, VALID_WHEP_ANSWER, VALID_WHIP_OFFER,
};
```

- [ ] **Step 5: Run the tests.**

Run: `cargo test --lib domain`
Expected: PASS (the three new tests plus the existing five).

- [ ] **Step 6: Verify the whole suite still builds (newtypes are unused so far → allow the dead-code they may trigger only if the compiler complains; they are `pub` so it should not).**

Run: `cargo test`
Expected: green, no dead-code warnings (the types are `pub`).

- [ ] **Step 7: Commit.**

```bash
git add src/domain/session_description.rs src/domain/mod.rs
git commit -m "feat(domain): add SdpOffer/SdpAnswer direction newtypes"
```

---

## Task 8: Thread the direction newtypes through the signaling plane

**Files:**
- Modify: `src/signal/messages.rs` (split the reply alias; type the payloads)
- Modify: `src/signal/coordinator.rs` (state fields, method signatures, test helpers)
- Modify: `src/signal/mod.rs` (`SignalHandle` method signatures; the mod test)
- Modify: `src/routes/whip_handler.rs`, `src/routes/whep_handler.rs` (parse via the newtypes)

**Interfaces:**
- Produces:
  - `messages.rs`: `pub type OfferReply = oneshot::Sender<Result<SdpOffer, SignalError>>;`, `pub type AnswerReply = oneshot::Sender<Result<SdpAnswer, SignalError>>;`, `pub type UnitReply = oneshot::Sender<Result<(), SignalError>>;`. `Command::CreateConnection{ id, reply: OfferReply }`, `Command::OfferReceived{ id, sdp: SdpOffer, reply: AnswerReply }`, `Command::AnswerReceived{ id, sdp: SdpAnswer, reply: UnitReply }`.
  - `SignalHandle::create_connection(id) -> Result<SdpOffer, SignalError>`, `offer_received(id, SdpOffer) -> Result<SdpAnswer, SignalError>`, `answer_received(id, SdpAnswer) -> Result<(), SignalError>`.
- Wire format (HTTP bodies) unchanged: `as_ref()` still yields the same SDP string.

- [ ] **Step 1: Retype the message aliases and payloads.** In `src/signal/messages.rs`:

```rust
use super::errors::SignalError;
use crate::domain::{SdpAnswer, SdpOffer};
use serde::Serialize;
use tokio::sync::oneshot;

pub type ConnectionId = String;
pub type OfferReply = oneshot::Sender<Result<SdpOffer, SignalError>>;
pub type AnswerReply = oneshot::Sender<Result<SdpAnswer, SignalError>>;
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
    CreateConnection { id: ConnectionId, reply: OfferReply },
    /// Loopback WHIP POST: the whipsink's offer; reply carries the SDP answer
    /// once the browser PATCHes it (or an error on timeout/failure).
    OfferReceived {
        id: ConnectionId,
        sdp: SdpOffer,
        reply: AnswerReply,
    },
    /// WHEP PATCH: the browser's answer; replied to immediately.
    AnswerReceived {
        id: ConnectionId,
        sdp: SdpAnswer,
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

- [ ] **Step 2: Update the coordinator types.** In `src/signal/coordinator.rs`:
  - Change the imports: `use super::messages::{AnswerReply, Command, ConnectionId, ConnectionInfo, OfferReply, UnitReply};` and add `use crate::domain::{SdpAnswer, SdpOffer};` (keep `SessionDescription` import only if still used — after this task it is not; remove it).
  - `ConnectionState::AwaitingOffer` field `whep_reply: OfferReply`; `AwaitingAnswer` field `whip_reply: AnswerReply`.
  - Constructors: `awaiting_offer(whep_reply: OfferReply, deadline)`, `awaiting_answer(whip_reply: AnswerReply, deadline)`.
  - `deliver_offer(self, sdp: SdpOffer) -> Result<OfferDelivery, ConnectionState>`.
  - `deliver_answer(self, sdp: SdpAnswer) -> Result<AnswerDelivery, ConnectionState>`.
  - Handler signatures: `create_connection(&mut self, id: ConnectionId, reply: OfferReply)`, `offer_received(&mut self, id, sdp: SdpOffer, reply: AnswerReply)`, `answer_received(&mut self, id, sdp: SdpAnswer, reply: UnitReply)`.
  - `fail_waiter` is unchanged — each arm sends `Err(err)` into its typed channel, which compiles for both `OfferReply` and `AnswerReply`.

- [ ] **Step 3: Update `SignalHandle`.** In `src/signal/mod.rs`, change the imports to `use crate::domain::{SdpAnswer, SdpOffer};` (drop `SessionDescription` if now unused) and retype the three methods:

```rust
pub async fn create_connection(&self, id: String) -> Result<SdpOffer, SignalError> {
    self.request(|reply| Command::CreateConnection { id, reply }).await
}

pub async fn offer_received(&self, id: String, sdp: SdpOffer) -> Result<SdpAnswer, SignalError> {
    self.request(|reply| Command::OfferReceived { id, sdp, reply }).await
}

pub async fn answer_received(&self, id: String, sdp: SdpAnswer) -> Result<(), SignalError> {
    self.request(|reply| Command::AnswerReceived { id, sdp, reply }).await
}
```

The generic `request<T>` helper is unchanged (`T` resolves to `SdpOffer` / `SdpAnswer` / `()`).

- [ ] **Step 4: Update the HTTP handlers.** In `src/routes/whip_handler.rs`, parse an offer and drop the inline direction check (now enforced by `SdpOffer::parse`):

```rust
use crate::domain::SdpOffer;
use crate::signal::{SignalError, SignalHandle};
use crate::stream::whip_sink_path;
use actix_web::{web, HttpResponse};

#[tracing::instrument(name = "WHIP SINK", skip(form, signal))]
pub async fn whip_handler(
    form: String,
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let conn_id = path.into_inner();
    let sdp = SdpOffer::parse(form).map_err(SignalError::from)?;

    tracing::info!("Received SDP offer for connection {}", conn_id);
    let answer = signal.offer_received(conn_id.clone(), sdp).await?;

    Ok(HttpResponse::Created()
        .append_header(("Location", whip_sink_path(&conn_id)))
        .content_type("application/sdp")
        .body(answer.as_ref().to_string()))
}
```

In `src/routes/whep_handler.rs`, `whep_patch_handler` parses an answer via `SdpAnswer::parse` (which enforces recvonly), dropping the inline `is_sendonly` check; `whep_handler` (POST) is unchanged except the returned `offer` is now an `SdpOffer` (its `as_ref()` is identical):

```rust
use crate::domain::SdpAnswer;
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};
use uuid::Uuid;

// whep_handler (POST /channel) body is unchanged; `offer` is now SdpOffer and
// `offer.as_ref().to_string()` yields the same SDP string as before.

#[tracing::instrument(name = "WHEP PATCH", skip(form, signal))]
pub async fn whep_patch_handler(
    form: String,
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    let sdp = SdpAnswer::parse(form).map_err(SignalError::from)?;

    signal.answer_received(id, sdp).await?;

    Ok(HttpResponse::NoContent().finish())
}
```

Keep `whep_handler`'s existing empty-body check (`SignalError::InvalidSdp("Empty body expected…")`) exactly as is.

- [ ] **Step 5: Update the unit-test helpers.** In `src/signal/coordinator.rs` `mod tests`, change the two helpers (all ~20 `Command::OfferReceived{ sdp: offer() }` / `AnswerReceived{ sdp: answer() }` sends then compile unchanged):

```rust
pub(super) fn offer() -> SdpOffer {
    SdpOffer::parse(VALID_WHIP_OFFER.to_string()).unwrap()
}

pub(super) fn answer() -> SdpAnswer {
    SdpAnswer::parse(VALID_WHEP_ANSWER.to_string()).unwrap()
}
```

Update that module's `use` to `use crate::domain::{SdpAnswer, SdpOffer, VALID_WHEP_ANSWER, VALID_WHIP_OFFER};` (drop `SessionDescription`). The assertions `delivered.is_sendonly()` / `!delivered.is_sendonly()` still compile — `SdpOffer`/`SdpAnswer` both expose `is_sendonly()`.

In `src/signal/mod.rs` `mod tests`, change `handle_drives_a_full_handshake`: replace `SessionDescription::parse(VALID_WHIP_OFFER…)` with `SdpOffer::parse(VALID_WHIP_OFFER…)` and `SessionDescription::parse(VALID_WHEP_ANSWER…)` with `SdpAnswer::parse(VALID_WHEP_ANSWER…)`; the `.is_sendonly()` result assertions are unchanged; update its `use` accordingly.

- [ ] **Step 6: Run the full suite.**

Run: `cargo test`
Expected: green. `tests/signaling.rs` is untouched (it drives raw HTTP bodies); the 400/404/409/503 assertions hold because `SdpOffer::parse`/`SdpAnswer::parse` return `SdpError` → `SignalError::Sdp` → 400, and direction mismatches that used to be handler `InvalidSdp` 400s are now `Sdp` 400s (same status).

- [ ] **Step 7: Commit.**

```bash
git add src/signal src/routes/whip_handler.rs src/routes/whep_handler.rs
git commit -m "refactor(signal): thread SdpOffer/SdpAnswer through the signaling plane"
```

---

## Task 9: Make the real `add_branch` check readiness atomically (single lock)

**Files:**
- Modify: `src/stream/gst_pipeline.rs` (extract `input_ready`, rewrite `ready` and `add_branch`)

**Interfaces:**
- Produces: `fn SharablePipeline::input_ready(pipeline: &gst::Pipeline) -> Result<bool, PipelineError>` — a pure predicate over an already-locked pipeline. `ready()` = lock → `input_ready`. `add_branch()` = one lock → `input_ready` → attach, with no check-to-use window.
- **This is the only task that touches real-GStreamer control flow.** No element logic changes — lock/control-flow only.

- [ ] **Step 1: Extract the readiness predicate.** In `impl SharablePipeline` (an inherent impl; add one if only trait impls exist — put it next to `new`), add:

```rust
impl SharablePipeline {
    /// Whether the input is demuxed and the matching output tees exist, so a
    /// branch can be linked. Pure check over an already-locked pipeline; the
    /// single source of truth for both `ready()` and `add_branch()`.
    fn input_ready(pipeline: &Pipeline) -> Result<bool, PipelineError> {
        let demux = pipeline
            .by_name("demux")
            .ok_or_else(|| PipelineError::Fatal("Failed to find element: demux".to_string()))?;

        let pads = demux.pads();
        let has_video = pads.iter().any(|pad| pad.name().starts_with("video"));
        let has_audio = pads.iter().any(|pad| pad.name().starts_with("audio"));
        if !has_video && !has_audio {
            return Ok(false);
        }

        // The demux exposes its media pads (pad-added) before the output tees
        // are built (no-more-pads -> link_media). A branch links onto those
        // tees, so the input is only truly ready once the matching tee exists.
        let video_ready = !has_video || pipeline.by_name("output_tee_video").is_some();
        let audio_ready = !has_audio || pipeline.by_name("output_tee_audio").is_some();
        Ok(video_ready && audio_ready)
    }
}
```

- [ ] **Step 2: Rewrite `ready()` to lock then delegate.** In the `BranchControl` impl:

```rust
async fn ready(&self) -> Result<bool, PipelineError> {
    let pipeline_state = self.state.lock_err().await.inspect_err(|e| {
        tracing::error!("Failed to lock pipeline: {}", e);
    })?;
    let Some(pipeline) = pipeline_state.pipeline.as_ref() else {
        tracing::error!("Pipeline is not initialized");
        return Ok(false);
    };
    Self::input_ready(pipeline)
}
```

- [ ] **Step 3: Rewrite `add_branch()` to check-and-attach under one lock.**

```rust
async fn add_branch(&self, id: String) -> Result<(), PipelineError> {
    // One lock acquisition: check readiness and attach under the same guard,
    // so a supervisor restart cannot slip between a separate ready() check
    // and the attach.
    let pipeline_state = self.state.lock_err().await?;
    // No pipeline means we are between supervisor restarts: retryable.
    let pipeline = pipeline_state
        .pipeline
        .as_ref()
        .ok_or(PipelineError::NotReady)?;

    if !Self::input_ready(pipeline)? {
        tracing::error!("Demux has no pad available. No connection can be added.");
        return Err(PipelineError::NotReady);
    }

    tracing::debug!("Add connection {} to pipeline", id);
    Branch::for_id(&id)
        .attach(pipeline, pipeline_state.args.port)
        .map_err(|e| PipelineError::Fatal(e.to_string()))
}
```

Behavior is identical: a `None` pipeline or an un-demuxed input still yields `NotReady`; a missing `demux` still yields `Fatal`; a successful attach is unchanged. The only difference is one lock acquisition instead of two and no TOCTOU window.

- [ ] **Step 4: Build and run the non-gst suite.**

Run: `cargo test`
Expected: green (the fake pipeline is unaffected; this task changes only the real pipeline).

- [ ] **Step 5: Manually verify the real pipeline via the e2e (isolated).** With the GStreamer env sourced:

```bash
cargo test --test e2e_gstreamer --no-run
BIN=$(ls -t target/debug/deps/e2e_gstreamer-* | grep -v '\.d$' | head -1)
pkill -9 -f e2e_gstreamer- 2>/dev/null; sleep 1
timeout --signal=KILL 180 "$BIN" --ignored --nocapture --test-threads=1
pkill -9 -f e2e_gstreamer- 2>/dev/null
```

Expected: the test prints `ok` and exits cleanly (three add/abandon cycles, then the pipeline still hands out offers). If it flakes on the live whipclientsink, run once more in isolation.

- [ ] **Step 6: Commit.**

```bash
git add src/stream/gst_pipeline.rs
git commit -m "refactor(stream): check readiness inside add_branch under one lock (no TOCTOU)"
```

---

## Task 10: Give the test fake the same not-ready gate

**Files:**
- Modify: `src/stream/pipeline.rs` (`TestPipeline::add_branch` gate + a unit test)

**Interfaces:**
- `TestPipeline::add_branch` returns `Err(PipelineError::NotReady)` when the fake is not ready, mirroring the real adapter. **Must land before Task 11.**

- [ ] **Step 1: Write the failing test.** In `src/stream/pipeline.rs` `mod tests`, add (extend the `use super::{…}` line to include `PipelineError` — import it as `use crate::stream::errors::PipelineError;`):

```rust
#[tokio::test]
async fn add_branch_on_a_not_ready_fake_is_not_ready() {
    let pipeline = TestPipeline::default(); // ready = false
    assert!(matches!(
        pipeline.add_branch("a".to_string()).await,
        Err(PipelineError::NotReady)
    ));
    assert!(pipeline.snapshot().added.is_empty());
}
```

- [ ] **Step 2: Run to confirm it fails.**

Run: `cargo test --lib stream::pipeline`
Expected: FAIL — the current fake pushes to `added` and returns `Ok`.

- [ ] **Step 3: Add the gate.** In the `BranchControl for TestPipeline` impl, `add_branch`:

```rust
async fn add_branch(&self, id: String) -> Result<(), PipelineError> {
    if let Some(err) = self.add_branch_error.lock().unwrap().take() {
        return Err(err);
    }
    // Mirror the real adapter: a branch cannot be added to a not-ready input.
    if !self.state.lock().unwrap().ready {
        return Err(PipelineError::NotReady);
    }
    self.state.lock().unwrap().added.push(id);
    Ok(())
}
```

The injected-error check stays first so `fail_next_add_branch` tests are unaffected. The existing `test_pipeline_records_calls` sets `ready(true)` before `add_branch`, so it still records.

- [ ] **Step 4: Run the tests.**

Run: `cargo test`
Expected: green, including the new gate test and all coordinator tests (which use `ready_pipeline()` → ready = true).

- [ ] **Step 5: Commit.**

```bash
git add src/stream/pipeline.rs
git commit -m "test(stream): TestPipeline.add_branch honors the not-ready contract like the real pipeline"
```

---

## Task 11: Remove the coordinator's readiness pre-check

**Files:**
- Modify: `src/signal/coordinator.rs` (`create_connection`)

**Interfaces:**
- `create_connection` no longer calls `pipeline.ready()`; it relies on `add_branch`'s internal gate (Task 9/10). The half-attach cleanup is scoped to `Fatal` errors, which are the only ones that can leave a partially-attached branch.

- [ ] **Step 1: Rewrite `create_connection`.** Delete the `match self.pipeline.ready().await { … }` block and scope the cleanup:

```rust
// Entry API can't be held across the pipeline awaits below.
#[allow(clippy::map_entry)]
async fn create_connection(&mut self, id: ConnectionId, reply: OfferReply) {
    if self.connections.contains_key(&id) {
        let _ = reply.send(Err(SignalError::WrongState(id)));
        return;
    }
    if let Err(add_err) = self.pipeline.add_branch(id.clone()).await {
        // Only a Fatal error can leave a half-attached branch (attach ran and
        // failed partway). NotReady/Transient fail before attaching, so there
        // is nothing to detach — and detaching then would be a spurious
        // teardown on a branch that was never added.
        if matches!(add_err, crate::stream::PipelineError::Fatal(_)) {
            if let Err(cleanup_err) = self.remove_branch_bounded(id.clone()).await {
                tracing::error!(
                    "Failed to detach half-attached branch for {}: {}",
                    id,
                    cleanup_err
                );
            }
        }
        let _ = reply.send(Err(add_err.into()));
        return;
    }
    let deadline = Instant::now() + self.config.offer_timeout;
    self.connections
        .insert(id, ConnectionState::awaiting_offer(reply, deadline));
}
```

(If `crate::stream::PipelineError` is already imported in this file, use the short name; otherwise the fully-qualified path above avoids adding an import.)

- [ ] **Step 2: Verify the not-ready and failure-cleanup tests hold.**
  - `not_ready_pipeline_rejects_creation` — the fake is not ready → `add_branch` returns `NotReady` (Task 10) → `SignalError::NotReady`, and `added` stays empty. With the `Fatal`-only cleanup guard, no spurious `removed` entry is recorded.
  - `transient_pipeline_failure_stays_retryable` — `fail_next_add_branch(Transient)` → 503 + Retry-After; cleanup is skipped (not `Fatal`), which the test does not assert on.
  - `add_branch_failure_detaches_the_half_attached_branch` — `fail_next_add_branch(Fatal)` → cleanup runs → `removed == ["a"]`.
  - `tests/signaling.rs::not_ready_pipeline_returns_503_with_retry_after` — 503 now originates in `add_branch` and maps identically.

Run: `cargo test`
Expected: green.

- [ ] **Step 3: Commit.**

```bash
git add src/signal/coordinator.rs
git commit -m "refactor(signal): drop the readiness pre-check; add_branch is the single gate"
```

---

## Task 12: Unify port types on `u16`

**Files:**
- Modify: `src/stream/pipeline.rs` (`Args.port: u16`)
- Modify: `src/stream/branch.rs` (`whip_endpoint`, `Branch::attach` take `u16`)
- Modify: `tests/e2e_gstreamer.rs` (drop the `as u32` cast)

**Interfaces:**
- `Args.port: u16`; `whip_endpoint(port: u16, id: &str)`; `Branch::attach(&self, pipeline: &gst::Pipeline, port: u16)`.

- [ ] **Step 1: Change the CLI field.** In `src/stream/pipeline.rs`, `Args.port`:

```rust
/// Port for whep client
#[clap(short, long, default_value_t = 8000)]
pub port: u16,
```

- [ ] **Step 2: Change the branch signatures.** In `src/stream/branch.rs`:
  - `fn whip_endpoint(port: u16, id: &str) -> String` (line ~26).
  - `pub(crate) fn attach(&self, pipeline: &gst::Pipeline, port: u16) -> Result<(), Error>` (line ~78).
  - The `whip_endpoint(8000, "abc")` test literal fits `u16` unchanged.

  `gst_pipeline.rs::add_branch` passes `pipeline_state.args.port` (now `u16`) into `attach` — no edit needed beyond the type flowing through.

- [ ] **Step 3: Fix the e2e struct literal.** In `tests/e2e_gstreamer.rs`, change `port: HTTP_PORT as u32,` to `port: HTTP_PORT,` (`HTTP_PORT` is already `u16`).

- [ ] **Step 4: Build and test.**

Run: `cargo test`
Expected: green. `main.rs`'s `format!("0.0.0.0:{}", args.port)` still compiles (Display for `u16`).

- [ ] **Step 5: Commit.**

```bash
git add src/stream/pipeline.rs src/stream/branch.rs tests/e2e_gstreamer.rs
git commit -m "refactor: use u16 for the WHEP/WHIP port end to end"
```

---

## Task 13: Assert listener/pipeline port agreement at assembly

**Files:**
- Modify: `src/startup.rs` (`assemble` gains `expected_whip_port: Option<u16>`)
- Modify: `src/main.rs` (pass `Some(args.port)`)
- Modify: `tests/signaling.rs` (fake pipeline → pass `None`; add a mismatch test)
- Modify: `tests/e2e_gstreamer.rs` (pass `Some(HTTP_PORT)`, drop the alignment comment)

**Interfaces:**
- `Application::assemble<P>(listener, pipeline, config, expected_whip_port: Option<u16>) -> Result<Self, std::io::Error>`. When `Some(port)`, a mismatch with the bound listener port fails assembly with `io::ErrorKind::InvalidInput`.

- [ ] **Step 1: Add the parameter and the check.** In `src/startup.rs`, change `assemble`:

```rust
pub fn assemble<P>(
    listener: TcpListener,
    pipeline: P,
    config: CoordinatorConfig,
    expected_whip_port: Option<u16>,
) -> Result<Self, std::io::Error>
where
    P: BranchControl + PipelineLifecycle + 'static,
{
    let port = listener.local_addr()?.port();
    // The pipeline's whipclientsink posts loopback WHIP offers to a fixed
    // port; if the HTTP server is bound elsewhere those offers 404 silently.
    // Fail loudly at wiring time instead. (This coupling exists only for the
    // loopback WHIP bridge — see src/stream/branch.rs — and is deleted with
    // the whepserversink migration, ADR 0001.)
    if let Some(expected) = expected_whip_port {
        if expected != port {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "HTTP listener bound to port {port} but the pipeline posts \
                     loopback WHIP offers to port {expected}; they must match"
                ),
            ));
        }
    }
    let signal = spawn_coordinator(pipeline.clone(), config);
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let supervisor = Supervisor::spawn(pipeline, signal.clone(), shutdown_rx);
    let server = run(listener, signal.clone())?;
    Ok(Self {
        server,
        supervisor,
        signal,
        shutdown: shutdown_tx,
        port,
    })
}
```

- [ ] **Step 2: Update the production caller.** In `src/main.rs`:

```rust
let app = Application::assemble(listener, pipeline, CoordinatorConfig::default(), Some(args.port))?;
```

- [ ] **Step 3: Update the integration harness and add a mismatch test.** In `tests/signaling.rs`, `spawn_app` passes `None` (the fake has no callback port):

```rust
let app = Application::assemble(listener, pipeline.clone(), config, None)
    .expect("Failed to assemble app");
```

Add a new test:

```rust
#[tokio::test]
async fn assemble_rejects_a_mismatched_whip_port() {
    Lazy::force(&TRACING);
    let listener = TcpListener::bind("127.0.0.1:0").expect("Failed to bind random port");
    let bound = listener.local_addr().unwrap().port();
    let pipeline = TestPipeline::default();
    // Deliberately claim a different callback port than the one bound.
    let wrong = bound.checked_add(1).unwrap_or(bound - 1);
    let result =
        Application::assemble(listener, pipeline, functional_config(), Some(wrong));
    assert!(result.is_err(), "mismatched whip port must fail assembly");
}
```

- [ ] **Step 4: Update the e2e caller.** In `tests/e2e_gstreamer.rs`, pass the expected port and drop the hand-alignment comment on the `Args.port` line:

```rust
let app = Application::assemble(listener, pipeline.clone(), config, Some(HTTP_PORT)).unwrap();
```

- [ ] **Step 5: Build and test.**

Run: `cargo test`
Expected: green, including `assemble_rejects_a_mismatched_whip_port`.

- [ ] **Step 6: Re-run the e2e once (isolated), since its wiring changed.** Use the Task 9 Step 5 recipe. Expected: `ok`, clean exit.

- [ ] **Step 7: Commit.**

```bash
git add src/startup.rs src/main.rs tests/signaling.rs tests/e2e_gstreamer.rs
git commit -m "feat(startup): assert the listener port matches the pipeline's loopback WHIP port"
```

---

## Task 14: Expose `CoordinatorConfig` as CLI flags

**Files:**
- Modify: `src/signal/coordinator.rs` (add `CoordinatorArgs` deriving `clap::Args` + `to_config`; unit test)
- Modify: `src/signal/mod.rs` (export `CoordinatorArgs`)
- Modify: `src/stream/pipeline.rs` (`Args` derives `clap::Args`, drop the top-level `#[command(...)]`)
- Modify: `src/main.rs` (root `Cli` flattens both; build config from flags)

**Interfaces:**
- Produces: `signal::CoordinatorArgs` (`#[derive(clap::Args)]`) with six flags defaulting to today's values, and `CoordinatorArgs::to_config(&self) -> CoordinatorConfig`.
- Root `Cli` (in `main.rs`) `#[derive(clap::Parser)]` flattening `stream::Args` and `signal::CoordinatorArgs`. Layering intact: `stream` never imports `signal`; the crate-root binary composes both.

- [ ] **Step 1: Write the defaults-equivalence test.** In `src/signal/coordinator.rs` `mod tests`:

```rust
#[test]
fn coordinator_args_default_to_the_hardcoded_config() {
    use super::CoordinatorArgs;
    use clap::Parser;

    // A tiny throwaway parser so we can parse CoordinatorArgs with no flags.
    #[derive(Parser)]
    struct OnlyCoordinator {
        #[command(flatten)]
        coordinator: CoordinatorArgs,
    }

    let parsed = OnlyCoordinator::parse_from(["test"]);
    let from_flags = parsed.coordinator.to_config();
    let defaults = CoordinatorConfig::default();

    assert_eq!(from_flags.offer_timeout, defaults.offer_timeout);
    assert_eq!(from_flags.answer_timeout, defaults.answer_timeout);
    assert_eq!(from_flags.watchdog_threshold, defaults.watchdog_threshold);
    assert_eq!(from_flags.watchdog_window, defaults.watchdog_window);
    assert_eq!(from_flags.sweep_interval, defaults.sweep_interval);
    assert_eq!(from_flags.teardown_timeout, defaults.teardown_timeout);
}
```

- [ ] **Step 2: Run to confirm failure.**

Run: `cargo test --lib signal::coordinator::tests::coordinator_args`
Expected: FAIL — `CoordinatorArgs` not found.

- [ ] **Step 3: Add `CoordinatorArgs`.** In `src/signal/coordinator.rs` (top of file, add `use clap::Args as ClapArgs;` or use `#[derive(clap::Args)]` inline). Add after `CoordinatorConfig`/its `Default`:

```rust
/// CLI surface for the coordinator's timing/watchdog knobs. Kept separate
/// from `stream::Args` so `stream` never depends on `signal`; the crate-root
/// binary flattens both into one parser.
#[derive(clap::Args, Debug, Clone)]
pub struct CoordinatorArgs {
    /// Seconds a WHEP client waits for the whipsink's SDP offer.
    #[clap(long, default_value_t = 10)]
    pub offer_timeout_sec: u64,
    /// Seconds the whipsink waits for the browser's SDP answer.
    #[clap(long, default_value_t = 10)]
    pub answer_timeout_sec: u64,
    /// Consecutive handshake failures (within the window) that trip a restart.
    #[clap(long, default_value_t = 3)]
    pub watchdog_threshold: u32,
    /// Seconds over which failures decay for the watchdog.
    #[clap(long, default_value_t = 60)]
    pub watchdog_window_sec: u64,
    /// Expiry-sweep interval in milliseconds.
    #[clap(long, default_value_t = 1000)]
    pub sweep_interval_ms: u64,
    /// Upper bound, in seconds, on a single branch teardown/quit.
    #[clap(long, default_value_t = 5)]
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
```

- [ ] **Step 4: Export it.** In `src/signal/mod.rs`, extend the coordinator re-export: `pub use coordinator::{Coordinator, CoordinatorArgs, CoordinatorConfig};`.

- [ ] **Step 5: Make `stream::Args` flatten-friendly.** In `src/stream/pipeline.rs`, change the derive/attributes on `Args` from `#[derive(Parser, Debug, Clone)]` + `#[command(author, version, about, long_about = None)]` to:

```rust
#[derive(clap::Args, Debug, Clone)]
pub struct Args {
```

(Remove the `#[command(author, version, about, long_about = None)]` line and drop the now-unused `Parser` from the `use clap::{...}` import, keeping `ValueEnum`.)

- [ ] **Step 6: Compose the root parser in `main.rs`.** Update `src/main.rs`:

```rust
use clap::Parser;
use srt_whep::signal::CoordinatorArgs;
use srt_whep::startup::Application;
use srt_whep::stream::{Args, SharablePipeline};
use srt_whep::telemetry::{get_subscriber, init_subscriber};
use std::error::Error;
use std::net::TcpListener;

/// srt-whep: SRT to WHEP (WebRTC) gateway.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    pipeline: Args,
    #[command(flatten)]
    coordinator: CoordinatorArgs,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    let subscriber = get_subscriber("srt_whep".into(), "debug".into(), std::io::stdout);
    init_subscriber(subscriber);

    let pipeline = SharablePipeline::new(cli.pipeline.clone());
    let listener = TcpListener::bind(format!("0.0.0.0:{}", cli.pipeline.port))
        .expect("WHEP port is already in use");
    let app = Application::assemble(
        listener,
        pipeline,
        cli.coordinator.to_config(),
        Some(cli.pipeline.port),
    )?;

    app.run_until_stopped(shutdown_signal()).await?;
    Ok(())
}
```

(Keep the existing `shutdown_signal` functions unchanged.)

- [ ] **Step 7: Run the tests.**

Run: `cargo test`
Expected: green, including `coordinator_args_default_to_the_hardcoded_config`.

- [ ] **Step 8: Smoke-check the CLI parses.** With the env sourced:

Run: `cargo run -- --help`
Expected: help lists both the SRT/pipeline flags and the six `--*-timeout-sec` / `--watchdog-*` / `--sweep-interval-ms` flags; `--version` still works.

- [ ] **Step 9: Commit.**

```bash
git add src/signal/coordinator.rs src/signal/mod.rs src/stream/pipeline.rs src/main.rs
git commit -m "feat(cli): expose CoordinatorConfig knobs as flags (defaults unchanged)"
```

---

## Task 15: Delimit the loopback-WHIP bridge (prep for whepserversink)

**Files:**
- Modify: `src/stream/branch.rs` (strengthen the module doc)
- Modify: `docs/adr/0001-signaling-plane-rebuild.md` (point Future Work at the deletion boundary)

**Interfaces:** none (documentation + comments only; no behavior change).

- [ ] **Step 1: Strengthen the branch module doc.** Prepend/extend the `//!` header of `src/stream/branch.rs` to name it explicitly as the deletion boundary:

```rust
//! One viewer's per-connection pipeline elements — "the Branch" — AND the
//! entire loopback-WHIP bridge.
//!
//! Everything here exists only because egress uses a WHIP *client*
//! (`whipclientsink`): the loopback route template ([`WHIP_SINK_ROUTE`] /
//! [`whip_sink_path`]), the endpoint URL the sink POSTs its offer to
//! (`whip_endpoint`), and the attach/detach of that sink. The listener↔pipeline
//! port coupling asserted in `startup::Application::assemble` exists for the
//! same reason.
//!
//! This module is the deletion boundary for the `whepserversink` migration
//! (ADR 0001, Future Work): moving egress to the native server-initiated
//! `whepserversink` removes this bridge wholesale. Keep loopback-specific
//! surface confined here so that migration stays a clean deletion.
//!
//! `startup.rs` imports [`WHIP_SINK_ROUTE`] and the WHIP handler imports
//! [`whip_sink_path`], so the HTTP contract and the whipclientsink's endpoint
//! can never drift apart.
```

- [ ] **Step 2: Point ADR 0001 at the boundary.** In `docs/adr/0001-signaling-plane-rebuild.md`, in the Future Work / `whepserversink` section, add a line:

```markdown
- **Deletion boundary:** the loopback-WHIP bridge is confined to
  `src/stream/branch.rs` (route template, endpoint URL, whipclientsink
  attach) plus the `expected_whip_port` check in
  `startup::Application::assemble`. The migration removes these together.
```

- [ ] **Step 3: Verify nothing broke (docs/comments only).**

Run: `cargo test`
Expected: green.

- [ ] **Step 4: Commit.**

```bash
git add src/stream/branch.rs docs/adr/0001-signaling-plane-rebuild.md
git commit -m "docs: mark the loopback-WHIP bridge as the whepserversink deletion boundary"
```

---

## Self-Review Notes

- **Spec coverage:** Tasks 1–2 = proposal Phase 1; Tasks 3–6 = Phase 2; Tasks 7–8 = Phase 3; Tasks 9–11 = Phase 4; Tasks 12–13 = Phase 5; Tasks 14–15 = Phase 6. Every proposal commit maps to a task.
- **Mandatory ordering:** Task 10 (fake gate) before Task 11 (drop pre-check), else `not_ready_pipeline_rejects_creation` would pass vacuously. Task 12 (u16) before Task 13 (port assert). Task 9 before Task 13's e2e re-run.
- **Type consistency:** `OfferReply`/`AnswerReply`/`UnitReply` (Task 8) are used identically in `messages.rs`, `coordinator.rs`, and `mod.rs`. `SdpOffer`/`SdpAnswer` names are consistent across domain, signal, and routes. `input_ready` (Task 9) is the single readiness predicate. `expected_whip_port: Option<u16>` (Task 13) matches `u16` from Task 12.
- **Real-GStreamer risk:** confined to Task 9 (control-flow only) and Task 13's wiring; both gated by a manual isolated e2e run. Every other task is covered by CI-run unit/integration tests.
- **Layering:** Task 14 keeps `stream` free of `signal` by composing `Args` + `CoordinatorArgs` at the crate-root binary, not by cross-importing.
