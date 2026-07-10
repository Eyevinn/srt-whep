# C4 — Test the signal plane through `SignalHandle` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the coordinator's actor tests drive the `SignalHandle` facade instead of hand-building raw `Command` messages, make `Command` private to `src/signal`, and unify `ListConnections`/`Reset` into the uniform `request()` shape.

**Architecture:** A behavior-preserving refactor. The existing test suite is the safety net: each task must leave `cargo test --all-targets` green with the *same test names and assertions*. No production control flow changes; the only interface change is shrinking `Command`'s visibility and making two command replies `Result`-shaped (invisible to callers, whose method signatures already return `Result`).

**Tech Stack:** Rust, tokio (current-thread `start_paused` test runtime), actix-web (for `ResponseError` status assertions), GStreamer framework env for `cargo` on macOS.

## Global Constraints

- **macOS build/test env:** every `cargo` command must be prefixed with the framework GStreamer env — `source tests/browser/lib/env.sh` (or the `DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib` block). Each shell is fresh.
- **Zero files change outside `src/signal/`.** The only files touched are `src/signal/coordinator.rs`, `src/signal/mod.rs`, `src/signal/messages.rs`.
- **Behavior unchanged.** ADR-0001/0002 pinned semantics (per-connection failure isolation, watchdog trip → restart, bounded teardown, sweep reaping) stay pinned by the same assertions. A *changed* assertion is a red flag — stop and reconsider.
- **Shared repo:** stage explicit paths only (never `git add -A`/`.`/`-a`), no branch ops inside a task. If `git status` shows changes you did not create (especially other `.rs` files), STOP and report. There is a known unrelated working-tree edit to `docs/proposals/2026-07-08-signal-plane-and-config-hygiene.md` and a concurrent deletion of `scripts/test_server.py` — leave both untouched.
- **Warnings are errors:** `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must be clean.
- Commit messages end with the two trailers (`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` and `Claude-Session: …`).

## File Structure

- `src/signal/coordinator.rs` — the `#[cfg(test)] mod tests` block (helpers + 21 tests) is rewritten in Task 1. The non-test actor code changes only in Task 2 (two `reply.send` sites).
- `src/signal/mod.rs` — Task 2: `Command` moves from `pub use` to a private `use`; `list_connections`/`reset` collapse to `request()`.
- `src/signal/messages.rs` — Task 2: add `SnapshotReply` alias; make `ListConnections`/`Reset` replies `Result`-shaped.

---

## Task 1: Migrate the coordinator actor tests to `SignalHandle`

**Files:**
- Modify: `src/signal/coordinator.rs` (the `mod tests` block, currently lines ~496–1245)

**Interfaces:**
- Consumes: `crate::signal::spawn_coordinator<P: BranchControl + 'static>(pipeline, config, branch_failures: mpsc::Receiver<BranchId>) -> SignalHandle` (already exists), and the `SignalHandle` methods `create_connection(String) -> Result<SdpOffer, SignalError>`, `offer_received(String, SdpOffer) -> Result<SdpAnswer, SignalError>`, `answer_received(String, SdpAnswer) -> Result<(), SignalError>`, `remove_connection(String) -> Result<(), SignalError>`, `list_connections() -> Result<Vec<ConnectionInfo>, SignalError>`, `reset() -> Result<(), SignalError>`.
- Produces: nothing new for later tasks; Task 2 relies only on the fact that no test references `Command` afterward.

**Migration rules (three shapes):**
1. **Immediate reply → direct `.await`.** A call that resolves without a later command (rejections; timeouts that auto-advance to resolution; transient/fatal add failures).
2. **In-flight leg → `tokio::spawn` then `.await` the `JoinHandle`.** A `create_connection` that only resolves once its offer arrives; an `offer_received` that only resolves once the answer arrives. Spawn it, drive the dependency (send the next command), then await the handle. Use `tokio::task::yield_now().await` (or `for _ in 0..5` when a snapshot assert must observe the actor having *processed* — not merely enqueued — a command) to order the enqueue before the dependent command.
3. **Abandonment → `tokio::spawn` then `abort()`.** Express "client disconnected" by dropping the in-flight future: spawn, yield until registered, `abort()` (dropping the future drops the reply receiver, exactly as the old `drop(reply_rx)` did).

The three pure `ConnectionState` tests (`fail_waiter_notifies_*` ×2, `transition_table_accepts_only_legal_events`) never touch `Command` — leave them byte-for-byte unchanged.

- [ ] **Step 1: Replace the whole `mod tests` block** (from `use super::{Coordinator, CoordinatorConfig};` through the last actor test `wedged_add_branch_cleanup_times_out_to_a_retryable_error`, i.e. everything above the pure `fail_waiter_*`/`transition_table` tests). Keep `offer()`, `answer()`, `test_config()`, `ready_pipeline()` fixtures. Keep the three pure `ConnectionState` tests unchanged. Write:

```rust
    use super::CoordinatorConfig;
    use super::ConnectionState;
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

    /// Spawn the coordinator and return its `SignalHandle` — the exact facade
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
        handle.answer_received(id.to_string(), answer()).await.unwrap();
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
```

- [ ] **Step 2: Confirm the pure `ConnectionState` tests are untouched.** The three tests `fail_waiter_notifies_the_awaiting_offer_waiter`, `fail_waiter_notifies_the_awaiting_answer_waiter`, and `transition_table_accepts_only_legal_events` (which build `ConnectionState` directly and use `oneshot`/`Instant`) must remain exactly as they were, following the replaced block.

