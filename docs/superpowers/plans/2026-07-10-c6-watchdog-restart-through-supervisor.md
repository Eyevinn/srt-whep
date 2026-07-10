# C6 — Route watchdog restarts through the supervisor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move `quit` from `BranchControl` to `PipelineLifecycle` and have the watchdog *request* a pipeline restart over an explicit channel, so the supervisor is the sole owner of ending and rerunning a pipeline.

**Architecture:** A behavior-preserving mechanism refactor. Today the coordinator's watchdog trip calls `BranchControl::quit()`, whose side effect resolves the supervisor's parked `run()` — a cross-seam coupling the `TestPipeline` fake hand-wires. After this change the coordinator does a non-blocking `restart_tx.try_send(())`; the supervisor's select loop force-quits the run (bounded) and reruns at base delay. ADR-0001/0002 watchdog semantics are unchanged; the pinned tests are updated deliberately where they observed the old mechanism.

**Tech Stack:** Rust, tokio (`mpsc` channels, `select!`, current-thread `start_paused` actor tests), actix-web, GStreamer framework env for `cargo` on macOS.

## Global Constraints

- **macOS build/test env:** every `cargo` command must be prefixed with the framework GStreamer env — `source tests/browser/lib/env.sh` (each shell is fresh).
- **Behavior unchanged.** ADR-0001/0002 pinned semantics stay pinned: N handshake failures in window ⇒ fail all pending waiters ⇒ full pipeline restart at base delay; runtime branch reaps do **not** feed the watchdog; teardown/quit stays bounded. A *changed* semantic assertion (beyond the deliberate mechanism-observation swaps listed here) is a red flag — stop and reconsider.
- **Shared repo:** stage explicit paths only (never `git add -A`/`.`/`-a`), no branch ops inside a task. If `git status` shows changes you did not create (especially other `.rs` files), STOP and report. There is a known unrelated working-tree edit to `docs/proposals/2026-07-08-signal-plane-and-config-hygiene.md` and a concurrent deletion of `scripts/test_server.py` — leave both untouched.
- **Warnings are errors:** `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` must be clean.
- Commit messages end with the two trailers (`Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>` and `Claude-Session: https://claude.ai/code/session_01MPfWCwKizxWm8Etutjuved`).

## File Structure

- `docs/adr/0005-watchdog-restart-through-supervisor.md` — **new** ADR (Task 1).
- `src/stream/pipeline.rs` — trait defs (`quit` moves `BranchControl`→`PipelineLifecycle`); `TestPipeline::quit` moves impl block.
- `src/stream/gst_pipeline.rs` — `SharablePipeline::quit` moves impl block.
- `src/signal/mod.rs` — `spawn_coordinator` gains a `restart_tx` param.
- `src/signal/coordinator.rs` — `restart_tx` field; trip sends instead of quitting; delete `quit_bounded`; rework watchdog actor tests.
- `src/supervisor.rs` — `restart_rx` field; `spawn` param; new select arm + `RunOutcome::Restart`; rework tests.
- `src/startup.rs` — `assemble` creates the restart channel and wires both ends.
- `tests/signaling.rs` — one watchdog assertion becomes a poll (async trip).

Task 2 is atomic: `quit` cannot move traits without simultaneously rewiring its only caller (the coordinator) and its new caller (the supervisor), so the crate does not compile — and the suite is not green — until every Task 2 step is done. Verification is at the end of the task, which is the reviewer gate. This mirrors the C4 plan's "existing suite is the safety net" refactor shape.

---

## Task 1: Write ADR-0005

**Files:**
- Create: `docs/adr/0005-watchdog-restart-through-supervisor.md`

**Interfaces:**
- Consumes: nothing.
- Produces: the accepted decision record Task 2 implements. No code depends on it.

- [ ] **Step 1: Create the ADR file** with exactly this content:

