# 1. Signaling-plane rebuild: coordinator actor over shared-state rendezvous

**Status:** Accepted, 2026-07-07

## Context

Each WHEP viewer connection was previously rendezvoused through a shared
`AppState` (a `HashMap` guarded by `timed_locks`, with `event_listener::Event`
used to wake waiters) that two HTTP handlers mutated directly. This spread
connection lifecycle across two handlers and a shared-state struct, with
implicit state transitions; had no working tests (`tests/sdp_exchange.rs`
targeted routes and behaviors that no longer existed); and had a fragile
failure model — any single failed handshake reset *all* connections and
restarted the whole GStreamer pipeline, and an abandoned client (dropped
handler future) leaked its connection entry and pipeline branch with no
reaper.

Full design and rationale: `docs/superpowers/specs/2026-07-07-signaling-plane-rebuild-design.md`.

## Decisions

| Question | Decision |
|---|---|
| Keep the loopback-WHIP bridge, or move to an in-process signaller? | **Keep the loopback bridge.** It keeps `src/stream` free of any dependency on `src/signal`, preserving an acyclic module graph. An in-process signaller is explicitly deferred, not rejected. |
| Who owns connection state and pipeline branch calls? | **A single coordinator actor** (`src/signal/coordinator.rs`): one tokio task owns `HashMap<ConnectionId, ConnectionState>` *and* is the only caller of `add_branch`/`remove_branch`, both serialized through its mailbox. No shared locks; nothing is held across an await. |
| Failure blast radius | **Per-connection isolation, with watchdog fallback.** A handshake timeout or failure cleans up only that connection. A watchdog counts *consecutive* failures across connections; at threshold it assumes the pipeline is wedged, fails all pending waiters, and force-restarts the pipeline. |
| Coordinator timeout/threshold configuration | **No new CLI flags in this rebuild.** `CoordinatorConfig` (offer/answer timeouts, watchdog threshold, sweep interval) is hardcoded via `Default` for now. This was a scope decision to keep the rebuild bounded, not a permanent ban — exposing `CoordinatorConfig` through CLI flags later is fair game. |

## Consequences

- Serializing branch calls inside the coordinator's mailbox is an accepted
  trade-off: a slow GStreamer branch operation (add/remove) stalls the rest
  of the signaling plane, since `list`/`remove`/the periodic sweep all queue
  behind it in the same mailbox. This has not been observed to matter in
  practice; if it does, un-serializing branch calls requires formally
  revisiting this ADR, not a local patch.
- Per-connection isolation and watchdog semantics are pinned by unit tests
  (paused-clock actor tests) and HTTP integration tests
  (`tests/signaling.rs`) — changing this behavior means updating those
  tests deliberately, not incidentally.
- The loopback WHIP hop remains a real (if internal) network round-trip per
  connection; removing it later is possible but out of scope here and would
  need its own design pass.
- `CoordinatorConfig` being hardcoded means operators cannot currently tune
  handshake timeouts or the watchdog threshold without a code change and
  rebuild.
