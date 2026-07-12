# Architecture Deepening Candidates — Round 2

**Status:** Proposed (no code changed yet). To be worked through one candidate
at a time.
**Date:** 2026-07-11
**Base commit:** `d080a61` (v2.1.1) — line numbers below refer to this commit;
re-verify before editing once anything lands.
**Source:** fresh architecture review pass (three parallel Explore sweeps over
`src/signal`, `src/stream` + `src/supervisor.rs`, and routes/startup/domain/
errors/tests; every headline claim spot-verified against source before
inclusion). Filtered against ADRs 0001–0005 and the closed 2026-07-09 pass
(`docs/proposals/2026-07-09-architecture-deepening-candidates.md`) — nothing
here re-litigates C1–C7 or the declined C2b.

Candidates are numbered **D1–D7** to avoid collision with round 1's C-series.

## Progress log

_(append an entry per landed/declined candidate, same discipline as round 1)_

- **D1 — landed (2026-07-12, PR #116).** `terminate(id, reason)` with
  `TerminateReason::policy()` as the single policy table; the five death
  paths now just name their reason. The Reason enum was revised from this
  card during design review (approved by Kun): `{Deleted, Expired, PeerGone,
  Reaped, Reset}` — `fail_connection` dissolved into policy application
  rather than being a reason, and `Expired` carries no leg payload because
  the state knows its own leg (`ConnectionState::expire()` sends the
  leg-specific `Timeout`). Two columns the card's matrix left implicit
  became explicit policy: notification *ordering* (`Deleted` notifies only
  after teardown succeeds; the rest notify first) and *missing-entry
  meaning* (`Reject`/`Skip`/`Proceed` — `PeerGone` arrives with the entry
  already consumed by the failed delivery). The optional PeerGone honesty
  fix was deliberately deferred: the surviving leg keeps `NotFound` → 404
  (wire-visible change; revisit as its own candidate). All companions
  landed: `deadline()`/`expire()`, named `list_connections()`/`reset()`,
  `ConnectionInfo` projection on `ConnectionState`. Both waiter-gone legs
  are now pinned through `SignalHandle`, including the watchdog-feeding
  row; `Termination` added to the CONTEXT.md glossary.

- **D2 — landed (2026-07-12, PR #117).** `classify_bus_message(&gst::Message)
  -> BusAction {Quit | ReapBranch(BranchId) | Ignore}` extracted from the
  `bus_watch` closure; the closure is now a thin dispatch that executes the
  action and logs. Two placements decided in design review (approved by
  Kun): the classifier lives in a **new `stream::bus` module** rather than
  in `naming` — `naming`'s "pure string logic, no GStreamer types" doc
  property was worth keeping, so `bus` sits on top of it — and **EOS moved
  into the classifier** as a `Quit` classification, so the closure retains
  no match on message shape beyond log formatting. Unit tests (7) cover
  EOS, direct and nested branch errors, core-element errors, source-less
  errors (fatal — previously an untested edge), and non-lifecycle messages;
  the C1 const-based naming-invariant test now has its hierarchy-level
  completion (`containment_scope_holds_at_the_hierarchy_level`). Same
  location-not-behavior discipline as D1: ADR-0002's containment scope is
  concentrated, not changed. Verified: 61 lib + 14 integration green, the
  manual `--ignored` e2e (`pipeline_survives_repeated_handshake_failures`)
  passed, and the browser media guard passed (frames 0→95 climbing).

- **D3 — landed (2026-07-12, PR #118).** The supervisor now names a
  one-method capability — `ResetSignal`, defined provider-side in
  `signal/mod.rs`, matching where `BranchControl`/`PipelineLifecycle` live
  in `stream` — instead of the whole six-method `SignalHandle`. Both wrong
  widths fixed in one change (approved by Kun, incl. the card's optional
  facade split): `spawn_coordinator` returns `(SignalHandle, ResetHandle)`
  and `SignalHandle::reset()` is deleted, so the "supervisor only" doc
  comment became structure — the routes' handle cannot reset. Two adapters
  make the seam real: `ResetHandle` in production, a `RecordingReset` fake
  in the supervisor tests, which no longer construct a coordinator.
  Coverage moved to its owners: `reset_on_cleanup_fails_inflight_handshakes`
  (a supervisor test standing up a real coordinator to prove a coordinator
  behavior) is replaced by `reset_is_requested_after_every_stop`, which
  pins the supervisor's side across all four stop kinds (error, EOS,
  watchdog restart, shutdown); the waiter-failing side stays pinned by the
  coordinator's own `reset_fails_all_waiters_and_clears_state`. The seam
  also made the `RESET_TIMEOUT` bound testable for the first time
  (`a_wedged_reset_does_not_hang_the_restart_loop`). `Reset` added to the
  CONTEXT.md glossary. Verified: 62 lib + 14 integration green; signal/
  supervisor plane only, no e2e needed (per the order table).

- **D4 — landed (2026-07-12, PR #119).** The codec table moved out of
  `init()`'s `no_more_pads` closure into a new `stream::egress` module:
  `build_egress_chain(pipeline, media_type, video_queue, audio_queue)`
  owns media type → parser choice → chain construction → the
  `sync_state_with_parent` invariant; the closure is now a thin dispatch
  that walks the demux pads and logs, mirroring D2's decision/mechanism
  split for the bus watch. Two placements decided in design review
  (approved by Kun, both as recommended): a **new module** (the `bus.rs`
  precedent — its own doc header, its own tests; `gst_pipeline.rs` shrinks
  644 → 570 lines) and the **card's handle-threading signature** — `init()`
  passes the queues it just built rather than the function re-finding them
  by name, so the "queues already in the pipeline" precondition rides in
  the arguments and no new `MissingElement` failure mode appears. The
  extraction kept strictly to the card's scope: the static ingest topology
  stays inline in `init()` (linear construction, no decisions — extraction
  would just move code). Unit tests (4) cover the previously-e2e-only
  table: h264 and h265 (asserting *which* parser the queue got linked to,
  not just that a tee exists), the audio AAC→Opus transcode chain, and
  unknown media as an error that builds nothing. Simpler than the card
  guessed: no `tsdemux` + forced caps needed — the extracted signature
  takes `media_type` as a string, so tests need only a pipeline with two
  named queues. CI runs these everywhere (the PR workflow installs
  base/good/bad/ugly/libav, and `bus.rs` set the `gst::init()` lib-test
  precedent). `Egress chain` added to the CONTEXT.md glossary. Verified:
  66 lib + 14 integration green, the manual `--ignored` e2e passed, and
  the browser media guard passed (frames 0→148 climbing).

- **D5 — landed (2026-07-12, PR #120).** Both halves, one commit each.
  **(a)** `StreamError` deleted, per the card's "pick one honestly": the
  re-verification against `ecd2174` found not just zero matches but zero
  *possible* policy — every one of the 14 construction sites' honest
  classification is Fatal (attach runs under the same lock as the
  `input_ready()` check, so a missing tee mid-attach is genuine breakage,
  not a race; the retryable cases were already constructed as
  `PipelineError::NotReady`/`Transient` directly at the seam), and five
  sites live inside `init()`'s GStreamer callbacks where the error only
  feeds a log line. So option B ("make it drive policy") would have added
  a downcast with identical arms — deletion chosen (approved by Kun).
  Sites now use anyhow `context`/`anyhow!` with the "Failed to find
  element: X" text kept verbatim so logs read the same; `stream::errors`
  is now exactly the `BranchControl` seam language (`PipelineError`
  alone). **(b)** The retryable set `{Timeout, NotReady, PipelineBusy}`
  is spelled once: a private `SignalError::http_contract()` match returns
  `(status, retry-after)` per variant, and both `ResponseError` methods
  are accessors over it. Chosen over the card's literal `is_retryable()`
  predicate (approved by Kun) because it keeps the match exhaustive — a
  new variant forces deciding status and retryability in one arm, instead
  of an early-return leaving unreachable arms or a drift-prone catch-all.
  Contract tests at `signal/errors.rs` unchanged and green, pinning that
  the refactor moved nothing observable. Verified: 66 lib + 14
  integration green, clippy `-D warnings` clean, the manual `--ignored`
  e2e passed (18.8s), and the browser media guard passed (frames 0→148
  climbing).

- **D6 — landed (2026-07-12, PR #121).** `assemble` now takes a pipeline
  factory (`impl FnOnce(mpsc::Sender<BranchId>) -> P`) and owns the
  bus-reap channel; the capacity moved to one `BRANCH_FAILURE_CAPACITY`
  const in `startup.rs` whose comment records why overflow is safe (the
  bus handler `try_send`s; the sweep is the backstop). The handle-return
  tension resolved as the card's second option (approved by Kun):
  `assemble` returns `(Application, P)` — both test files need the
  constructed pipeline (`snapshot()`/`fail_branch()`/`ready()`), and the
  factory-captures-a-clone alternative would have traded three lines of
  channel ceremony for five of capture ceremony at exactly the sites that
  need the handle; the one wart is `main.rs` ignoring it with
  `let (app, _pipeline)`. Re-verification found a **fourth** call site
  the card missed: `assemble_rejects_a_mismatched_whip_port` built a
  throwaway `channel(1)` just to satisfy the parameter — the clearest
  symptom of the channel not belonging in the interface. The port check
  now runs before the factory, so on bad config the pipeline is never
  constructed; that test pins the ordering with a
  `|_| -> TestPipeline { unreachable!(...) }` factory and lost its
  throwaway channel. `spawn_app`'s `set_ready(true)` moved inside the
  factory, preserving ready-before-coordinator ordering exactly. The
  sink-present-from-birth invariant (C3) is now unexpressible to get
  wrong: the sender exists nowhere outside `assemble`. No new CONTEXT.md
  term ("pipeline factory" is implementation detail, as with D5).
  Verified: 66 lib + 14 integration green, clippy `-D warnings` clean,
  the manual `--ignored` e2e passed (18.6s; one cold-start flake on the
  fresh worktree's first-ever run, clean on rerun), and the browser
  media guard passed (frames 0→148 climbing) — run deliberately because
  it exercises the release binary through `main.rs`, the one call site
  no test executes.

- **D7 — landed (2026-07-12, PR #122), both tiers.** Design review
  (approved by Kun) took tier 1 + tier 2 together rather than the doc move
  alone: doc-move-only would have enshrined the wart in `branch.rs`'s own
  interface docs ("detach aborts at the first unlinked queue; leftovers
  wait for a restart"), whereas hardening first lets the moved docs state a
  clean contract — `detach` removes every element this branch put in the
  pipeline, however far `attach` got. Tier 2:
  `remove_branch_from_pipeline`'s exists-but-unlinked arm now removes the
  element directly (no tee pad to release) instead of erroring, so the
  failed-attach cleanup detach in `gst_pipeline.rs` actually cleans instead
  of logging "cleanup also failed" and leaning on the next restart. The
  arm is reachable only from partial state: a normal no-audio branch has
  no audio queue at all (absent → skip), and `link_many` links
  sequentially, so a linked sink pad always takes the pad-probe path. Two
  discoveries beyond the card: (1) ADR 0002 already *asserted* "detach
  tolerates a half-built branch" — the code only half-delivered it
  (absent yes, unlinked no); tier 2 aligns the code with the ADR, no ADR
  edit needed. (2) The card's "done when" imagined a manual e2e as the
  only tier-2 verification, but `bus.rs`/`egress.rs` already run
  `gst::init()` unit tests in CI — so the partial-attach teardown is now
  pinned by an automated test
  (`detach_after_a_partial_attach_removes_everything_it_can_reach`,
  core elements only: queue/identity/fakesink stand-ins found by derived
  name), written red-first against the old abort behaviour. The caller
  comment at `gst_pipeline.rs` shrank from nine lines of borrowed
  mechanics to caller-local facts (ADR 0002 location note, error
  reporting, id-never-mapped). No new CONTEXT.md term — post-hardening,
  "partial attach" is not even part of the caller-visible contract, which
  is the point. Verified: 67 lib + 14 integration green, clippy
  `-D warnings` clean, the manual `--ignored` e2e passed (18.8s, first
  try), and the browser media guard passed (frames 0→145 climbing).
  **Round 2 complete: D1–D7 all landed.** Deferred for a future round:
  the PeerGone 404 honesty fix (wire-visible).

- **PeerGone honesty fix — landed (2026-07-12, PR #123).** The candidate
  deferred from D1, taken up after round 2 closed. Design review
  (approved by Kun) widened the scope from "the two PeerGone surviving
  legs" to the full honesty line, because exploration showed the
  dishonesty was provable at three sites, not two:
  `WaiterNotice::Gone` (used by `Deleted` and `Reaped` too) is a
  dishonest 404 *by construction* — `terminate` only notifies waiters
  whose entry it just removed, so the connection always existed. New
  `SignalError::Gone(id)` → **410 Gone** (no Retry-After: a dead session
  won't come back; the client's move is a fresh POST) at all three
  sites; the two genuine-404 families (unknown-id lookups,
  `MissingEntry::Reject`) stay `NotFound`. The resulting wire invariant
  is statable in one line — **in-flight at the moment of death → 410
  ("it existed; it just ended"); later by-id lookups → 404 ("never knew
  it")** — with no tombstones kept, so it cannot conflict with
  WHIP/WHEP's 404-after-termination convention (subsequent requests
  still 404). Recorded in CONTEXT.md's Termination entry. DELETE
  idempotency (`remove.rs` matching `NotFound` → 204) is untouched: it
  matches only the `Reject` arm. Pinned three ways: the errors.rs
  contract tests gained the 410/no-Retry-After rows, the two
  vanished-peer coordinator tests now assert `Gone`, and a NEW
  wire-level integration test
  (`in_flight_requests_get_410_when_the_connection_dies`) parks a WHEP
  POST, DELETEs the connection, and asserts the parked request resolves
  410 without Retry-After — the first test to pin the parked-waiter
  notice on the wire. Verified: 67 lib + 15 integration green, clippy
  `-D warnings` clean, the manual `--ignored` e2e passed (18.7s; the
  usual fresh-worktree cold-start flake on the first run, clean on
  rerun — the real whipclientsink receives the 410 on its offer POST in
  exactly those vanished-viewer rounds), and the browser media guard
  passed (frames 0→142 climbing).

---

## Required reading before touching code

1. **`CONTEXT.md`** — domain glossary (Connection / Branch / Coordinator /
   Supervisor / Watchdog / Sweep / Reap / Parked waiter), the three-name
   terminology map (channel ↔ connection ↔ branch — do NOT introduce a fourth
   term), module map, closed constraints, and the parallel-session git
   discipline.
2. **`docs/adr/0001`–`0005`** — closed decisions. In particular:
   - Loopback WHIP bridge stays; `src/stream` never imports `src/signal`.
   - Branch add/remove stay serialized in the coordinator's mailbox; teardowns
     bounded by `teardown_timeout`.
   - Watchdog / reap / sweep **semantics** are pinned by tests. D1 and D2
     concentrate those semantics without changing them — same
     location-not-behavior discipline as C2a and ADR-0005. Say so in commit
     messages.
   - `BranchControl::ready()` stays (C2b declined 2026-07-10) — do not
     re-remove it while doing D-work in `pipeline.rs`.
3. **Round 1 handover doc** — for the test-style decision (assert external
   behavior at public seams, never private state or call sequences) and the
   environment/test-loop notes (macOS GStreamer env, `cargo test
   --all-targets`, when a manual `--ignored` e2e run is required).

## Suggested order & independence

| Candidate | Depends on | Risk | Size | Needs e2e run |
|---|---|---|---|---|
| D1 terminate module | — | low (signal plane only) | M | no |
| D2 bus-policy classifier | — | low-medium | S–M | yes |
| D3 reset capability seam | — | low | S | no |
| D4 egress codec table | — | medium | M | yes |
| D5 error taxonomy hygiene | — | low | S | 5a: yes; 5b: no |
| D6 assemble owns reap channel | — | low | S | yes (touches e2e file) |
| D7 branch contract locality | doc move: free | doc: none; hardening: medium | XS–S | hardening: yes |

All seven are independent of each other. File-overlap coordination only:
D2/D4/D5a all edit `gst_pipeline.rs`; D6 edits the same three assemble call
sites D2/D4's e2e runs exercise. Land serially, not in parallel worktrees,
if picking several.

**Top recommendation: D1** — biggest locality win, no GStreamer needed, tests
through the existing `SignalHandle` infrastructure (C4), and it makes the
only-in-comments watchdog policy into code. D2 is the close second — pick it
first if closing the e2e-only coverage gap matters more than signal-plane
locality.

---

## D1 — Concentrate connection termination into one module · **Strong**

**Files:** `src/signal/coordinator.rs:379–493` — `remove_connection`
(379–399), `sweep_expired` (401–430), `fail_connection` (432–446),
`reap_branch` (448–462), `reset_all` (489–493); plus the `WaiterGone` arms at
338–344 / 365–371 and the `handle()` dispatcher inlines at 266–281.

**Problem.** There is no single owner of "a connection is ending." Five paths
each hand-assemble a different subset of the same five primitives, and the
load-bearing semantic — *does this death feed the watchdog?* (sweep: yes;
reap: deliberately no) — exists only as prose comments (:432, :451). Locality
of the termination *policy* is missing. The verified matrix:

| Path | drop entry | fail waiter | rm branch | watchdog | restart |
|---|---|---|---|---|---|
| `remove_connection` (DELETE) | first, re-insert on fail (:395) | `NotFound`, on success only (:391) | ✓ (:388) | — | — |
| `sweep_expired` | ✓ (:419) | inline leg-specific `Timeout` (:421/:424) | via `fail_connection` | ✓ | ✓ |
| `fail_connection` | (caller's) | (caller's) | ✓ (:435) | `record_failure` (:438) | `try_send` if tripped (:444) |
| `reap_branch` | ✓ (:454) | `NotFound` (:458) | ✓ (:459) | **no — deliberate, comment-only (:451)** | — |
| `reset_all` | drain (:490) | `Unavailable` (:491) | **no** | — | — |

Supporting friction that folds in:

- **Deadline bookkeeping is split three ways:** the `deadline` field is
  produced in `create_connection` (:322) and `offer_received` (:334), stored
  in both `Awaiting*` variants, and consumed only by `sweep_expired` — which
  matches the same two variants **twice** (:406–414 to filter, :420–425 to
  pick the leg-specific `Timeout` message), because `fail_waiter` can only
  send one generic error.
- **The `WaiterGone` path is untested.** No test drives "peer's oneshot
  receiver dropped → deliver fails → fail the handshake → feed the watchdog"
  (:338–344, :365–371) — it's awkward to race through `SignalHandle` today.
  It's a real production path (actix drops handler futures on client
  disconnect).
- **`WaiterGone` reports a misleading error:** the surviving leg gets
  `NotFound(id)` → 404 although the id existed and its branch was attached —
  the viewer left, the id wasn't unknown.
- **`handle()` inlines `ListConnections` and `Reset`** (:266–281) while
  delegating the other four commands, so "Reset also resets the watchdog"
  lives in the dispatcher, away from `reset_all`.

**Change.** An internal `terminate(id, Reason)` where
`Reason ∈ {Deleted, Expired(leg), HandshakeFailed, PeerGone, Reset}` and the
reason-to-policy table (waiter error, branch teardown y/n, watchdog y/n,
restart y/n) is written once. The five paths shrink to "compute the reason,
call terminate." Companion moves, same commit or adjacent:

- `ConnectionState::deadline()` accessor + `expire()` yielding the
  leg-specific `Timeout`, collapsing the sweep's double match.
- `list_connections()` / `reset()` become named methods like the other four;
  the `ConnectionInfo` projection moves next to `ConnectionState` (where
  `name()` already lives).
- Optional honesty fix while there: a distinct waiter error for the
  `PeerGone` reason instead of `NotFound` (weigh the wire-visible change —
  today's 404 is defensible since the connection *is* gone by reply time; if
  changed, update the integration tests deliberately).

**Constraints.** ADR-0001/0002 pin watchdog/reap/sweep *semantics* — this
concentrates them without changing them. Existing paused-clock tests through
`SignalHandle` must stay green **unchanged** (they assert external behavior,
which doesn't move). New unit tests for `terminate` per reason become the
pinning surface for the policy table, including the previously untestable
`PeerGone` row.

**Done when:** each of the five primitives is invoked from exactly one place;
the watchdog y/n column is code (a match on `Reason`), not comments; the
sweep matches the `Awaiting*` variants once; a `terminate`-level test covers
every `Reason` including `PeerGone`; `cargo test --all-targets` green with no
existing assertions weakened.

---

## D2 — Extract the bus reap-or-quit policy as a pure classifier · **Strong**

**Files:** `src/stream/gst_pipeline.rs:490–551` (the `bus_watch` closure) ·
`src/stream/naming.rs:78–88` (`branch_id_from_name`, consumed by the walk).

**Problem.** The system's most load-bearing guarantee — one bad peer reaps its
own branch, never the whole pipeline (ADR-0002 containment) — lives as an
inline closure where the *decision* (quit-all vs reap-one vs ignore) is fused
with the *mechanism* (`main_loop.quit()` / `branch_failures.try_send`). The
ancestry walk (:511–520) that turns "an element nested inside a whipclientsink
errored" into "reap viewer X" exists nowhere else and is exercised **only** by
the `#[ignore]` e2e — `TestPipeline` bypasses it entirely (the fake's
`fail_branch` injects directly into the reap channel).

**Change.** A pure
`fn classify_bus_message(msg: &gst::Message) -> BusAction`
with `BusAction { Quit | ReapBranch(BranchId) | Ignore }`, living next to (or
in) `stream::naming` — it is the hierarchy-level completion of
`branch_id_from_name`'s single-name classification. The closure becomes a
thin match dispatching each action. The classifier gets unit tests built on a
fake element hierarchy (construct `gst::Bin`s/elements with branch-derived and
core names, post an error, classify) — no SRT, no live peer.

**Constraints.** ADR-0002 pins the containment *scope* (branch queues reap;
core `video-queue`/`audio-queue` errors stay fatal). The classifier
concentrates that scope; extend C1's const-based naming-invariant test to it
(core names must classify `Quit`, branch-derived names `ReapBranch`). EOS
handling (:495–500) stays in the closure or becomes a `Quit` classification —
either is fine, keep it explicit.

**Done when:** the closure contains no classification logic (no ancestry
walk, no name matching); `classify_bus_message` has unit tests covering
branch-element error (nested and direct), core-element error, non-error
message; behavior unchanged; one manual `--ignored` e2e run passes.

---

## D3 — Narrow the supervisor↔signal seam to a reset capability · **Worth exploring**

**Files:** `src/supervisor.rs:19` (field), `:156` (the only production use),
`:206–216` (`wire()` test helper) · `src/signal/mod.rs:99–101` (`reset()`,
"supervisor only" by doc comment).

**Problem.** Two widths wrong at once. The supervisor depends on the entire
six-method `SignalHandle` to call exactly one method — verified: `self.signal`
appears at exactly one production site (:156) — so every supervisor test must
spawn a full real coordinator just to obtain a handle. Meanwhile the HTTP
routes hold the *same* handle via `web::Data`, on which the supervisor-only
`reset()` (fails all waiters, clears the map) is guarded by nothing but a doc
comment. Each caller's interface is wider than its contract.

**Change.** The supervisor takes a narrow reset capability — a one-method
trait (e.g. `ResetSignal { async fn reset(&self) -> Result<(), SignalError> }`)
— which `SignalHandle` implements. Supervisor tests use a recording fake and
assert the reset contract (called after every stop, bounded by
`RESET_TIMEOUT`) without standing up the coordinator. Two adapters
(`SignalHandle` in prod, fake in tests) make the seam real. If splitting
`reset` fully off the route-visible facade is cheap in the same change (a
separate control handle returned by `spawn_coordinator`), do it; otherwise
record it as the follow-up.

**Done when:** `supervisor.rs` names a one-method capability, not
`SignalHandle`; supervisor tests construct no coordinator; routes' handle
either lacks `reset` or the follow-up is recorded; `cargo test --all-targets`
green.

---

## D4 — Extract the egress codec table from `init()` · **Worth exploring**

**Files:** `src/stream/gst_pipeline.rs:269–384` (`connect_no_more_pads` →
`link_media`, the codec table) · `:386–460` (`connect_pad_added` →
`insert_sink`) — both closures inside the 276-line `init()` (:193–469).

**Problem.** `link_media` is a genuine decision table — media type → parser
choice (`h264parse`/`h265parse`/none) → egress chain construction
(`output_tee_*` + fakesink) → the load-bearing `sync_state_with_parent`
invariant (:324–328) — nested ~6 levels deep as a closure inside `init()`,
reachable only via the ignored e2e.

**Change.** Extract only the decision half as
`build_egress_chain(pipeline, media_type, video_queue, audio_queue) -> Result<()>`
(the closure captures the pre-built queue handles at :319/:345 — the extracted
interface must thread them). Unit-testable with a `tsdemux` + forced caps,
asserting `by_name(OUTPUT_TEE_VIDEO)` exists — needs GStreamer initialised
(registry elements) but no SRT, no live source. **Deliberately leave the
static ingest topology inline** (:208–267: element construction, `add_many`,
three `link_many` chains) — it fails the deletion test in the other direction:
linear construction, no decisions, one caller; extraction would just move
code.

**Constraints.** Round 1 scoped out "decomposing init's topology" — that was
scoping for that pass, not an ADR. This candidate re-evaluates it and takes
only the decision half. Do not expand into the static half; if tempted,
that's a sign the extraction is drifting into move-code.

**Done when:** `init()` contains no codec/parser decisions; the extracted
module has unit tests per media type (h264, h265, audio, unknown); behavior
unchanged; one manual `--ignored` e2e run passes.

---

## D5 — Error taxonomy hygiene: four enums, two carry the load · **Worth exploring**

Two independent halves; land together or separately.

**(a) Collapse `StreamError`.**
**Files:** `src/stream/errors.rs:43–55` (definition); ~14 construction sites
across `branch.rs` and `gst_pipeline.rs`; flattened at `gst_pipeline.rs:149`
and `:183`.
**Problem.** Verified: constructed at 14 sites, matched at zero. Its type
information dies via `.to_string()` into `PipelineError::Fatal(String)` before
crossing the `BranchControl` seam; the variant distinction only changes a
Display prefix. Fails the deletion test — nothing downstream would notice.
**Change.** Either delete it in favour of `anyhow` context strings at the
construction sites, or — if the `MissingElement` vs `FailedOperation`
distinction *should* be load-bearing — make it drive the `PipelineError`
classification at the seam instead of both collapsing to `Fatal`. Pick one
honestly; the current halfway state buys nothing.

**(b) Single-source the retryable predicate.**
**Files:** `src/signal/errors.rs:37–39` (`status_code`) and `:48–51`
(`error_response`).
**Problem.** The set `{Timeout, NotReady, PipelineBusy}` is spelled
character-for-character twice — once to map 503, once to attach `Retry-After`.
Add a retryable variant, miss one list: a 503 with no `Retry-After` (or the
reverse).
**Change.** One `fn retry_after(&self) -> Option<...>` (or `is_retryable()`);
both methods consult it. The existing contract tests (`signal/errors.rs:76–166`)
pin the outcome.

**Done when:** (a) `StreamError` is gone or its variants provably drive
policy; (b) the retryable set appears exactly once; contract tests green.

---

## D6 — `assemble` owns the bus-reap channel · **Worth exploring**

**Files:** `src/startup.rs:51–94` (`Application::assemble`) · callers:
`src/main.rs:30–31`, `tests/signaling.rs:56–57` (+ `:521–522`),
`tests/e2e_gstreamer.rs:63–64`.

**Problem.** `assemble` is almost deep — one call wires coordinator +
supervisor + HTTP server — except the bus-reap channel: every caller must
create the channel, hand the sender to the pipeline constructor, and hand the
receiver to `assemble`. Verified identical three-line ceremony at all call
sites. The invariant C3 established (sink present from pipeline birth) is
honored by convention at N call sites, not by construction at the seam —
because `assemble` is generic over `P: BranchControl + PipelineLifecycle` and
can't construct `P` itself.

**Change.** `assemble` accepts a pipeline factory
(`FnOnce(mpsc::Sender<BranchId>) -> P`) and creates the channel internally.
Tension to resolve in design: tests need the constructed pipeline handle back
(`snapshot()`/`fail_branch()` on the fake) — either the factory captures a
clone out, or `assemble` returns the pipeline alongside the app. Also decide
where the channel capacity (currently 64 at every site) lives — one const in
`startup.rs`.

**Done when:** no caller creates the reap channel; the sender-from-birth
invariant is unexpressible to get wrong; all three call sites shrink;
`cargo test --all-targets` green; e2e compiles and passes one run (its call
site changes).

---

## D7 — Branch's partial-attach contract lives at its caller · **Speculative**

**Files:** `src/stream/branch.rs:79`, `:181` (attach/detach ordering docs),
`:299–305` (`remove_branch_from_pipeline` errors on an exists-but-unlinked
queue) · `src/stream/gst_pipeline.rs:127–149` (call-site comment holding the
partial-attach semantics).

**Problem.** `Branch` is genuinely deep (attach hides ~90 lines; detach hides
the tee pad-probe dance), but its sharpest edge — a failed attach can leave
elements behind, because detach aborts at the first unlinked queue before
reaching the whip sink, "cleared on next restart" — is documented only at the
caller in `gst_pipeline.rs`. A reader of `branch.rs` alone cannot learn it.

**Change.** Two tiers:
1. **Doc move (free):** the partial-attach/leftover-elements invariant moves
   into `Branch::attach`/`detach`'s own interface documentation.
2. **Hardening (behaviour-adjacent — decide deliberately):**
   `remove_branch_from_pipeline` treats exists-but-unlinked as removable
   (clean the element rather than erroring), making detach robust to partial
   state so partial attaches tear down fully. Today's leftovers are swept on
   restart, so this is an improvement, not a bug fix — weigh against the e2e
   run it requires.

**Done when:** tier 1 — the contract is readable from `branch.rs` alone.
Tier 2, if taken — detach after a simulated partial attach removes everything
it can reach; one manual e2e run passes.

---

## Checked and found already deep — do not re-litigate

- **Watchdog** (`signal/watchdog.rs`) — three-method interface hides the
  windowed-decay math; deletion test says the counter + decay comparison would
  reappear at three record sites. Known interface subtlety (not a fix):
  `record_failure() -> bool` couples "tripped" with auto-reset; the caller
  relies on it silently.
- **Supervisor** (`supervisor.rs`) — restart policy, backoff, bounded
  shutdown, restart arm, stale-request draining behind a 4-arg spawn. Deep.
  Only its `SignalHandle` dependency is wide (D3).
- **SignalError status seam** (`signal/errors.rs`) — one `ResponseError`
  impl; routes never map statuses; retry policy survives every seam crossing,
  pinned by tests. A new SDP rule reaching 400 touches one file. Only wart:
  D5b.
- **naming.rs classifier** — core-vs-branch invariant holds by construction,
  tested against the consts (C1). D2 builds on it.
- **Domain SDP newtypes** — parse-don't-validate, direction proven by
  construction (C7). Deep.
- **DELETE idempotency** (`routes/remove.rs`, PRs #111/#114) — the
  200/204/propagate policy is one private `delete()` fn both routes share;
  routes only match `NotFound`, which is public vocabulary. The fresh code
  did not leak coordinator internals.
- **Static ingest topology** (`gst_pipeline.rs:208–267`) — fails the deletion
  test in the other direction; leave inline (see D4).
- **utils.rs discoverer** — result explicitly swallowed; value is log lines
  only. Shallow but harmless; a delete-or-leave, not a deepening target.

## Minor notes (recorded, not candidates)

- `whep_handler.rs:11–15` — the "empty body expected" protocol rule
  hand-builds `SignalError::InvalidSdp` in the route; every other
  request-shape rule lives in the domain. Folding it into a domain parse step
  would let the near-duplicate `InvalidSdp` variant collapse into `Sdp`.
  Defensible as-is.
- The restart channel's coalesce + drain-stale contract is matching prose on
  both ends (`coordinator.rs:441–444`, `supervisor.rs:85–91`) rather than a
  type. Works today.
- Doc rot: `create_custom_queue` claims overrun *and underrun* signals; only
  `"overrun"` is wired (`gst_pipeline.rs:634`/`:648`).
- Stale test comment: `failed_add_registers_nothing…` (`coordinator.rs:~890`)
  still references the removed `matches!(Fatal)` block.
- Integration tests learn connection ids by reading the fake's recorded
  `added` list (`signaling.rs:71–80`) — inherent to server-minted ids in the
  loopback design; recorded so nobody mistakes it for an oversight.
