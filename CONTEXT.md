# CONTEXT

srt-whep ingests one MPEG-TS-over-SRT stream and re-serves it to WebRTC
viewers over WHEP (server-initiated signaling), fanning it out to each
viewer through a loopback WHIP bridge inside the same process.

> **New to the code?** [`docs/connection-lifecycle.md`](docs/connection-lifecycle.md)
> walks one viewer through the handshake step by step. This file is the
> reference map; [`docs/architecture-evolution-shared-lock-to-actor.md`](docs/architecture-evolution-shared-lock-to-actor.md)
> is the *why* behind the coordinator-actor design.

## Domain glossary

- **Connection** — one WHEP viewer's signaling lifecycle: offer, answer,
  established, teardown. Owned end-to-end by the coordinator.
- **Branch** — that connection's per-viewer GStreamer elements
  (`whipclientsink` + queues) hot-plugged into the pipeline. One connection,
  one branch.
- **Egress chain** — the per-media output chain built once the demux
  announces the stream's pads (`src/stream/egress.rs`, the codec table):
  video is parsed (`h264parse`/`h265parse`) into a named output tee; audio
  is transcoded AAC → Opus into its own tee; unknown media is an error.
  The named tees are the attach points for Branches, and the terminating
  fakesink keeps each chain consuming — and pops EOS onto the bus when the
  SRT input closes — even with zero viewers attached.
- **Loopback WHIP** — the in-process `whipclientsink` POSTing its SDP offer
  back to this app's own HTTP server (`/whip_sink/{id}`) instead of to an
  external WHIP endpoint. This is how the WHEP-facing signaling plane gets
  an SDP offer out of the GStreamer pipeline without the pipeline importing
  signaling code.
- **Coordinator** — the signaling actor (`src/signal/coordinator.rs`): a
  single tokio task that owns all connection state and every branch
  add/remove call, serialized through its mailbox.
- **Parked waiter** — a oneshot reply sender held *inside* a connection's
  state instead of being answered right away. The WHEP `POST /channel` reply
  parks in `AwaitingOffer` until the whipsink's offer arrives; the loopback
  WHIP `POST` reply parks in `AwaitingAnswer` until the browser's answer
  arrives. Delivering the SDP later completes the long-held HTTP request. See
  [`docs/connection-lifecycle.md`](docs/connection-lifecycle.md).
- **Supervisor** — the restart loop (`src/supervisor.rs`) that runs the
  pipeline and, when it stops — on EOS, on error, or on a watchdog restart
  request — cleans up, resets signaling, and reruns it with backoff, until
  told to shut down. It owns `pipeline.quit()`, the forceful teardown used to
  end a run on a watchdog restart.
- **Watchdog** — a consecutive-failure counter inside the coordinator; N
  handshake failures in a row trip it. On a trip the coordinator fails all
  waiters and sends a restart request to the supervisor over an mpsc channel
  (it does not quit the pipeline itself); the supervisor force-quits and
  reruns, on the assumption the pipeline itself is wedged. See
  [`docs/adr/0005`](docs/adr/0005-watchdog-restart-through-supervisor.md).
- **Sweep** — the coordinator's periodic (1s) deadline reaper: on each tick
  it expires any connection past its offer/answer deadline, replies `Err`
  to a waiting handler if one is still attached, removes the branch, and
  drops the entry — including for abandoned clients whose HTTP handler
  future was already dropped.
- **Reap** — cleanup triggered by the pipeline's bus watch reporting a
  branch's *runtime* failure (`reap_branch`): the connection is dropped and its
  branch detached. Unlike the sweep, a reap deliberately does **not** feed the
  watchdog — a dead peer is a fact about one viewer, not a pipeline-health
  signal.
- **Termination** — the coordinator's single owner of "a connection is
  ending" (`terminate(id, reason)`). Every death path — a client DELETE, a
  sweep expiry, a peer vanishing mid-handshake, a bus reap, a reset — names
  its reason, and one policy table maps that reason to what happens: how a
  parked waiter is failed, whether the branch teardown gates the death, and
  whether the watchdog is fed. The sweep and reap rows keep their pinned
  semantics; "a reap does not feed the watchdog" is a value in that table,
  not a comment. On the wire, termination answers requests still in flight
  with **410 Gone** ("it existed; it just ended"); a later request naming
  the id gets a plain **404** ("never knew it") — the map holds no
  tombstones.