- [ ] **Step 3: Run the full suite — must be green.**

Run: `source tests/browser/lib/env.sh && cargo test --all-targets`
Expected: PASS — same test count as before in `signal::coordinator::tests` (all names preserved), no hangs, 0 failures.

- [ ] **Step 4: Lint clean.**

Run: `source tests/browser/lib/env.sh && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diff.

- [ ] **Step 5: Commit** (explicit path only).

```bash
git add src/signal/coordinator.rs
git commit  # subject: "refactor(signal): drive coordinator actor tests through SignalHandle (C4)"
```

---

## Task 2: Make `Command` private + unify `ListConnections`/`Reset`

**Files:**
- Modify: `src/signal/messages.rs` (reply types)
- Modify: `src/signal/coordinator.rs` (two `reply.send` sites, ~lines 259–273)
- Modify: `src/signal/mod.rs` (`pub use`; `list_connections`/`reset`)

**Interfaces:**
- Consumes: the `request()` helper on `SignalHandle` (already exists).
- Produces: `Command` no longer in the crate's public surface; `SnapshotReply` type alias; all six handle methods route through `request()`. No public method signature changes.

- [ ] **Step 1: `messages.rs` — make the two replies `Result`-shaped.** Add the alias next to the others and update the two variants:

```rust
pub type UnitReply = oneshot::Sender<Result<(), SignalError>>;
pub type SnapshotReply = oneshot::Sender<Result<Vec<ConnectionInfo>, SignalError>>;
```

```rust
    ListConnections { reply: SnapshotReply },
```

```rust
    /// Supervisor: the pipeline restarted; fail all waiters, clear the map.
    Reset { reply: UnitReply },
```

- [ ] **Step 2: `coordinator.rs` — wrap both replies in `Ok`.** In `handle_command`:
  - `ListConnections` arm: `let _ = reply.send(list);` → `let _ = reply.send(Ok(list));`
  - `Reset` arm: `let _ = reply.send(());` → `let _ = reply.send(Ok(()));`

- [ ] **Step 3: `mod.rs` — privatize `Command` and collapse the two methods.** Change the re-export and add a private import:

```rust
pub use messages::{ConnectionId, ConnectionInfo};
use messages::Command;
```

Replace the hand-rolled `list_connections` and `reset` bodies with the uniform `request()` form (docs unchanged):

```rust
    /// List the current connections and their states (GET /list). Sends
    /// `ListConnections` and awaits the reply carrying the snapshot; errors
    /// only if the coordinator is unavailable.
    pub async fn list_connections(&self) -> Result<Vec<ConnectionInfo>, SignalError> {
        self.request(|reply| Command::ListConnections { reply }).await
    }

    /// Reset the coordinator after a pipeline restart (supervisor only).
    /// Sends `Reset`, which fails all in-flight waiters and clears the
    /// connection map; the reply is `Ok(())` once done, or an error if the
    /// coordinator is unavailable.
    pub async fn reset(&self) -> Result<(), SignalError> {
        self.request(|reply| Command::Reset { reply }).await
    }
```

- [ ] **Step 4: Run the full suite — must be green.**

Run: `source tests/browser/lib/env.sh && cargo test --all-targets`
Expected: PASS, unchanged counts. (The migrated tests call `handle.list_connections()/reset()`, whose signatures are unchanged, so this is invisible to them.)

- [ ] **Step 5: Verify `Command` is truly private.**

Run: `grep -rn --include='*.rs' -E '\bCommand\b' src | grep -v 'src/signal/'`
Expected: no output (no external reference). And `grep -n 'pub use messages' src/signal/mod.rs` shows only `ConnectionId, ConnectionInfo`.

- [ ] **Step 6: Lint clean.**

Run: `source tests/browser/lib/env.sh && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 7: Commit** (explicit paths only).

```bash
git add src/signal/messages.rs src/signal/coordinator.rs src/signal/mod.rs
git commit  # subject: "refactor(signal): privatize Command; unify list/reset via request() (C4)"
```

---

## Post-implementation (controller, after both tasks)

- Whole-branch code review (most-capable model) over the `main..HEAD` range for C4.
- **Browser e2e regression guard:** `source tests/browser/lib/env.sh && tests/browser/run.sh` once — expect exit 0, frames climbing. C4 cannot change the runtime media path, so this is belt-and-suspenders.
- The `#[ignore]`d `tests/e2e_gstreamer.rs` needs no run (no `gst_pipeline.rs`/`branch.rs` change).

## Self-Review

- **Spec coverage:** Task 1 covers "actor tests construct no raw `Command`" + the Trap (abandonment via `abort()`, no escape hatch). Task 2 covers "`Command` private to `src/signal`" + the approved `ListConnections`/`Reset` unification. Browser e2e + full-suite-green cover the "behavior unchanged" done-when. ✓
- **Placeholder scan:** none — every code step shows complete code. ✓
- **Type consistency:** `spawn_actor`/`spawn_actor_with_reaper` return `SignalHandle`; `establish`/`list_ids` take `&SignalHandle`; `SnapshotReply`/`UnitReply` match the `request::<Vec<ConnectionInfo>>`/`request::<()>` closures; `messages::Command` stays in scope for `mod.rs` via the private `use`. ✓
- **Ordering safety:** in-flight legs are spawned and the enqueue is ordered before dependent commands with `yield_now`; snapshot asserts that must observe *processing* (not just enqueue) use `for _ in 0..5`. The abandonment test aborts only after registration. ✓