```markdown
# 5. Route watchdog restarts through the supervisor's lifecycle seam

Date: 2026-07-10

## Status

Accepted. Refines the mechanism notes in ADR-0001 (§Consequences, the
"mechanism near the mailbox is revisited, not patched" clause) and ADR-0002
(watchdog rows). Does not change the pinned watchdog *semantics*.

## Context

The signaling plane splits the pipeline into two traits by caller:
`BranchControl` (the coordinator's per-connection seam) and `PipelineLifecycle`
(the supervisor's whole-pipeline seam). `quit` sat on `BranchControl`, yet its
only effect is to end the supervisor's `run()` — a cross-seam side effect
neither interface documents, and one the `TestPipeline` fake must hand-wire (its
`BranchControl::quit` fires the `run_gate` its `PipelineLifecycle::run` awaits).
ADR-0001 flagged that mechanism changes near the mailbox get a formal revisit;
this is that revisit.

## Decision

- `quit` moves from `BranchControl` to `PipelineLifecycle`. The coordinator's
  seam carries only per-connection verbs (`ready`, `add_branch`,
  `remove_branch`); the supervisor's seam owns the whole-pipeline lifecycle,
  including forcefully ending a run.
- On a watchdog trip the coordinator fails all pending waiters and sends a
  restart *request* over an explicit `mpsc` channel (symmetric with the
  `branch_failures` reap channel). It no longer ends the run itself.
- The supervisor's select loop gains a restart arm that force-quits the current
  run (bounded by the same join timeout as graceful shutdown) and reruns through
  its normal cleanup/backoff path, treated as a clean restart (base delay).
- A forceful `quit` (direct `main_loop.quit()`), not a graceful `end` (EOS
  event), is retained for restart: the watchdog exists for the suspected-wedge
  case, where EOS may never propagate and the process — unlike at shutdown —
  stays alive, so the old run must be guaranteed dead before rerunning.

## Consequences

- Watchdog semantics are unchanged (N failures in window ⇒ fail all waiters ⇒
  full restart; reaps don't feed the watchdog; base-delay restart). The
  force-quit *bound* relocates from the coordinator's `teardown_timeout` to the
  supervisor's bounded join. The coordinator's trip path is now non-blocking
  (`try_send`), so a wedged quit can never stall the mailbox.
- The `TestPipeline` cross-trait `run_gate` wiring is gone; `quit` releasing the
  run is now within the `PipelineLifecycle` domain.
- Pinned watchdog tests were updated deliberately to observe the restart request
  instead of a recorded `quit`; intent and assertions are otherwise preserved.
```

- [ ] **Step 2: Commit** (explicit path only).

```bash
git add docs/adr/0005-watchdog-restart-through-supervisor.md
git commit  # subject: "docs(adr): ADR-0005 route watchdog restarts through the supervisor (C6)"
```

---

## Task 2: Relocate `quit` and route the watchdog restart through the supervisor

**Files:**
- Modify: `src/stream/pipeline.rs` (trait defs ~89–108; `TestPipeline::quit` ~257–261; `TestPipelineState` ~110–124 if needed; test helpers)
- Modify: `src/stream/gst_pipeline.rs` (`SharablePipeline::quit` ~189–197)
- Modify: `src/signal/mod.rs` (`spawn_coordinator` ~26–34; the `handle_drives_a_full_handshake` test ~104–...)
- Modify: `src/signal/coordinator.rs` (struct ~202–213, `new` ~215–231, `fail_connection` ~427–436, `quit_bounded` ~479–486; test helpers ~528–548; watchdog actor tests)
- Modify: `src/supervisor.rs` (struct ~17–21, `spawn` ~26–39, `run` match ~54–64, `run_pipeline_until_stopped` ~77–111, `RunOutcome` ~128–133; tests ~156–301)
- Modify: `src/startup.rs` (`assemble` ~51–82)
- Modify: `tests/signaling.rs` (`watchdog_restarts_pipeline_after_consecutive_failures` ~279–294)

