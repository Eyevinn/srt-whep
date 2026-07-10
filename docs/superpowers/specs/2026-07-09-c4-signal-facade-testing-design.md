# C4 — Test the signal plane through `SignalHandle` (design)

**Status:** Approved (design); ready for implementation plan
**Date:** 2026-07-09
**Base:** `main` @ `347d2a0`
**Source card:** `docs/proposals/2026-07-09-architecture-deepening-candidates.md` § C4
**Candidate:** C4 (low-risk, tests-only). Fourth of the remaining arch-deepening
candidates; C1/C2a/C3/C5/C7 already landed.

## Goal

Make the coordinator's actor tests drive the same facade (`SignalHandle`) that
every HTTP route depends on, and make the `Command` enum private to
`src/signal`. Today ~18 actor tests hand-build raw `Command` messages and
oneshot reply channels one level *below* the facade, so the facade is crossed by
exactly one direct test (`mod.rs::handle_drives_a_full_handshake`). A
facade-level regression (wrong reply mapping, wrong error conversion) would
surface only in the slower integration suite.

## Why this is safe

- **Behavior unchanged.** This is a test-surface migration plus an interface
  visibility change. No production control flow moves. The ADR-0001/0002 pinned
  semantics (per-connection failure isolation, watchdog trip/restart, bounded
  teardown, sweep reaping) are re-asserted by the *same test names and
  assertions* — only the plumbing under each assertion changes.
- **Zero files change outside `src/signal/`.** Verified: `Command`,
  `ConnectionInfo`, and the reply-type aliases have no users outside
  `src/signal`; the `pub use messages::{Command, …}` in `mod.rs` is dead public
  surface. `spawn_coordinator` already returns `SignalHandle` (C3), and all
  external callers (`startup.rs`, `supervisor.rs`, `routes/*`) already talk to
  the facade.

## Scope of change (all within `src/signal/`)

### 1. Test helpers return `SignalHandle`

`coordinator.rs`'s test helpers currently return `mpsc::Sender<Command>`:

- `spawn_actor(pipeline, config) -> mpsc::Sender<Command>`
- `spawn_actor_with_reaper(pipeline, config, branch_failures) -> mpsc::Sender<Command>`

Both become thin wrappers over the production `spawn_coordinator`, returning
`SignalHandle`:

```rust
pub(super) fn spawn_actor(pipeline: TestPipeline, config: CoordinatorConfig) -> SignalHandle {
    let (_fail_tx, fail_rx) = mpsc::channel(1); // disconnected reaper
    spawn_coordinator(pipeline, config, fail_rx)
}
pub(super) fn spawn_actor_with_reaper(
    pipeline: TestPipeline,
    config: CoordinatorConfig,
    branch_failures: mpsc::Receiver<BranchId>,
) -> SignalHandle {
    spawn_coordinator(pipeline, config, branch_failures)
}
```

The two shared helpers `establish(&SignalHandle, id)` and
`list_ids(&SignalHandle)` are rewritten to call handle methods (see below).

### 2. Migrate the 18 actor tests to drive the handle

Each `tx.send(Command::X { .., reply })` + `reply_rx.await` pair collapses to a
`handle.method(..).await` call. The mapping falls into three shapes:

- **Immediate replies → direct `.await`.** A call that resolves without waiting
  for a later command (rejections, timeouts that auto-advance to resolution,
  transient/fatal add failures): `handle.create_connection(id).await`,
  `handle.remove_connection(id).await`, etc. Covers: `offer_timeout_*`,
  `transient_pipeline_failure_*`, `not_ready_*`, `unknown_id_*`,
  `failed_add_*`, `failed_delete_*`, `wedged_teardown_*`, `wedged_add_branch_*`,
  and the watchdog failure loops (`watchdog_trips_*`, `success_between_*`,
  `reset_clears_*`).

- **In-flight legs → `tokio::spawn` then `.await` the `JoinHandle`.** When a
  call only resolves once a *later* command arrives (a `create_connection`
  waiting for its offer; an `offer_received` waiting for the answer), spawn it,
  drive the dependency, then await the handle. This is exactly the pattern
  `mod.rs::handle_drives_a_full_handshake` already uses. Covers: `happy_path_*`,
  `answer_timeout_*`, `wrong_state_*` (holds several concurrent in-flight
  requests), `reset_fails_all_waiters_*`, and the "one success" leg of
  `success_between_*`.

