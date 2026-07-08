# Signaling-Plane Rebuild — Design

**Date:** 2026-07-07
**Status:** Approved (design review with Kun, 2026-07-07)
**Scope:** `src/routes/`, `src/domain/app_state.rs` (deleted), new `src/signal/`, small rewires in `main.rs`/`startup.rs`/`src/utils.rs`. No GStreamer element logic changes.

## Context

srt-whep bridges one SRT/MPEG-TS input to WebRTC viewers using WHEP (server-initiated). Internally, each viewer connection hot-plugs a `whipclientsink` into the GStreamer pipeline; that sink POSTs its SDP offer to the app's own HTTP server (`/whip_sink/{id}`), and two concurrently-blocked HTTP handlers hand SDPs to each other through a shared `AppState` (HashMap + `event_listener::Event` + `timed_locks`).

Problems this rebuild addresses:

1. **Complexity** — the rendezvous is spread across two handlers and `AppState`, guarded by lock-timeout workarounds; state transitions are implicit.
2. **No working tests** — `tests/sdp_exchange.rs` is stale: it targets routes that no longer exist (`/health_check`, `/whip_sink` without id), asserts status codes the handlers don't return, and asserts `a=setup:active/passive` rewriting that production code never performs (`set_as_active`/`set_as_passive` are dead code).
3. **Fragile failure handling** — any single failed handshake calls `app_state.reset()` (wipes *all* connections) and `pipeline.quit()` (restarts the pipeline, dropping every connected viewer). An abandoned browser (actix drops the handler future on client disconnect) leaks its connection entry and pipeline branch.

## Decisions (from design review)

| Question | Decision |
|---|---|
| Keep loopback-WHIP bridge or in-process signaller? | **Keep loopback bridge**; rebuild internals only |
| Failure blast radius | **Per-connection isolation + watchdog fallback** (N consecutive failures → full pipeline restart) |
| Test depth | **Unit + HTTP integration + `#[ignore]` GStreamer e2e** |
| Core structure | **Coordinator actor** (single task owns all state; handlers send commands) |

## Architecture

New module replaces the `AppState` rendezvous:

```
src/signal/
  mod.rs           // SignalHandle: clone-able mpsc command sender, spawn fn
  coordinator.rs   // actor loop + connection state machine
  messages.rs      // Command enum + reply types
  watchdog.rs      // consecutive-failure counter
```

- The **coordinator actor** is one tokio task owning `HashMap<ConnectionId, ConnectionState>`. It is the *single owner of connection lifecycle*: state entries **and** pipeline branch calls (`add_connection`/`remove_connection`) happen only inside the actor, serialized by its mailbox. No shared locks; no lock held across await anywhere.
- HTTP handlers become thin adapters: parse/validate → `SignalHandle::send(command)` → await oneshot reply → map to HTTP response.
- The actor is generic over `PipelineBase`, holding a pipeline handle. Production: `SharablePipeline`; unit tests: a recording mock.
- Dependencies: `event-listener` dropped. `timed_locks` remains only in the pipeline module.
- `SharableAppState` is deleted; `run()` in `startup.rs` takes a `SignalHandle` instead. `main.rs` spawns the actor at startup.

## Connection state machine

```
POST /channel          offer from whipsink        PATCH answer
      │                       │                        │
      ▼                       ▼                        ▼
 AwaitingOffer ────────► AwaitingAnswer ────────► Established ──► (DELETE) removed
      │ deadline (10s)        │ deadline (10s)
      ▼ timeout               ▼ timeout
   reply Err to waiter + remove pipeline branch + drop entry + watchdog.record_failure()
```

States:

- `AwaitingOffer { whep_reply: oneshot::Sender<Result<SessionDescription, SignalError>>, deadline }` — created by `CreateConnection` after the actor calls `pipeline.ready()` and `pipeline.add_connection(id)` successfully.
- `AwaitingAnswer { whip_reply: oneshot::Sender<...>, deadline }` — entered when the offer arrives; the offer is delivered to `whep_reply`.
- `Established { since }` — answer delivered to `whip_reply`; watchdog reset to 0. Removed on DELETE or pipeline reset.