**Interfaces:**
- Consumes: existing `PipelineLifecycle`, `BranchControl`, `spawn_coordinator`, `Supervisor::spawn`, `Application::assemble`.
- Produces:
  - `PipelineLifecycle::quit(&self) -> Result<(), PipelineError>` (moved here; `PipelineError` is already the return type quit used).
  - `spawn_coordinator<P: BranchControl + 'static>(pipeline: P, config: CoordinatorConfig, branch_failures: mpsc::Receiver<BranchId>, restart_tx: mpsc::Sender<()>) -> SignalHandle`.
  - `Supervisor::spawn(pipeline: P, signal: SignalHandle, shutdown: watch::Receiver<bool>, restart_rx: mpsc::Receiver<()>) -> JoinHandle<()>`.
  - `RunOutcome::Restart` variant.

> **Note on `quit`'s return type.** `BranchControl::quit` returns `Result<(), PipelineError>`. Keep that exact signature when moving it to `PipelineLifecycle` (do **not** switch it to `PipelineLifecycle`'s usual `Result<(), Error>` — `Error` is `anyhow::Error`; `PipelineError` is fine and is what the supervisor's `let _ = self.pipeline.quit().await;` discards).

- [ ] **Step 1: Move `quit` between the trait definitions** in `src/stream/pipeline.rs`. Remove the `quit` line from `BranchControl` and add it to `PipelineLifecycle`:

```rust
#[async_trait]
pub trait BranchControl: Clone + Send + Sync {
    async fn ready(&self) -> Result<bool, PipelineError>;
    async fn add_branch(&self, id: String) -> Result<(), PipelineError>;
    async fn remove_branch(&self, id: String) -> Result<(), PipelineError>;
}

/// The supervisor's view of the pipeline: whole-pipeline lifecycle.
///
/// Call order: `init` → `run` (resolves only at EOS, a fatal error, or a
/// forced `quit`) → `clean_up`, after which `init` may be called again. `end`
/// requests EOS from outside that loop for a graceful shutdown; `quit` forces
/// the run down immediately (used by the supervisor on a watchdog restart).
#[async_trait]
pub trait PipelineLifecycle: Clone + Send + Sync {
    async fn init(&self) -> Result<(), Error>;
    async fn run(&self) -> Result<(), Error>;
    async fn end(&self) -> Result<(), Error>;
    async fn clean_up(&self) -> Result<(), Error>;
    async fn quit(&self) -> Result<(), PipelineError>;
}
```

- [ ] **Step 2: Move `SharablePipeline::quit`** in `src/stream/gst_pipeline.rs` from the `impl BranchControl for SharablePipeline` block to the `impl PipelineLifecycle for SharablePipeline` block. The method body is unchanged; only which `impl` block it lives in changes. Cut this method out of the `BranchControl` impl (currently ~lines 186–197):

```rust
    /// Quit pipeline by sending a quit message to the main loop
    /// This function is used to restart the pipeline in case of
    /// unrecoverable errors
    async fn quit(&self) -> Result<(), PipelineError> {
        let pipeline_state = self.state.lock_err().await?;
        if let Some(main_loop) = pipeline_state.main_loop.as_ref() {
            tracing::debug!("Force-quit pipeline");
            main_loop.quit();
        }

        Ok(())
    }
```

and paste it as the last method inside `impl PipelineLifecycle for SharablePipeline` (after `clean_up`).

- [ ] **Step 3: Move `TestPipeline::quit`** in `src/stream/pipeline.rs` from the `impl BranchControl for TestPipeline` block (currently ~lines 257–261) to the `impl PipelineLifecycle for TestPipeline` block (after `clean_up`, ~line 289). Body unchanged:

```rust
    async fn quit(&self) -> Result<(), PipelineError> {
        self.state.lock().unwrap().quit_count += 1;
        self.run_gate.notify_one();
        Ok(())
    }
```

(`quit_count` stays on `TestPipelineState` — it now records supervisor-driven quits and is asserted by supervisor tests.)

- [ ] **Step 4: `spawn_coordinator` gains `restart_tx`** in `src/signal/mod.rs`. Update the signature and the `Coordinator::new` call:

```rust
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
```

Update the doc comment above it to mention `restart_tx` ("the watchdog trip sends a `()` restart request to the supervisor over `restart_tx`").

- [ ] **Step 5: Coordinator holds `restart_tx`; trip sends instead of quitting** in `src/signal/coordinator.rs`.

  5a. Add the field to the struct (after `branch_failures`):

