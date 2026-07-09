# Architecture Deepening Candidates — Handover

**Status:** C1 + C2a + C5 + C7 **landed on main** (2026-07-09, commits `b7184ad`…`316b9f8`); C2b, C3, C4, C6 not started
**Date:** 2026-07-09
**Base commit:** c93f9b4 (line numbers below refer to this commit). ⚠ **Now partly stale:**
main is at `55e4cd8` after C1+C2a. Those two landed touched `src/stream/naming.rs`
(new), `branch.rs`, `gst_pipeline.rs`, `src/signal/coordinator.rs`, and
`src/stream/pipeline.rs`, so line numbers for candidates that touch those files
(C3, C4, C5, C6) have shifted — re-verify against `main` before editing. C7's file
(`src/domain/session_description.rs`) is untouched.
**Source:** architecture review pass (Explore sweep over all of `src/` + `tests/`, filtered against ADRs 0001–0004 and the 2026-07-08 proposal)

## Progress log

- **2026-07-09 — C1 + C2a landed.** Designed, planned, implemented via
  subagent-driven development (per-task review + a whole-branch review), and
  fast-forward merged to `main` (`b7184ad`…`55e4cd8`).
  - Design spec: `docs/proposals/2026-07-09-c1-c2a-naming-and-add-branch-cleanup.md`
  - Implementation plan: `docs/proposals/2026-07-09-c1-c2a-implementation-plan.md`
  - **C1**: `src/stream::naming` now owns all element names; branch names derive
    from core stems so the core-vs-branch classifier invariant holds by
    construction (pinned by a const-based test).
  - **C2a**: `add_branch` detaches its own half-attached branch; the
    coordinator's `matches!(Fatal)` guess is gone. The whole-branch review
    caught that the move had dropped the cleanup's `teardown_timeout` bound —
    fixed in `f78a3d4` (coordinator now bounds `add_branch`; ADR 0002 row updated).
  - Verified: `cargo test --all-targets` green (53 lib + 12 integration, 0
    warnings); the ignored GStreamer e2e passed once in isolation.
  - **Remaining after this entry:** C2b, C3, C4, C5, C6, C7 (see suggested order below).

- **2026-07-09 — C5 landed** (`04f464b`). Named `DEFAULT_*` consts (unit in
  the name) are now the single source for the coordinator's six defaults;
  both `Default for CoordinatorConfig` and the `CoordinatorArgs` clap
  `default_value_t` attributes read from them. The drift-guard test
  (`coordinator_args_default_to_the_hardcoded_config`) is deleted — the
  property it pinned now holds by construction. `sweep_interval`'s
  secs-vs-ms representation drift is resolved (both ms). No behavior change;
  `cargo test --all-targets` green (52 lib + 12 integration), clippy
  `-D warnings` + fmt clean. Done directly (no subagent flow — pure
  mechanical refactor, card was already the spec).
  - **Remaining after this entry:** C2b, C3, C4, C6, C7.

