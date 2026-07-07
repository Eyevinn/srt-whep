# CONTEXT

srt-whep ingests one MPEG-TS-over-SRT stream and re-serves it to WebRTC
viewers over WHEP (server-initiated signaling), fanning it out to each
viewer through a loopback WHIP bridge inside the same process.

## Domain glossary

- **Connection** — one WHEP viewer's signaling lifecycle: offer, answer,
  established, teardown. Owned end-to-end by the coordinator.
- **Branch** — that connection's per-viewer GStreamer elements
  (`whipclientsink` + queues) hot-plugged into the pipeline. One connection,
  one branch.
- **Loopback WHIP** — the in-process `whipclientsink` POSTing its SDP offer
  back to this app's own HTTP server (`/whip_sink/{id}`) instead of to an
  external WHIP endpoint. This is how the WHEP-facing signaling plane gets
  an SDP offer out of the GStreamer pipeline without the pipeline importing
  signaling code.
- **Coordinator** — the signaling actor (`src/signal/coordinator.rs`): a
  single tokio task that owns all connection state and every branch
  add/remove call, serialized through its mailbox.
- **Supervisor** — the restart loop (`src/supervisor.rs`) that runs the
  pipeline, and on EOS or error cleans up, resets signaling, and reruns it
  with backoff, until told to shut down.
- **Watchdog** — a consecutive-failure counter inside the coordinator; N
  handshake failures in a row trip it and force a full pipeline restart
  (`pipeline.quit()`), on the assumption the pipeline itself is wedged.
- **Sweep** — the coordinator's periodic (1s) deadline reaper: on each tick
  it expires any connection past its offer/answer deadline, replies `Err`
  to a waiting handler if one is still attached, removes the branch, and
  drops the entry — including for abandoned clients whose HTTP handler
  future was already dropped.

## Terminology map: one lifecycle, three vantage points

The same viewer lifecycle is named differently depending on which layer is
looking at it. This is intentional drift left over from three build passes,
not three different things:

| Layer | Term used | Where |
|---|---|---|
| HTTP surface | **channel** | `/channel`, `/channel/{id}` routes |
| Signal plane | **connection** | `src/signal` (`ConnectionId`, `ConnectionState`) |
| Stream plane | **branch** | `src/stream` (`BranchControl::add_branch`/`remove_branch`) |

A "channel" a client POSTs to, the "connection" the coordinator tracks, and
the "branch" the pipeline hot-plugs are the same underlying thing seen from
the wire, the signaling plane, and the stream plane respectively. When
writing code or docs: use **channel** only in HTTP-facing text (routes,
client-facing docs), **connection** in `src/signal` and anything about
signaling state, **branch** in `src/stream` and anything about GStreamer
elements. Do not introduce a fourth term.

## Module map

- `src/signal` — the coordinator actor; sole owner of connection state and
  the only caller of branch add/remove. HTTP handlers talk to it through
  `SignalHandle` (mpsc + oneshot replies), never touching pipeline state
  directly.
- `src/stream` — the GStreamer pipeline. Two seams split by caller:
  `BranchControl` (the coordinator's view: `ready`, `add_branch`,
  `remove_branch`, `quit`) and `PipelineLifecycle` (the supervisor's view:
  `init`, `run`, `end`, `clean_up`). `src/stream/gst_pipeline.rs` holds the
  real implementation; `TestPipeline` in `src/stream/pipeline.rs` is the
  recording fake both traits share for tests.
- `src/supervisor.rs` — the restart loop: `init` → `run` → `cleanup` (pipeline
  `clean_up` + `signal.reset()`) → backoff → repeat, until a `watch`
  channel signals shutdown.
- `src/startup.rs` — the actix route table and `Application::assemble`,
  which wires the coordinator, the supervisor, and the HTTP server together
  in one place; used by `main`, the HTTP integration tests, and the
  GStreamer e2e test. One Ctrl-C, handled once in `main`, stops all three.
- `src/routes` — thin HTTP adapters: parse/validate the request, call
  `SignalHandle`, map the result to a status code. No pipeline or state
  knowledge lives here.
- `src/domain` — `SessionDescription`, a parse-don't-validate newtype: SDP
  facts (e.g. `is_sendonly`) are queryable on the type; handlers decide
  direction *policy* (WHEP PATCH rejects sendonly, WHIP POST rejects
  non-sendonly).

## Decided constraints

These are closed decisions; see `docs/adr/0001-signaling-plane-rebuild.md`
before proposing to revisit any of them:

- The loopback WHIP bridge stays. `src/stream` never imports `src/signal`;
  an in-process signaller was considered and explicitly deferred.
- Branch add/remove calls are serialized inside the coordinator's mailbox,
  alongside connection state. Accepted trade-off: a slow branch operation
  stalls the rest of the signaling plane (list/remove/sweep queue behind
  it).
- Failure isolation is per-connection, with watchdog fallback to a full
  pipeline restart. Both are pinned by tests — don't change the semantics
  without updating them.

## Dev environment

On macOS, `cargo build`/`test`/`run` need the GStreamer dylib path exported
first, or you get a linker/runtime error. See the OSX build section of
`README.md` for the full env var list (`PATH`, `PKG_CONFIG_PATH`,
`GST_PLUGIN_PATH`, `DYLD_FALLBACK_LIBRARY_PATH`).