```rust
    branch_failures: mpsc::Receiver<BranchId>,
    /// Watchdog restart requests to the supervisor. On a trip the coordinator
    /// fails all waiters and sends `()` here; the supervisor owns the actual
    /// force-quit + rerun. A non-blocking `try_send` (coalescing) so a wedged
    /// pipeline can never stall this mailbox.
    restart_tx: mpsc::Sender<()>,
```

  5b. Add the parameter to `Coordinator::new` and store it:

```rust
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
```

  5c. Replace the trip body in `fail_connection`:

```rust
        if self.watchdog.record_failure() {
            tracing::error!("Watchdog tripped: requesting a pipeline restart");
            self.reset_all();
            // Non-blocking: the supervisor owns the force-quit + rerun. A full
            // buffer means a restart is already pending, so dropping the extra
            // request is correct.
            let _ = self.restart_tx.try_send(());
        }
```

  5d. Delete the `quit_bounded` method entirely (currently ~lines 479–486). The coordinator no longer calls any whole-pipeline method.

- [ ] **Step 6: Supervisor gains `restart_rx`, the restart arm, and `RunOutcome::Restart`** in `src/supervisor.rs`.

  6a. Add the field to the struct:

```rust
pub struct Supervisor<P: PipelineLifecycle> {
    pipeline: P,
    signal: SignalHandle,
    shutdown: watch::Receiver<bool>,
    restart_rx: mpsc::Receiver<()>,
}
```

  6b. Add the import at the top (`use tokio::sync::watch;` already exists — add `mpsc`):

```rust
use tokio::sync::{mpsc, watch};
```

  6c. Add the parameter to `spawn` and store it:

```rust
    pub fn spawn(
        pipeline: P,
        signal: SignalHandle,
        shutdown: watch::Receiver<bool>,
        restart_rx: mpsc::Receiver<()>,
    ) -> JoinHandle<()> {
        tokio::spawn(
            Self {
                pipeline,
                signal,
                shutdown,
                restart_rx,
            }
            .run(),
        )
    }
```

  6d. Add the `Restart` variant to `RunOutcome`:

```rust
enum RunOutcome {
    /// The pipeline stopped on its own: cleanly (EOS) or with an error.
    Completed(Result<(), anyhow::Error>),
    /// A shutdown token arrived; the pipeline was stopped and joined.
    ShuttingDown,
    /// The watchdog requested a restart; the run was force-quit and joined.
    Restart,
}
```

  6e. Handle `Restart` in the `run()` match (add an arm alongside the existing ones):

```rust
            match outcome {
                RunOutcome::ShuttingDown => break,
                RunOutcome::Restart => {
                    consecutive_failures = 0; // base-delay rerun, like a clean run
                    tracing::info!("Watchdog requested a restart. Reset and rerun the pipeline.");
                }
                RunOutcome::Completed(Ok(())) => {
                    consecutive_failures = 0;
                    tracing::info!("Pipeline reached EOS. Reset and rerun the pipeline.");
                }
                RunOutcome::Completed(Err(e)) => {
                    consecutive_failures = consecutive_failures.saturating_add(1);
                    tracing::error!("Pipeline stopped with an error: {}", e);
                }
            }
```

  6f. Add the third select arm in `run_pipeline_until_stopped` (after the shutdown arm):

