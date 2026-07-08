# 2. Signaling-plane hardening: reaper, bounded teardown, windowed watchdog

**Status:** Accepted, 2026-07-08

## Context

The signaling-plane rebuild (ADR 0001) left several gaps found in review.
This ADR records the decisions taken to close them. They refine, and in a
couple of places deliberately revise, behavior pinned by ADR 0001.

## Decisions

| Question | Decision |
|---|---|
| A per-viewer branch that errors at runtime (peer gone, whipsink failed) — who reaps it? | **The pipeline bus watch reports it to the coordinator over a dedicated channel; the coordinator drops the connection and detaches the branch.** ADR 0001 said an abandoned client "leaked its connection entry and pipeline branch with no reaper"; only mid-handshake connections were swept. Established connections are now reaped too, triggered by the bus error rather than by age (so `Established { since }` is still not aged out). The channel is separate from the command mailbox, so it never gates coordinator shutdown, and `src/stream` still holds only a plain `mpsc::Sender<String>` — no dependency on `src/signal`, preserving the acyclic module graph from ADR 0001. |
| Bus-watch error containment scope | **Contain errors from ALL of a viewer's branch elements — the whip sink AND its per-media queues (`video-queue-{id}`/`audio-queue-{id}`).** The rebuild only matched `whip-sink-*`, so a dying branch's own queue (a direct child of the pipeline) posted an "Internal data stream error" as itself and quit the whole main loop — the exact single-bad-peer wedge the containment code existed to prevent. The core (non-branch) queues are named `video-queue`/`audio-queue` (no id suffix); their errors stay fatal. |
| Serialized branch calls stalling the mailbox (the trade-off ADR 0001 flagged) | **Bound each teardown (`remove_branch`/`quit`) with a timeout instead of un-serializing.** ADR 0001 said un-serializing would require revisiting it; we did not un-serialize. A wedged teardown now surfaces as a retryable error after `CoordinatorConfig::teardown_timeout` instead of stalling every command, the sweep, and the watchdog indefinitely. The supervisor's `signal.reset()` on cleanup is bounded the same way. |
| Watchdog scope | **Give the watchdog a time window (decay) instead of a windowless consecutive counter.** ADR 0001's watchdog counted *consecutive* failures with no time bound, so a few client-caused abandoned handshakes spread over hours would eventually force a full pipeline restart and drop every established viewer. Only failures within `CoordinatorConfig::watchdog_window` of each other now accumulate toward a trip; success still resets. A runtime branch reap is peer-caused and deliberately does **not** feed the watchdog. |
| DELETE ordering | **Remove the branch first; only drop the map entry on success.** The rebuild removed the entry before `remove_branch`, so a failed/transient DELETE returned 404 on retry and leaked the branch. On failure the entry is re-inserted so DELETE stays retryable. |
| Half-attached branch on `add_branch` failure | **Detach it before replying with the error.** The id was never inserted into the map, so DELETE could never reach it; `detach` tolerates a half-built branch. |
| Termination signals | **Re-establish graceful shutdown on SIGTERM and SIGQUIT alongside SIGINT.** `HttpServer::disable_signals()` removed actix's own handlers and only SIGINT (`ctrl_c`) was re-established, so `docker stop` / k8s SIGTERM hard-killed with no HTTP drain / EOS / NULL-state cleanup. |
| `GET /list` wire contract | **Keep the new shape (breaking change, documented here).** See below. |
| `SharablePipeline::print()` dot-graph dump | **Removed as dead code.** `GET /list` no longer calls it and nothing else did; there is no debug trigger for it anymore. Re-add a dedicated debug endpoint later if the dot dump is wanted. |

## `GET /list` — breaking change

`GET /list` changed from a JSON **array of connection-id strings**:

```json
["id-1", "id-2"]
```

to a JSON **array of objects** carrying each connection's coordinator state:

```json
[{"id": "id-1", "state": "established"}, {"id": "id-2", "state": "awaiting_offer"}]
```

`state` is one of `awaiting_offer`, `awaiting_answer`, `established`. This is
intentional (it surfaces the coordinator's per-connection state machine) and
ships without a compatibility path or versioning: there is no known consumer
of the old array shape. A consumer that needs the ids only should read the
`id` field of each object. If a real back-compat need surfaces, add the old
shape behind a query parameter or a separate endpoint rather than reverting.

## Consequences

- Established connections now have a death path (the bus reaper), but it is
  event-driven, not age-based: a genuinely idle-but-alive viewer is never
  reaped, and a viewer whose branch never posts a bus error still relies on
  DELETE / the handshake sweep as backstops.
- `CoordinatorConfig` gains `watchdog_window` and `teardown_timeout`; like the
  rest of the struct (ADR 0001) they remain hardcoded via `Default`, not CLI
  flags.
- The bounded teardown is a mitigation, not a removal, of ADR 0001's
  serialized-mailbox trade-off: teardowns still run on the actor's critical
  path, just no longer unboundedly.