- **Reset** — the restart contract between the supervisor and the
  coordinator: after *every* pipeline stop (error, EOS, watchdog restart,
  shutdown) the supervisor resets signaling, and the coordinator fails all
  in-flight waiters and clears the connection map — no waiter outlives the
  pipeline run it was created against. Bounded by a timeout at the call
  site so a wedged coordinator can't hang the restart loop. The supervisor
  holds this as a one-method capability (`ResetSignal`, carried by
  `ResetHandle`); the route-facing `SignalHandle` cannot reset.

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
  directly; the supervisor holds the separate one-method `ResetHandle`
  (`ResetSignal`), so only it can reset.
- `src/stream` — the GStreamer pipeline. Two seams split by caller:
  `BranchControl` (the coordinator's per-connection view: `ready`,
  `add_branch`, `remove_branch`) and `PipelineLifecycle` (the supervisor's
  whole-pipeline view: `init`, `run`, `end`, `clean_up`, `quit`).
  `src/stream/gst_pipeline.rs` holds the real implementation, with the
  codec decisions split out into `src/stream/egress.rs` (which parser or
  transcode chain each demuxed media type gets); `TestPipeline` in
  `src/stream/pipeline.rs` is the recording fake both traits share for
  tests.
- `src/supervisor.rs` — the restart loop: `init` → `run` → `cleanup` (pipeline
  `clean_up` + `signal.reset()`) → backoff → repeat, until a `watch`
  channel signals shutdown. Its `select!` also carries a restart arm: a
  watchdog request (a separate mpsc) force-quits the current run
  (`pipeline.quit()`) and reruns it through the same path at base delay.
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

The `whipclientsink` is **not** compiled into this binary — it comes from the
`rswebrtc` plugin the GStreamer installation provides, resolved via
`GST_PLUGIN_PATH` at runtime. Keep that path pointed at one GStreamer install:
a second, higher-versioned `rswebrtc` there wins the registry and can break the
WebRTC media path. See [`docs/adr/0003`](docs/adr/0003-webrtc-plugin-from-installation.md).

## Parallel sessions — use an isolated worktree when possible

This repo is one shared working directory with one git HEAD and one index.
When more than one Claude session (or a dispatched subagent) acts on it at
once, that shared state collides: a broad `git add` sweeps another session's
uncommitted work into the wrong commit, and a branch switch ping-pongs HEAD
between agents. This has happened here more than once, so whenever a session
may overlap with another actor, isolate the work.

- **Commit or stash WIP promptly first** — a commit lives on your branch ref
  and survives HEAD moving under you.
- **Adopting an existing branch:** `git worktree add .claude/worktrees/<branch>
  <existing-branch>` checks that branch out into its own directory and locks
  it (no other agent can move a branch that is checked out in a worktree).
  `.claude/worktrees/` is already git-ignored here — no `.gitignore` change
  needed. Then switch the session in with the `EnterWorktree` tool
  (`path=<absolute worktree path>`).
- **Fresh work:** `EnterWorktree name=<branch>`, or the
  `superpowers:using-git-worktrees` skill, creates a new branch in its own
  worktree. Note `EnterWorktree name=...` branches off origin/main or HEAD, so
  use `git worktree add <path> <existing-branch>` when you need to adopt work
  that already exists on a branch.
- **Dispatched subagents / SDD:** run implementers with the Agent tool's
  `isolation: "worktree"` so they work on an isolated copy and cannot touch the
  main working tree.

**Git discipline (even without a worktree):** never `git add -A`, `git add .`,
or `git commit -a` — stage only the explicit paths you changed. Avoid branch
ops (`checkout`/`switch`/`reset`/`rebase`/`stash`) on a shared checkout you do
not own. If `git status` shows changes you did not make, STOP and report —
another session may be working here. Commit or push only when the user asks.

**Finishing while another agent holds the shared checkout:** do not "merge
locally" — a local merge runs `git checkout main` and yanks the shared checkout
out from under the other agent. Push your branch and open a PR instead.