```rust
        tokio::select! {
            res = &mut run_task => RunOutcome::Completed(flatten_join(res)),
            _ = wait_for_shutdown(&mut self.shutdown) => {
                let _ = self.pipeline.end().await;
                match tokio::time::timeout(SHUTDOWN_JOIN_TIMEOUT, &mut run_task).await {
                    Ok(joined) => {
                        if let Err(e) = flatten_join(joined) {
                            tracing::warn!("Pipeline stopped with an error during shutdown: {}", e);
                        }
                    }
                    Err(_) => {
                        run_task.abort();
                        tracing::error!(
                            "Pipeline did not stop within {:?} after EOS; abandoning it",
                            SHUTDOWN_JOIN_TIMEOUT
                        );
                    }
                }
                RunOutcome::ShuttingDown
            }
            Some(()) = self.restart_rx.recv() => {
                // Watchdog asked for a restart: force the run down, then join
                // (bounded — a wedged quit must not hang the loop).
                let _ = self.pipeline.quit().await;
                match tokio::time::timeout(SHUTDOWN_JOIN_TIMEOUT, &mut run_task).await {
                    Ok(joined) => {
                        if let Err(e) = flatten_join(joined) {
                            tracing::warn!("Pipeline errored during watchdog restart: {}", e);
                        }
                    }
                    Err(_) => {
                        run_task.abort();
                        tracing::error!(
                            "Pipeline did not stop within {:?} after watchdog quit; abandoning it",
                            SHUTDOWN_JOIN_TIMEOUT
                        );
                    }
                }
                RunOutcome::Restart
            }
        }
```

- [ ] **Step 7: Wire the restart channel in `startup.rs::assemble`** (`src/startup.rs`). Create the channel and pass each end. `mpsc` is already imported (`branch_failures: mpsc::Receiver<BranchId>`). Replace the two spawn lines:

```rust
        // The watchdog restart channel: the coordinator holds the sender (it
        // requests a restart on a trip), the supervisor the receiver (it owns
        // the force-quit + rerun). Created here so both ends exist before
        // either task is spawned — symmetric with the bus-reap channel.
        let (restart_tx, restart_rx) = mpsc::channel(1);
        let signal = spawn_coordinator(pipeline.clone(), config, branch_failures, restart_tx);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let supervisor = Supervisor::spawn(pipeline, signal.clone(), shutdown_rx, restart_rx);
```

- [ ] **Step 8: Update the coordinator actor test helpers + watchdog assertions** in `src/signal/coordinator.rs`.

  8a. The helpers currently return `SignalHandle`. Change them to also return the restart receiver so watchdog tests can observe the request. Update `spawn_actor` and `spawn_actor_with_reaper`:

