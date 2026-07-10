# C6 — Route watchdog restarts through the supervisor's lifecycle seam (design)

**Status:** Approved (design); ready for implementation plan
**Date:** 2026-07-10
**Base:** `main` @ `bf83b42`
**Source card:** `docs/proposals/2026-07-09-architecture-deepening-candidates.md` § C6
**Candidate:** C6 (speculative, ADR-adjacent). Sixth of the arch-deepening
candidates; C1/C2a/C3/C4/C5/C7 already landed. Remaining after this: C2b.

## Goal

Remove the cross-seam coupling by which the watchdog restarts the pipeline.
Today a method on the *coordinator's* seam (`BranchControl::quit`) has the side
effect of ending the *supervisor's* pipeline run — a coupling neither interface
admits and the `TestPipeline` fake must hand-wire. Move `quit` to the
supervisor's seam (`PipelineLifecycle`) and have the watchdog *request* a
restart over an explicit channel, so the supervisor is the single owner of
ending and rerunning a pipeline.

**Behavior-preserving.** This moves mechanism, not semantics. The ADR-0001/0002
pinned watchdog semantics stay pinned by the same tests, with assertions updated
deliberately (not incidentally) where they observed the old mechanism.

## The coupling today

Two traits split the pipeline by caller (`src/stream/pipeline.rs:90,103`):