Commands (`messages.rs`):

| Command | Sent by | Reply |
|---|---|---|
| `CreateConnection { id, reply }` | WHEP POST handler | `Result<SessionDescription>` (the offer) when it arrives or deadline hits |
| `OfferReceived { id, sdp, reply }` | WHIP sink POST handler | `Result<SessionDescription>` (the answer) when it arrives or deadline hits |
| `AnswerReceived { id, sdp, reply }` | WHEP PATCH handler | `Result<()>` immediately |
| `RemoveConnection { id, reply }` | DELETE handler | `Result<()>` |
| `ListConnections { reply }` | GET /list handler | `Vec<(ConnectionId, StateName)>` |
| `Reset { reply }` | supervisor (`PipelineGuard`) on pipeline restart | `()` after all waiters get `Err` and map is cleared |

Rules:

- **Timeout sweep**: the actor loop `tokio::select!`s over the mailbox and a 1 s `tokio::time::interval`; each tick expires entries past their deadline (reply `Err(Timeout)` if a waiter is still attached, remove pipeline branch, drop entry, `watchdog.record_failure()`). This also reaps abandoned clients whose handler futures were dropped — no reliance on handlers still being alive.
- **Wrong-state safety**: `OfferReceived` on a connection not in `AwaitingOffer` → `Err(WrongState)` (409); any command on an unknown id → `Err(NotFound)` (404). State is never mutated on rejected commands. `oneshot` replies make double-sends structurally impossible.
- **Config**: `CoordinatorConfig { offer_timeout: 10s, answer_timeout: 10s, watchdog_threshold: 3, sweep_interval: 1s }` with `Default`. No new CLI flags in this rebuild.

## Failure model

- **Per-connection**: a handshake timeout or explicit failure cleans up only that connection (branch + entry). Other viewers are unaffected.
- **Watchdog**: counts *consecutive* handshake failures; any connection reaching `Established` resets it. At `watchdog_threshold` (3), the actor assumes the pipeline is wedged: replies `Err` to all pending waiters, clears the map, resets the counter, and calls `pipeline.quit()`. The existing supervisor loop in `main.rs` rebuilds the pipeline as today.
- **Pipeline-level errors** (GStreamer bus error, SRT EOS) keep their current restart path. `PipelineGuard::cleanup` sends `Reset` to the actor instead of calling `app_state.reset()`.

## HTTP surface

Routes, methods, and the loopback `whipclientsink → POST /whip_sink/{id}` contract are unchanged. Handler bodies shrink to adapters. Deliberate refinements:

| Case | Today | New |
|---|---|---|
| `POST /channel`, input stream not ready | 400 | **503 + `Retry-After: 3`** |
| Handshake timeout (either side) | 400 | **503** |
| Unknown connection id (PATCH/DELETE/whip) | 400 | **404** |
| Command in wrong state (duplicate offer/PATCH) | n/a (races) | **409** |
| Invalid SDP (parse/direction) | 400 | 400 (unchanged) |
| Happy path codes/headers | 201+Location / 204 / 200 | unchanged |

- Errors consolidate into one `SignalError` enum with a single `actix_web::ResponseError` impl, replacing the `MyError`-inside-`SubscribeError` nesting for routes. `MyError` remains for the pipeline/domain internals it still serves.
- `GET /list` returns `[{ id, state }]` instead of bare ids.
- Dead code deleted: `SessionDescription::set_as_active` / `set_as_passive`.
- `SessionDescription` validation (v=0, sendonly/recvonly) unchanged; WHEP PATCH still rejects sendonly SDP, WHIP POST still rejects non-sendonly SDP. Trickle-ICE PATCH remains unsupported (400 with clear message; the old TODO is resolved by making the rejection explicit).

## Testing