```rust
    /// Spawn the coordinator and return its `SignalHandle` plus the receiving
    /// end of the watchdog restart channel. No reaper wired (disconnected
    /// failure receiver). Tests that trip the watchdog assert on `restart_rx`.
    pub(super) fn spawn_actor(
        pipeline: TestPipeline,
        config: CoordinatorConfig,
    ) -> (SignalHandle, mpsc::Receiver<()>) {
        let (_fail_tx, fail_rx) = mpsc::channel(1);
        let (restart_tx, restart_rx) = mpsc::channel(1);
        (spawn_coordinator(pipeline, config, fail_rx, restart_tx), restart_rx)
    }

    pub(super) fn spawn_actor_with_reaper(
        pipeline: TestPipeline,
        config: CoordinatorConfig,
        branch_failures: mpsc::Receiver<BranchId>,
    ) -> (SignalHandle, mpsc::Receiver<()>) {
        let (restart_tx, restart_rx) = mpsc::channel(1);
        (
            spawn_coordinator(pipeline, config, branch_failures, restart_tx),
            restart_rx,
        )
    }
```

  8b. Every actor test binds the helper's return as a tuple. For tests that don't touch the watchdog, bind the receiver as `_restart_rx` (keep it alive so the sender isn't dropped — a dropped receiver would make `try_send` on a trip error, which is harmless here but keep it bound for clarity). Example rewrite of a non-watchdog test header:

```rust
        let (handle, _restart_rx) = spawn_actor(pipeline.clone(), test_config());
```

  Apply the same tuple-binding to `spawn_actor_with_reaper` call sites (e.g. `established_connection_is_reaped_on_branch_failure`).

  8c. Rewrite the three watchdog assertions to observe the restart request instead of `quit_count`:

  - `watchdog_trips_after_three_consecutive_failures` — replace `assert_eq!(1, pipeline.snapshot().quit_count);` with:

```rust
        assert!(restart_rx.try_recv().is_ok()); // one restart requested
```

  (bind `let (handle, mut restart_rx) = spawn_actor(...)` for this test.)

  - `success_between_failures_prevents_the_trip` — replace `assert_eq!(0, pipeline.snapshot().quit_count);` with:

```rust
        assert!(restart_rx.try_recv().is_err()); // no restart requested
```

  - `reset_clears_the_watchdog_counter` — replace `assert_eq!(0, pipeline.snapshot().quit_count);` with:

```rust
        assert!(restart_rx.try_recv().is_err()); // no restart requested
```

  8d. Any other actor test asserting `quit_count == 0` incidentally (`offer_timeout_fails_only_that_connection` and `abandoned_whep_client_is_reaped_by_the_sweep` assert `snap.quit_count == 0` for "watchdog not tripped") — replace that line with `assert!(restart_rx.try_recv().is_err());` and bind `mut restart_rx`. Search the test module for `quit_count` and convert every occurrence.

- [ ] **Step 9: Update the `mod.rs` facade test** in `src/signal/mod.rs`. `handle_drives_a_full_handshake` calls `spawn_coordinator(pipeline.clone(), CoordinatorConfig::default(), fail_rx)`. Add a restart sender:

```rust
        let (_restart_tx, restart_rx) = mpsc::channel::<()>(1);
        let _ = &restart_rx; // supervisor-less test: nothing consumes restarts
        let handle = spawn_coordinator(pipeline.clone(), CoordinatorConfig::default(), fail_rx, _restart_tx);
```

  (If the test never trips the watchdog, holding `_restart_tx` is enough; the `restart_rx` binding just keeps the channel open. Simplest: `let (restart_tx, _restart_rx) = mpsc::channel::<()>(1);` and pass `restart_tx`.)

- [ ] **Step 10: Update the supervisor tests** in `src/supervisor.rs`.

  10a. `spawn_coordinator_no_reaper` now must also supply a restart sender to `spawn_coordinator`, and the tests must create the restart channel and pass the receiver to `Supervisor::spawn`. Change the helper to build and return both the handle and the restart sender/receiver, or (simpler) create the restart channel inline in each test. Recommended: a small helper that returns the wired supervisor pieces. Replace `spawn_coordinator_no_reaper`:

```rust
    /// Build the coordinator + restart channel for a supervisor test. The
    /// coordinator gets a disconnected failure receiver (never reaps); the
    /// returned `restart_tx` lets a test drive a watchdog restart directly, and
    /// `restart_rx` is handed to `Supervisor::spawn`.
    fn wire(pipeline: TestPipeline) -> (SignalHandle, mpsc::Sender<()>, mpsc::Receiver<()>) {
        let (_fail_tx, fail_rx) = mpsc::channel(1);
        let (restart_tx, restart_rx) = mpsc::channel(1);
        let signal = spawn_coordinator(pipeline, CoordinatorConfig::default(), fail_rx, restart_tx.clone());
        (signal, restart_tx, restart_rx)
    }
```

  10b. Update every supervisor test to use `wire` and pass `restart_rx` to `Supervisor::spawn`. Pattern (e.g. `restarts_after_a_failed_run`):

```rust
        let pipeline = TestPipeline::default();
        let (signal, _restart_tx, restart_rx) = wire(pipeline.clone());
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx, restart_rx);
```

  Apply to all six existing tests. Tests that don't drive a restart bind `_restart_tx`.

  10c. Replace `quit_restarts_like_a_clean_run` with a restart-channel-driven version. The coordinator is not involved here — the test sends the restart request directly on `restart_tx`:

```rust
    #[tokio::test(start_paused = true)]
    async fn restart_request_restarts_like_a_clean_run() {
        // A watchdog restart request force-quits the run and reruns, exactly
        // like EOS — cleanup runs and the pipeline is rerun at base delay.
        let pipeline = TestPipeline::default();
        let (signal, restart_tx, restart_rx) = wire(pipeline.clone());
        let (_shutdown_tx, shutdown_rx) = watch::channel(false);
        let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx, restart_rx);
        wait_until(&pipeline, |s| s.run_count == 1).await;

        restart_tx.send(()).await.unwrap();

        wait_until(&pipeline, |s| s.quit_count == 1).await; // supervisor force-quit
        wait_until(&pipeline, |s| s.cleanup_count == 1).await;
        wait_until(&pipeline, |s| s.run_count == 2).await;
    }
```

- [ ] **Step 11: Update the integration watchdog test** in `tests/signaling.rs`. The trip is now async across two tasks (coordinator → channel → supervisor), so the immediate `assert_eq!(1, quit_count)` would race. Replace it with a bounded poll, reusing the existing polling style:

```rust
    // Threshold 2: the second consecutive failure requests a restart; the
    // supervisor force-quits the pipeline. That hop is async (coordinator ->
    // channel -> supervisor), so poll rather than assert immediately.
    for _ in 0..200 {
        if pipeline.snapshot().quit_count == 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(1, pipeline.snapshot().quit_count);
```

  The `assert_eq!(0, ...quit_count)` no-trip assertions elsewhere in the file (~lines 147, 269, 275, 454) stay unchanged: with no trip, no restart is requested and the supervisor never quits, so `quit_count` is reliably 0.

- [ ] **Step 12: Build and run the full suite — must be green.**

Run: `source tests/browser/lib/env.sh && cargo test --all-targets`
Expected: PASS. Same test count as before except `quit_restarts_like_a_clean_run` is renamed to `restart_request_restarts_like_a_clean_run` (net 0). No hangs, 0 failures.

- [ ] **Step 13: Lint clean.**

Run: `source tests/browser/lib/env.sh && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: no warnings, no diff. (Watch for an unused-import warning if `BranchControl` is no longer needed in a test `use` list, and for `mut restart_rx` bindings that don't need `mut`.)

- [ ] **Step 14: Commit** (explicit paths only).

```bash
git add src/stream/pipeline.rs src/stream/gst_pipeline.rs src/signal/mod.rs \
        src/signal/coordinator.rs src/supervisor.rs src/startup.rs tests/signaling.rs
git commit  # subject: "refactor(signal): route watchdog restarts through the supervisor (C6)"
```

---

## Post-implementation (controller, after both tasks)

- Whole-branch code review (most-capable model) over the `main..HEAD` range for C6, with emphasis on: watchdog semantics unchanged, no cross-seam call remaining, the restart arm's bounded join, and dropped-sender arm safety.
- **Browser e2e regression guard:** `source tests/browser/lib/env.sh && tests/browser/run.sh` once — expect exit 0, frames climbing. C6 does not touch the runtime media path, so this is belt-and-suspenders.
- The `#[ignore]`d `tests/e2e_gstreamer.rs` needs no run.

## Self-Review

- **Spec coverage:** ADR-0005 (Task 1) ↔ spec § "Proposed ADR-0005". `quit` relocation (Task 2 Steps 1–3) ↔ spec § Decision 1. Restart channel + coordinator trip (Steps 4–5) ↔ Decision 2. Supervisor arm + `RunOutcome::Restart` + backoff (Step 6) ↔ Decision 3–4. Startup wiring (Step 7) ↔ spec § Files touched. Test updates (Steps 8–11) ↔ spec § Testing plan — with one correction: the integration `watchdog_restarts_*` test needs a poll (async trip), which the spec listed as "verify"; verified here and specified. ✓
- **Placeholder scan:** every code step shows complete code; the ADR text is verbatim, not "copy from spec". ✓
- **Type consistency:** `spawn_coordinator` gains `restart_tx: mpsc::Sender<()>`; `Coordinator::new` matches; `Supervisor::spawn` gains `restart_rx: mpsc::Receiver<()>`; `RunOutcome::Restart` handled in the `run()` match and produced by the select arm; `quit` keeps its `Result<(), PipelineError>` signature on its new trait. Helper return types (`(SignalHandle, mpsc::Receiver<()>)`, `wire -> (SignalHandle, mpsc::Sender<()>, mpsc::Receiver<()>)`) are used consistently at their call sites. ✓
- **Ordering safety:** the `Some(()) = self.restart_rx.recv()` arm uses the dropped-sender-disables-arm idiom (same as `branch_failures`), so unwired-restart tests never busy-loop; the actor tests bind the receiver to keep the sender alive. ✓