- **2026-07-09 — C7 landed** (`316b9f8`). **Verification gate resolved to
  outcome (a):** `SdpOffer::parse` rejects non-sendonly SDP and
  `SdpAnswer::parse` rejects sendonly SDP; both newtypes have a private inner
  field constructed only in their own `parse`, so direction holds by
  construction. The HTTP handlers enforce direction solely through `parse` and
  never call `is_sendonly` on these types — so `is_sendonly` now returns the
  documented constant (`true`/`false`), runtime `a=sendonly` scan deleted
  (parse-don't-validate completed), HTTP behavior unchanged. The identical
  `AsRef<str>` + `Display` pair on all three types is folded behind one
  `impl_sdp_string_traits!` macro; `parse` stays hand-written per type so the
  offer-vs-answer distinction stays greppable. All three types kept (never
  collapsed — the swap guard from `08d0bb4`). Pure refactor, done directly.
  `cargo test --all-targets` green (52 lib + 12 integration), clippy
  `-D warnings` + fmt clean.
  - **Remaining:** C2b, C3, C4, C6.

Seven independent refactor candidates, each sized for one agent to pick up
solo. Read this whole preamble before starting any candidate — it exists so
you don't re-litigate closed decisions or break pinned semantics.

---

## Required reading before touching code

1. **`CONTEXT.md`** (repo root) — domain glossary (Connection / Branch /
   Loopback WHIP / Coordinator / Supervisor / Watchdog / Sweep), the
   three-name terminology map (channel ↔ connection ↔ branch — do NOT
   introduce a fourth term), module map, and closed constraints.
2. **`docs/adr/0001`–`0004`** — closed decisions. In particular:
   - The loopback WHIP bridge stays; `src/stream` must never import
     `src/signal` (acyclic module graph).
   - Branch add/remove stay serialized in the coordinator's mailbox.
   - Per-connection failure isolation + watchdog fallback semantics are
     pinned by tests; changing semantics requires deliberate test updates
     and ADR revisiting, not a local patch.
   - `rswebrtc` comes from the GStreamer installation, never compiled in.
3. **`docs/proposals/2026-07-08-signal-plane-and-config-hygiene.md`** —
   ⚠ its "Status: Proposed (no code changed yet)" line is **stale**: all 15
   commits landed on main (`1bcb05c` … `e44005c`, see git log). Treat its
   Decision Document and Testing Decisions sections as **decided and
   implemented**. Two of its decisions directly constrain candidates below
   (called out inline).

## Environment & test loop

- macOS: `cargo build`/`test` need the GStreamer env exported first — see
  README's OSX section. Keep `GST_PLUGIN_PATH` pointed at the framework
  install only (ADR 0003: a second, higher-versioned `rswebrtc` on the path
  shadows the working one).
- CI-equivalent check: `cargo test --all-targets` (unit + actor +
  `tests/signaling.rs` integration). The real-GStreamer
  `tests/e2e_gstreamer.rs` is `#[ignore]`d, needs a live GStreamer install,
  and must be run **in isolation** (see its header comment). Only candidates
  touching `src/stream/gst_pipeline.rs` / `branch.rs` need a manual e2e run.
- Test style (decided in the 2026-07-08 proposal): assert external behavior
  at public seams — HTTP responses through the assembled app, coordinator
  behavior through `SignalHandle`, seam contracts through the traits. Never
  private state or call sequences.

## Suggested order & independence

| Candidate | Depends on | Risk | Size |
|---|---|---|---|
| ~~C1 naming module~~ | — | ✅ **landed** (`b7184ad`, `3ba3fa6`) | S |
| ~~C2a add_branch cleans up after itself~~ | — | ✅ **landed** (`55ceef0`, `f78a3d4`) | S–M |
| C2b retire `ready()` | after C2a (landed); contradicts a recorded decision — read its card | low | S |
| C3 constructor-inject reap channel | — | low–medium | S–M |
| C4 test through SignalHandle | — | low (tests only) | M |
| ~~C5 config defaults single-source~~ | — | ✅ **landed** (`04f464b`) | XS |
| C6 watchdog restart via supervisor | needs ADR discussion FIRST | high (pinned semantics) | M |
| ~~C7 SDP newtype dedup~~ | — | ✅ **landed** (`316b9f8`) | S |

C1+C2a+C5+C7 landed (2026-07-09) — see the Progress log. Of what remains,
C3 and C4 are the low-risk standalones; C2b is the natural follow-on to the
landed C2a (but read its ⚠ card — it contradicts a recorded decision); C6
must not be started without a design conversation and a new/amended ADR.

---

## C1 — Concentrate element naming into one module  · ✅ **LANDED** (`b7184ad`, `3ba3fa6`)

**Files:** `src/stream/gst_pipeline.rs` (creates core element names as bare
literals: `"demux"`, `"video-queue"`, `"audio-queue"`,
`"output_tee_video"`, `"output_tee_audio"` — at ~57, 214–227, 297, 324,
409–426) · `src/stream/branch.rs:40–61` (branch-name prefixes + the
`branch_id_from_name` classifier) and `branch.rs:108–139` (`by_name`
lookups of the same core names).

**Problem.** Whether a pipeline bus error reaps one viewer's branch or
kills the whole pipeline hangs on a naming convention split across two
files. `branch_id_from_name` classifies an element as branch-owned by
stripping prefixes like `"video-queue-"`; the core queues are named exactly
`"video-queue"`/`"audio-queue"` (no id suffix), so **a trailing dash is the
only thing keeping core errors fatal** (see the comment at
`branch.rs:46–55`). Name *construction* (gst_pipeline.rs) and name
*classification* (branch.rs) are bare literals kept in sync by hand: rename
`"video-queue"` in gst_pipeline.rs and the classifier silently
reclassifies a fatal core error as a per-viewer reap.

**Change.** One naming module in `src/stream` (e.g. `stream::naming`) that
owns: core-element name constants, per-branch name constructors
(`video_queue_name(id)`, `whip_sink_name(id)`, `audio_queue_name(id)`,
`video_decoder_name(id)` — note the optional `--decode-video`
`"avdec-h264-"` prefix), and the `branch_id_from_name` classifier. Both
`gst_pipeline.rs` and `branch.rs` consume it. Pure string logic — no
GStreamer types in the module.

**Tests.** `branch_id_from_name` already has unit tests
(`branch.rs:345–369`) — move/extend them. Add the load-bearing invariant as
an explicit test: `branch_id_from_name(CORE_VIDEO_QUEUE)` and
`…(CORE_AUDIO_QUEUE)` return `None` (core names never classify as branch).
That test is the whole point of the candidate.

**Constraints.** The 2026-07-08 proposal's Out of Scope section names "the
real-pipeline init decomposition … the hard-coded element names" as the
acknowledged remaining debt; this candidate is deliberately only the
*naming* slice of it — do not expand into decomposing `init`'s topology.

**Done when:** no element-name string literal appears in more than one
place; the core-vs-branch invariant is pinned by a unit test; behavior
unchanged (`cargo test --all-targets` green; one manual e2e run on macOS).

---

## C2a — `add_branch` cleans up its own half-attached branch  · ✅ **LANDED** (`55ceef0`, `f78a3d4`)

**Files:** `src/signal/coordinator.rs:266–282` (the leak) ·
`src/stream/gst_pipeline.rs:97–121` (`add_branch`) ·
`src/stream/branch.rs:102–185` (`attach`) · `src/stream/pipeline.rs`
(`TestPipeline` must mirror the new contract).

**Problem.** The coordinator matches `PipelineError::Fatal` to guess
whether `add_branch` left a half-attached branch, then detaches it
(`coordinator.rs:271` — `matches!(add_err, PipelineError::Fatal(_))` →
`remove_branch_bounded`). That is a stream-plane implementation fact
("only Fatal can mean attach ran partway") living in the signal plane.
`stream/errors.rs` documents the error taxonomy as pure retry policy;
this caller has overloaded `Fatal` to also mean "maybe half-attached".
Concrete wrong case: `input_ready`'s missing-demux error is `Fatal` and
occurs *before* any attach (`gst_pipeline.rs:58`), so today's code issues a
spurious (harmless but wasteful, bounded-teardown-on-the-actor's-critical-
path) detach.

**Change.** Move the cleanup behind the seam: on attach failure,
`SharablePipeline::add_branch` detaches its own half-built branch (element
lookups are by derived name; `Branch`/detach already tolerates a
half-built branch — see `branch.rs:63–65`) before returning the error. The
coordinator's `matches!(Fatal)` block is deleted; error variants go back to
meaning retry policy only. Optional same-PR improvement: `input_ready`
already derives has-video/has-audio (`gst_pipeline.rs:55–73`) and `attach`
re-derives it from demux pads (`branch.rs:129–133` + audio equivalent) —
derive once and pass it in.

**Constraints — ADR 0002.** ADR 0002 decided the *semantics*: a
half-attached branch must be detached before the error reply. This
candidate keeps those semantics and moves only the *location* of the
detach (coordinator → inside `add_branch`). Say so in the commit message
and update ADR 0002's row if the wording warrants it. Also honor the
decided contract symmetry: fake and real adapters must behave identically
(2026-07-08 proposal, Phase 4) — `TestPipeline::add_branch` needs the same
"failed add leaves nothing attached" observable behavior, and any actor
test currently asserting a recorded `remove_branch` call after a failed
add must be updated deliberately.

**Done when:** `src/signal` contains no `matches!` on stream error
variants; grep for `PipelineError::` under `src/signal/` shows only the
`From` conversion in `signal/errors.rs`; actor tests + integration tests
green; one manual e2e run.

---

## C2b — Retire `BranchControl::ready()`  · **Worth exploring** ⚠ contradicts a recorded decision

**Files:** `src/stream/pipeline.rs:88–114` (trait) ·
`src/stream/gst_pipeline.rs:79–88` (impl) · callers: only
`pipeline.rs` unit tests and `tests/e2e_gstreamer.rs:94`.

**Problem.** `ready()` has zero production callers — production readiness
is enforced inside `add_branch` under one lock (no TOCTOU). It duplicates
`input_ready` purely for the e2e test's startup polling.

**⚠ Recorded decision.** The 2026-07-08 proposal (Decision Document,
"Readiness contract") explicitly *retained* `ready()` on the trait "solely
because the e2e test uses it for startup polling". Do not silently
override this: the candidate is only viable if the e2e test gets an
equivalent poll. Options: poll `POST /channel` until it stops returning
503 + Retry-After (the retry contract already exists and is pinned), or
poll `add_branch` directly for a non-`NotReady` result. If the reviewer
prefers keeping `ready()`, record the outcome (one line in the proposal
doc or an ADR note) so the next architecture pass doesn't re-suggest it.

**Done when:** either `ready()` is gone from `BranchControl` (interface: 4
methods → 3, counting `set_branch_failure_sink` — see C3) and e2e still
passes, or the retention is re-recorded with its reason.

---

## C3 — Constructor-inject the typed bus-reap channel  · **Worth exploring**

**Files:** `src/stream/gst_pipeline.rs:37–39` (the
`Arc<std::sync::Mutex<Option<mpsc::Sender<String>>>>` field), `168–170`
(`set_branch_failure_sink`), `517–524` (the `if let Some(sink)` guard that
silently drops early errors) · `src/stream/pipeline.rs:100` (trait method
with a **default no-op body** — an adapter that forgets to override it
silently loses all reaps) and `227–229` (TestPipeline override) ·
`src/signal/coordinator.rs:199–206` (creates the channel and installs the
sink) · `src/startup.rs:76–78` (spawn ordering that makes it work today).

**Problem.** The reap channel — how a dying established branch gets its
connection cleaned up (ADR 0002) — is a `Sender<String>` installed *after*
pipeline construction into a `Mutex<Option<…>>`. A bus error arriving
before installation is silently dropped. The ordering that prevents this
(coordinator spawned before supervisor) lives in `assemble`'s statement
order; nothing in any interface states it. The payload is a bare `String`
(`ConnectionId` is itself just a `String` alias — `signal/messages.rs:6`),
so the seam carries no type.

**Change.** Make the failure sink a constructor argument:
`SharablePipeline::new(args, failure_sink)`. Define a `BranchId` newtype
**in `src/stream`** (the module graph must stay acyclic — `stream` cannot
import `signal`'s `ConnectionId`); the coordinator maps `BranchId` →
`ConnectionId` at its own edge. Delete `set_branch_failure_sink` from the
trait (and its no-op default), delete the `Option`/`Mutex`, and have
`assemble` create the channel and hand the receiver to the coordinator.
`TestPipeline` gets the same constructor shape. Find all
`SharablePipeline::new` / `TestPipeline` construction sites (startup,
actor tests, supervisor tests, e2e) by grep — several tests construct
pipelines directly.

**Done when:** the sink is present from pipeline birth (the
`if let Some(sink)` guard is gone or unconditional); the trait has no
installer method; the id crossing the seam is typed; all tests green.
The supervisor-restart path needs no change — the sink lives on the
`SharablePipeline` wrapper, which survives pipeline reruns.

---

## C4 — Test the signal plane through `SignalHandle`  · **Worth exploring**

**Files:** `src/signal/mod.rs:31–103` (the facade; note `request()` helper
at 32–42, and the two hand-rolled methods `list_connections` / `reset`
whose replies aren't `Result<_, SignalError>`) · `src/signal/coordinator.rs:501–508`
(`spawn_actor` returns a raw `mpsc::Sender<Command>`) and the ~20 actor
tests below it · `src/signal/messages.rs` (the `Command` enum).

**Problem.** The actor tests hand-build raw `Command` messages one level
*below* `SignalHandle`, so the facade every HTTP route depends on is
crossed by exactly one direct unit test (`mod.rs:112–141`). The interface
is not the test surface; a facade-level regression (wrong reply mapping,
wrong error conversion) would surface only in the slower integration
suite.

**Change.** `spawn_actor` returns a `SignalHandle` (keep the paused-clock
setup); migrate the actor tests to drive it. Then `Command` and the reply
shapes can become module-private — real interface shrinkage, not just test
hygiene. Optionally unify `list_connections`/`reset` into the `request()`
shape (give them `Result`-shaped replies) or leave them but test them.

**Trap.** Some actor tests simulate **abandoned clients** by dropping the
reply `oneshot::Receiver` they hand-built. Through the facade you express
that by dropping the in-flight request *future* instead (e.g.
`tokio::select!` against a paused-clock sleep, then drop). Verify each
abandonment test can be expressed this way; if one genuinely can't, keep a
thin raw-mailbox escape hatch for exactly those tests and say why in a
comment.

**Done when:** actor tests construct no raw `Command`; `Command` is
private to `src/signal`; behavior pinned by the same test names/assertions
as before (renames fine, semantics identical).

---

## C5 — One source of truth for the coordinator's six knobs  · ✅ **LANDED** (`04f464b`)

**Files:** `src/signal/coordinator.rs:11–77` (`CoordinatorConfig`, its
`Default`, `CoordinatorArgs` clap defaults, `to_config()`) and
`1122–1144` (the drift-guard test
`coordinator_args_default_to_the_hardcoded_config`).

**Problem.** Six knobs (offer/answer timeouts, watchdog threshold+window,
sweep interval, teardown timeout) are spelled four times; the clap
`default_value_t` values and the `Default` impl are maintained
independently, and a unit test exists solely to catch drift between them.

**Change.** Named consts as the single default source — with the unit in
the name, since the CLI uses secs/ms while the config holds `Duration`
(e.g. `DEFAULT_OFFER_TIMEOUT_SEC: u64 = 10`,
`DEFAULT_SWEEP_INTERVAL_MS: u64 = 1000`). Feed both
`#[clap(default_value_t = …)]` and `Default for CoordinatorConfig` from
them. Delete the drift-guard test.

**Context.** That test was added deliberately (2026-07-08 proposal, Commit
14) to pin "running with no flags is bit-for-bit today's behavior". With a
single const source the property holds by construction — note that in the
commit message so the deletion reads as intentional, not as lost coverage.

**Done when:** each default value appears exactly once; drift test gone;
`cargo test` green.

---

## C6 — Route watchdog restarts through the supervisor's seam  · **Speculative** ⚠ ADR-adjacent — design conversation first

**Files:** `src/signal/coordinator.rs:399–403` (watchdog trip calls
`pipeline.quit()` via `BranchControl`) · `src/stream/gst_pipeline.rs:158–166`
(`quit` = GLib main-loop quit) · `src/stream/pipeline.rs:221–251`
(`TestPipeline` emulates the coupling: `quit()` fires the same `run_gate`
that `run()` awaits) · `src/supervisor.rs:84–91` and the
`quit_restarts_like_a_clean_run` test at 240–252.

**Problem.** `BranchControl::quit()` — a method on the *coordinator's*
seam — quits the GLib main loop and thereby resolves the *supervisor's*
`PipelineLifecycle::run()`, triggering the restart cycle. A cross-seam
side effect neither interface admits; the "two traits split by caller"
story understates the coupling, and `TestPipeline` must hand-wire it.

**Change (sketch — do not implement before an ADR conversation).** The
watchdog sends a restart request to the supervisor over an explicit
channel; the supervisor (already a select loop) ends the pipeline run via
`PipelineLifecycle` and reruns through its normal cleanup/backoff path.
`quit()` leaves `BranchControl`; `PipelineLifecycle` becomes the only
interface that ends a pipeline run; `TestPipeline`'s `run_gate`
crosswiring is deleted.

**Constraints.** ADR 0001/0002 pin the watchdog *semantics* (N failures in
window ⇒ fail all pending waiters ⇒ full pipeline restart; runtime reaps
don't feed the watchdog; teardown/quit bounded by
`teardown_timeout`) with paused-clock actor tests and
`tests/signaling.rs`. This keeps semantics, moves mechanism — but the
pinned tests (coordinator watchdog tests asserting a recorded `quit`,
`quit_restarts_like_a_clean_run`, any integration watchdog test) must be
updated deliberately, and both ADRs say mechanism changes near the mailbox
get formally revisited, not patched. **Start by proposing the ADR
amendment; only implement after sign-off.** If rejected, record the
rejection reason so future reviews don't re-suggest it.

---

## C7 — Fold the SDP newtypes' triplicated ceremony  · ✅ **LANDED** (`316b9f8`)

**Gate outcome (a):** parse proves direction (`SdpOffer::parse` rejects
non-sendonly, `SdpAnswer::parse` rejects sendonly; private inner field built
only in `parse`), and the handlers enforce direction through `parse` alone —
so `is_sendonly` returns the documented constant and the runtime scan is gone.
The `AsRef<str>`+`Display` pair is folded behind one `impl_sdp_string_traits!`
macro; `parse` stays per-type. All three types kept.

**Files:** `src/domain/session_description.rs:9–131`.

**Problem.** `SessionDescription`, `SdpOffer`, `SdpAnswer` each
hand-implement the same four things — `parse`, `is_sendonly`,
`AsRef<str>`, `Display` — ~120 of the file's 250 lines. On the direction
newtypes, `is_sendonly` is documented as constant ("Always true"/"Always
false") yet still string-scans `a=sendonly` at runtime.

**Verification gate — do this first.** Establish where direction is
actually *enforced*. CONTEXT.md says handlers decide direction policy
(WHEP PATCH rejects sendonly, WHIP POST rejects non-sendonly) — if
`SdpOffer::parse` does **not** itself reject non-sendonly SDP, then
hardcoding `is_sendonly() == true` would be wrong. Two honest outcomes:
(a) parse already proves direction → return the constant, delete the scan
(parse-don't-validate completed); (b) parse doesn't prove it → either move
the policy check into parse (check the HTTP status mapping in
`routes/whep_handler.rs` / `whip_handler.rs` stays identical) or keep the
runtime scan and only dedupe the trait impls.

**Change.** Keep all three types — collapsing them would reintroduce the
offer/answer-swap bug they exist to prevent (threaded through the signal
plane in commit `08d0bb4`). Generate the shared impls once (macro_rules! or
a shared inner struct); resolve `is_sendonly` per the verification gate.

**Done when:** the shared impls exist once; domain unit tests green;
route-level direction-policy tests (integration suite) unchanged.

---

## Out of scope for all candidates

- The `whepserversink` migration (ADR 0001 future work — needs its own ADR;
  the deletion boundary is already marked in code, commit `e44005c`).
- Decomposing `gst_pipeline.rs`'s init topology beyond C1's naming slice.
- The serialized-mailbox trade-off, list API shape, reap/watchdog
  semantics, and everything else in ADR 0001/0002's decision tables.