All three layers replace `tests/sdp_exchange.rs`, which is deleted.

**1. Unit — actor state machine** (`src/signal/` `#[cfg(test)]` + a `MockPipeline` recording calls, controllable `ready()`; `tokio::time::pause()`/`advance()` for instant timeouts):

- Happy path: create → offer → answer; replies delivered in order; watchdog reset.
- Offer timeout: whep waiter gets `Err(Timeout)`; `remove_connection` called on mock; entry gone; watchdog incremented.
- Answer timeout: same on the whip side.
- Duplicate offer → 409-mapped error, state unchanged. Late PATCH after timeout → `NotFound`.
- Unknown id on every command → `NotFound`.
- Interleaved concurrent connections stay independent.
- Abandoned client: drop the `CreateConnection` reply receiver; sweep still cleans branch + entry at deadline.
- Watchdog: 3 consecutive failures → `quit()` called on mock, map cleared, pending waiters get `Err`; a success in between resets the counter.
- `Reset`: all waiters get `Err`, map cleared.
- `pipeline.ready() == false` → `CreateConnection` replies `Err(NotReady)` without creating state.

**2. HTTP integration** (`tests/signaling.rs`; real `run()` server + mock pipeline; test client plays the whipclientsink role against `/whip_sink/{id}`, mirroring production loopback):

- Full WHEP↔WHIP exchange: POST /channel (empty body) → 201 + Location + offer; POST /whip_sink/{id} with sendonly offer → 201 + answer after PATCH; PATCH /channel/{id} with recvonly answer → 204.
- Invalid SDPs → 400 (table-driven, as the old test attempted).
- Not-ready pipeline → 503 + Retry-After.
- PATCH/DELETE unknown id → 404.
- **Isolation**: let one handshake time out (no whip POST) → 503; a subsequent full handshake succeeds; mock records exactly one branch removal.
- Non-empty body on POST /channel → 400 (client-initiated WHEP unsupported).
- OPTIONS /channel CORS/Accept-Post contract; GET /list shows states; DELETE removes and 200s.

**3. E2E — `#[ignore]`, requires GStreamer** (`tests/e2e_gstreamer.rs`, run with `cargo test -- --ignored`):

Scope is the **wedge risk** (dynamic branch add/remove on a live pipeline), not media playout:

- In-test SRT source: `videotestsrc is-live=true ! x264enc ! mpegtsmux ! srtsink` (listener), app connects as caller.
- Real `SharablePipeline` + real HTTP server; POST /channel and assert a real SDP offer arrives from the real whipclientsink.
- No scripted answers: feeding a canned SDP answer to a real whipclientsink triggers DTLS/ICE against a nonexistent peer, which can post a bus error and kill the pipeline — a false failure. Instead: abandon handshakes (timeout path), DELETE others, repeat several cycles; assert the pipeline remains `PLAYING` and later connections still receive offers (branch add/remove does not wedge the pipeline).
- Full media verification stays manual (WHEP player), as today.

## Migration steps (implementation order)

1. Add `src/signal/` (messages, watchdog, coordinator) + unit tests — no wiring yet; `cargo test` green.
2. Rewire: `startup.rs` routes take `SignalHandle`; rewrite the four handlers as adapters; `main.rs` spawns the actor; `PipelineGuard` sends `Reset`.
3. Delete `src/domain/app_state.rs`, dead SDP methods, `event-listener` dep; consolidate route errors into `SignalError`.
4. Replace `tests/sdp_exchange.rs` with `tests/signaling.rs`.
5. Add `tests/e2e_gstreamer.rs` (`#[ignore]`).

Each step compiles and passes tests independently.

## Out of scope

- In-process signaller (replacing the loopback WHIP hop) — revisit later; this design shrinks the surface it would replace.
- New CLI flags for timeouts/threshold.
- Trickle ICE, client-initiated WHEP, media-level e2e assertions.
- The uncommitted `--srt-latency`/`--tsdemux-latency` work in the tree (separate change, unrelated).