- **`BranchControl`** (coordinator's seam): `ready`, `add_branch`,
  `remove_branch`, **`quit`**.
- **`PipelineLifecycle`** (supervisor's seam): `init`, `run`, `end`, `clean_up`.

On a watchdog trip (`coordinator.rs:427` `fail_connection`), the coordinator
calls `reset_all()` then `quit_bounded()` → `BranchControl::quit()`. In the real
pipeline (`gst_pipeline.rs:189`) `quit()` calls `main_loop.quit()`, which
resolves the supervisor's parked `PipelineLifecycle::run()`
(`gst_pipeline.rs:485`), triggering the supervisor's cleanup → backoff → rerun
cycle (`supervisor.rs:41`). So a call on the coordinator's seam ends the
supervisor's run — an implicit cross-seam side effect. `TestPipeline` emulates
it by having `BranchControl::quit()` fire the same `run_gate` that
`PipelineLifecycle::run()` awaits (`pipeline.rs:257`).

## Constraints (pinned by ADR-0001/0002)

- N handshake failures within `watchdog_window` ⇒ **fail all pending waiters** ⇒
  **full pipeline restart**. A runtime branch reap is peer-caused and must
  **not** feed the watchdog.
- Teardown/quit is **bounded** so a wedged operation cannot stall the actor.
- Watchdog semantics are pinned by paused-clock actor tests and
  `tests/signaling.rs`; changing behavior means updating those tests
  **deliberately**. Mechanism changes near the mailbox get **formally revisited**
  (a new ADR), not locally patched — hence C6 is ADR-first.

## Decision

### 1. `quit` moves `BranchControl` → `PipelineLifecycle`

`BranchControl` shrinks to `ready`, `add_branch`, `remove_branch`. Whole-pipeline
ending now lives entirely on the supervisor's seam alongside `init`/`run`/`end`/
`clean_up`. The coordinator's seam no longer carries a whole-pipeline verb — a
genuine deepening of the two-trait split.

Rationale for a forceful `quit` (not graceful `end`): the watchdog exists for the
suspected-wedge case. `end()` sends an EOS *event* that must propagate through
live dataflow to the bus to bring `run()` down; on a wedged pipeline it may never
resolve, and the shutdown path's abort fallback (`supervisor.rs:101`) only
tolerates a leaked GLib thread because the process is exiting. A *restart* keeps
the process alive, so it must be able to force `main_loop.quit()` directly.

### 2. Watchdog requests a restart over a channel

New channel `restart: mpsc::channel::<()>(1)`, created in `startup.rs::assemble`,
symmetric with the existing `branch_failures` reap channel. Sender → coordinator;
receiver → supervisor.

Coordinator `fail_connection` on trip:

```rust
if self.watchdog.record_failure() {
    tracing::error!("Watchdog tripped: restarting the pipeline");
    self.reset_all();                     // fail pending waiters NOW (pinned semantic)
    let _ = self.restart_tx.try_send(()); // request restart; replaces quit_bounded()
}
```

`try_send` is non-blocking and coalescing (a full buffer means a restart is
already pending), mirroring `branch_failures.try_send`. `quit_bounded()` is
deleted. The coordinator no longer references any whole-pipeline method, so
`spawn_coordinator<P>` now needs only `P: BranchControl` (unchanged bound, but
`quit` is no longer used).

### 3. Supervisor owns ending the run

`run_pipeline_until_stopped` (`supervisor.rs:77`) gains a third `select!` arm:

```rust
tokio::select! {
    res = &mut run_task => RunOutcome::Completed(flatten_join(res)),
    _ = wait_for_shutdown(&mut self.shutdown) => { /* end() + bounded join */ RunOutcome::ShuttingDown }
    Some(()) = self.restart_rx.recv() => {
        // Watchdog asked for a restart: force the run down, then join (bounded).
        let _ = self.pipeline.quit().await;
        match tokio::time::timeout(SHUTDOWN_JOIN_TIMEOUT, &mut run_task).await {
            Ok(joined) => { if let Err(e) = flatten_join(joined) {
                tracing::warn!("Pipeline errored during watchdog restart: {}", e); } }
            Err(_) => { run_task.abort();
                tracing::error!("Pipeline did not stop within {:?} after watchdog quit; abandoning it",
                    SHUTDOWN_JOIN_TIMEOUT); }
        }
        RunOutcome::Restart
    }
}
```

`Some(()) = recv()` uses the dropped-sender-disables-arm idiom already used for
`branch_failures` in the coordinator (`coordinator.rs:245`): if the sender is
dropped (tests that don't wire restart), the arm is disabled, never a busy-loop.

The force-quit **bound relocates** from the coordinator's `teardown_timeout` to
this bounded join (`SHUTDOWN_JOIN_TIMEOUT`; both 5s today). A wedged quit can no
longer stall the coordinator mailbox — the coordinator only does a non-blocking
`try_send`.

### 4. Backoff and watchdog-counter reset unchanged

New `RunOutcome::Restart` variant. In `run()` (`supervisor.rs:54`):

```rust
RunOutcome::Restart => {
    consecutive_failures = 0;   // base-delay rerun, like a clean run
    tracing::info!("Watchdog requested a restart. Reset and rerun the pipeline.");
}
```

This preserves today's behavior: `quit()` resolved `run()` as `Ok`, which reset
the supervisor's run-failure counter, so a watchdog restart used the base delay
(pinned by `quit_restarts_like_a_clean_run`). It does **not** break the loop.

The watchdog counter itself is still cleared by the supervisor's post-restart
`cleanup()` → `signal.reset()` (`supervisor.rs:120`) — unchanged.

## Files touched

- `src/stream/pipeline.rs` — move `quit` between the two trait defs; move
  `TestPipeline::quit` from its `BranchControl` impl to its `PipelineLifecycle`
  impl (body unchanged: `quit_count += 1; run_gate.notify_one()` — now
  same-domain as `run`, no longer cross-trait).
- `src/stream/gst_pipeline.rs` — move `SharablePipeline::quit` from the
  `BranchControl` impl block to the `PipelineLifecycle` impl block (body
  unchanged).
- `src/signal/coordinator.rs` — coordinator holds `restart_tx: mpsc::Sender<()>`;
  trip path sends instead of quitting; delete `quit_bounded`; rework the watchdog
  actor tests (below).
- `src/signal/mod.rs` — `spawn_coordinator` gains a `restart_tx` parameter; every
  call site takes the new arg (`startup.rs`, the `supervisor.rs` and
  `coordinator.rs` test helpers, and the `mod.rs::handle_drives_a_full_handshake`
  test).
- `src/supervisor.rs` — `Supervisor` holds `restart_rx`; `spawn` gains the param;
  new select arm + `RunOutcome::Restart`; rework supervisor tests (below).
- `src/startup.rs::assemble` — create the restart channel; wire sender to
  `spawn_coordinator`, receiver to `Supervisor::spawn`. `P: BranchControl +
  PipelineLifecycle` bound unchanged.
- `docs/adr/0005-watchdog-restart-through-supervisor.md` — new ADR (below).

No changes to `main.rs` wiring beyond what `assemble` needs internally; the
restart channel is an `assemble` internal (like `shutdown`), not a `main.rs`
concern.

## Testing plan

Pinned tests are updated **deliberately** (ADR requirement), preserving intent:

- **Coordinator actor tests** (`coordinator.rs`): `spawn_actor` /
  `spawn_actor_with_reaper` helpers create the restart channel and expose its
  receiver to the test. Watchdog assertions swap the mechanism they observe:
  - `watchdog_trips_after_three_consecutive_failures`:
    `pipeline.snapshot().quit_count == 1` → `restart_rx.try_recv().is_ok()`.
  - `success_between_failures_prevents_the_trip`,
    `reset_clears_the_watchdog_counter`: the no-trip `quit_count == 0` →
    `restart_rx.try_recv()` is empty (`Err(TryRecvError::Empty)`).
  - Other actor tests that assert `quit_count == 0` incidentally (e.g.
    `offer_timeout_fails_only_that_connection`,
    `established_connection_is_reaped_on_branch_failure`) assert the empty
    restart channel instead.
- **`TestPipeline`:** `quit()` on the `PipelineLifecycle` impl; removed from the
  `BranchControl` impl. `quit_count` field retained for supervisor-side use.
- **Supervisor tests** (`supervisor.rs`): `Supervisor::spawn` gains `restart_rx`;
  helpers build a restart channel. `quit_restarts_like_a_clean_run` →
  `restart_request_restarts_like_a_clean_run`, driven by `restart_tx.send(())`
  and asserting `run_count` 1→2 with `cleanup_count == 1`. Tests that don't
  exercise restart pass a channel whose sender they hold (or drop) so the arm
  stays idle.
- **`tests/signaling.rs`** `watchdog_restarts_pipeline_after_consecutive_failures`:
  HTTP-observable behavior is unchanged (three handshake timeouts → restart →
  service recovers). Expected to pass **as-is**; verify, do not rewrite unless it
  observes the mechanism directly.
- **Full gates:** `cargo test --all-targets`, `cargo clippy --all-targets -D
  warnings`, `cargo fmt --check` all green. Browser e2e
  (`tests/browser/run.sh`) as a belt-and-suspenders runtime check — C6 does not
  touch the media path.

## Proposed ADR-0005 (full text, to be committed to `docs/adr/`)

> # 5. Route watchdog restarts through the supervisor's lifecycle seam
>
> Date: 2026-07-10
>
> ## Status
>
> Accepted. Refines the mechanism notes in ADR-0001 (§Consequences, the
> "mechanism near the mailbox is revisited, not patched" clause) and ADR-0002
> (watchdog rows). Does not change the pinned watchdog *semantics*.
>
> ## Context
>
> The signaling plane splits the pipeline into two traits by caller:
> `BranchControl` (the coordinator's per-connection seam) and
> `PipelineLifecycle` (the supervisor's whole-pipeline seam). `quit` sat on
> `BranchControl`, yet its only effect is to end the supervisor's `run()` — a
> cross-seam side effect neither interface documents, and one the `TestPipeline`
> fake must hand-wire (its `BranchControl::quit` fires the `run_gate` its
> `PipelineLifecycle::run` awaits). ADR-0001 flagged that mechanism changes near
> the mailbox get a formal revisit; this is that revisit.
>
> ## Decision
>
> - `quit` moves from `BranchControl` to `PipelineLifecycle`. The coordinator's
>   seam carries only per-connection verbs (`ready`, `add_branch`,
>   `remove_branch`); the supervisor's seam owns the whole-pipeline lifecycle,
>   including forcefully ending a run.
> - On a watchdog trip the coordinator fails all pending waiters and sends a
>   restart *request* over an explicit `mpsc` channel (symmetric with the
>   `branch_failures` reap channel). It no longer ends the run itself.
> - The supervisor's select loop gains a restart arm that force-quits the current
>   run (bounded by the same join timeout as graceful shutdown) and reruns through
>   its normal cleanup/backoff path, treated as a clean restart (base delay).
> - A forceful `quit` (direct `main_loop.quit()`), not a graceful `end` (EOS
>   event), is retained for restart: the watchdog exists for the suspected-wedge
>   case, where EOS may never propagate and the process — unlike at shutdown —
>   stays alive, so the old run must be guaranteed dead before rerunning.
>
> ## Consequences
>
> - Watchdog semantics are unchanged (N failures in window ⇒ fail all waiters ⇒
>   full restart; reaps don't feed the watchdog; base-delay restart). The
>   force-quit *bound* relocates from the coordinator's `teardown_timeout` to the
>   supervisor's bounded join. The coordinator's trip path is now non-blocking
>   (`try_send`), so a wedged quit can never stall the mailbox.
> - The `TestPipeline` cross-trait `run_gate` wiring is gone; `quit` releasing the
>   run is now within the `PipelineLifecycle` domain.
> - Pinned watchdog tests were updated deliberately to observe the restart
>   request instead of a recorded `quit`; intent and assertions are otherwise
>   preserved.

## Non-goals / out of scope

- No change to watchdog thresholds, windows, timeouts, or backoff curve.
- No change to the per-connection reap path (`branch_failures`).
- Not un-serializing the coordinator mailbox (a separate ADR-0001 concern).
- C2b (retire `ready()`) is separate and unaffected.

## Self-review checklist

- **Semantics preserved:** trip → fail waiters → full restart at base delay;
  reaps don't feed the watchdog; teardown/quit bounded. All still hold. ✓
- **Coupling removed:** `quit` no longer on the coordinator's seam; no cross-seam
  side effect; `TestPipeline` no longer hand-wires cross-trait. ✓
- **Bounding retained:** relocated to the supervisor's bounded join. ✓
- **Dropped-sender safety:** `Some(()) = recv()` disables the arm; no busy-loop. ✓
- **Tests updated deliberately, not incidentally:** enumerated above. ✓
