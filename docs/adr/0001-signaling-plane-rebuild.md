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
  behind it in the same mailbox. No operational data yet on how often this
  matters; if it does, un-serializing branch calls requires formally
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

## Future work — revisit the loopback bridge with `whepserversink`

Not decided; a breadcrumb for whoever picks up the "removing the loopback
hop" design pass named in the Consequences above.

The whole loopback WHIP bridge exists because this app needs *server-initiated*
WHEP (the client POSTs `/channel` and the server hands back the SDP **offer** —
see `docs/whep.md`), but the only egress element available when it was built
was `whipclientsink`, a WHIP *client*. So each viewer's `whipclientsink` is
pointed back at this app's own `/whip_sink/{id}` route to coax a server-side
offer out of a client-initiated element (see `src/stream/branch.rs`).

The framework's `rswebrtc` (0.15.2 as installed — see
[ADR 0003](0003-webrtc-plugin-from-installation.md)) now ships
**`whepserversink`** ("WebRTC sink with WHEP server signaller") — a native
*server-initiated* WHEP sink that does exactly what the loopback trick
emulates. Adopting it could delete the loopback hop, the `/whip_sink/{id}`
route, and much of the per-branch signaller wiring, collapsing the internal
network round-trip per connection.

This stays deferred, not decided. A design pass would need to check:

- Whether `whepserversink`'s fan-out model fits this app's per-viewer
  **branch** model (one sink hot-plugged per connection) or expects to own
  its own HTTP server / multiple consumers per sink.
- How its signaller surfaces the offer/answer exchange, and whether the
  coordinator can still own connection lifecycle (offer/answer timeouts,
  sweep, watchdog) around it — the actor model from this ADR should survive.
- Whether it keeps `src/stream` free of `src/signal` (the acyclic-module-graph
  reason the loopback bridge was kept in the first place).
- The version-skew risk from binding `rswebrtc`'s Rust types into this binary
  again, which ADR 0003 deliberately backed away from — prefer driving it via
  stable GObject properties, as `branch.rs` now does for `whipclientsink`.

If pursued, this supersedes the "keep the loopback bridge" decision above and
should be recorded in a new ADR.
