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