- **Abandonment → `tokio::spawn` then `abort()`.** The Trap from the card. The
  single genuine abandonment test,
  `abandoned_whep_client_is_reaped_by_the_sweep`, expresses "browser
  disconnected" by dropping the in-flight future: spawn the `create_connection`,
  `yield_now().await` to let the actor register the branch, then `abort()` the
  task (dropping the future drops the reply receiver, exactly as the raw test's
  `drop(whep_rx)` did). The sweep then reaps the timed-out connection. The
  assertion (pipeline `added == removed == ["a"]`) is unchanged.

  `list_reports_ids_and_state_names` also ends with a `drop(whep_rx)` — but only
  to silence an unused-variable warning, not as an abandonment assertion; its
  in-flight create is spawned and `abort()`ed at the end for the same effect.

**No raw-mailbox escape hatch is needed.** All 18 tests were traced; every
abandonment/concurrency case is expressible through the facade. The card
allowed a thin escape hatch "if one genuinely can't" — none can't.

The three pure `ConnectionState` tests (`fail_waiter_*` ×2,
`transition_table_accepts_only_legal_events`) never touch `Command` and are left
untouched.

### 3. Make `Command` private to `src/signal`

`mod.rs`: `pub use messages::{Command, ConnectionId, ConnectionInfo};` →
`pub use messages::{ConnectionId, ConnectionInfo};`. `Command` stays `pub` inside
the private `messages` module, so it remains reachable from `coordinator.rs`
(`use super::messages::{… Command …}`) but is no longer part of the crate's
public surface. After the test migration, the test module's
`use crate::signal::messages::Command;` (and the now-unused `Coordinator`
import) are removed.

### 4. Unify `ListConnections` / `Reset` into the `request()` shape

The two commands whose replies are bare (not `Result`-shaped) get folded into
the uniform path so all six handle methods flow through `request()`:

- `messages.rs`: add `pub type SnapshotReply = oneshot::Sender<Result<Vec<ConnectionInfo>, SignalError>>;`
  - `ListConnections { reply: oneshot::Sender<Vec<ConnectionInfo>> }` → `{ reply: SnapshotReply }`
  - `Reset { reply: oneshot::Sender<()> }` → `{ reply: UnitReply }`
- `coordinator.rs` handlers: `reply.send(list)` → `reply.send(Ok(list))`;
  `reply.send(())` → `reply.send(Ok(()))`.
- `mod.rs`: the hand-rolled `list_connections`/`reset` bodies collapse to
  `self.request(|reply| Command::ListConnections { reply }).await` and
  `self.request(|reply| Command::Reset { reply }).await`.

**No external behavior change:** both public method signatures already return
`Result<_, SignalError>`; the coordinator always replies `Ok(..)`, so the only
error path stays `Unavailable` on a dropped channel — identical to today.

## Testing

- **Per task / at completion:** `cargo test --all-targets` green (currently
  52 lib + 12 integration; the migrated actor tests keep their names). Clippy
  `-D warnings` + `cargo fmt --check` clean. The framework GStreamer env must be
  exported first (macOS).
- **Semantics guard:** the migration is correct iff every pre-existing actor
  test still passes with the same assertion. Renames are fine; a changed
  assertion is a red flag to stop and reconsider.
- **End-to-end regression guard:** `tests/browser/run.sh` (real-Chrome WHEP
  media check) run once after C4 lands. C4 is tests-only and cannot change the
  runtime media path, so this is a belt-and-suspenders confirmation that the
  build still serves media; it is not expected to be sensitive to this change.
- The `#[ignore]`d `tests/e2e_gstreamer.rs` needs no run for C4 (no
  `gst_pipeline.rs`/`branch.rs` change), but is harmless to run.

## Done when

- The coordinator actor tests construct **no** raw `Command`; they drive
  `SignalHandle`.
- `Command` is private to `src/signal` (absent from `mod.rs`'s `pub use`; no
  external references — already true).
- All six `SignalHandle` methods go through `request()`; `ListConnections` and
  `Reset` carry `Result`-shaped replies.
- Behavior is pinned by the same test names/assertions as before; the full
  suite is green; the browser e2e passes once.

## Out of scope

- Any change to production control flow, error taxonomy, or ADR-pinned
  semantics.
- C6's watchdog→supervisor reshaping (separate candidate; ADR conversation
  first).
- Touching files outside `src/signal/`.
