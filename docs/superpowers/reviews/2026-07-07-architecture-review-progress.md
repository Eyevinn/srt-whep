# Architecture-deepening pass — progress log

Working branch: `arch/deepening` (off `main` @ `9cc8f48`).
Note: `main` had moved past the review's stated baseline `f1867d3` — it now
includes the whip-branch error-isolation merge (`9cc8f48`), i.e. the wedge
fix recorded in project memory. Test baseline re-verified before starting:
**23 unit + 9 integration green, 1 ignored e2e** — identical counts to the
review's baseline, so all "Done when" criteria remain meaningful as written.

Execution order: C1 → C2 → C5 → C3a → C3b → C3c → C4 (C6 only if C4 invites it).

---

## 2026-07-07 — Setup

- Branch `arch/deepening` created; handoff docs committed as `a6e0451`.
- Baseline `cargo test` (with the GStreamer DYLD env): 23 unit + 9
  integration pass, e2e stays `#[ignore]`d. Matches review baseline counts.

## 2026-07-07 — C1 complete (`71761a0`)

**What changed**

- `PipelineBase` (10 methods) deleted; replaced by two traits in
  `src/stream/pipeline.rs`:
  - `BranchControl` — `ready`, `add_branch`, `remove_branch`, `quit`
    (coordinator's seam; connection→branch rename done at the seam).
  - `PipelineLifecycle` — `init`, `run`, `end`, `clean_up` (supervisor's seam).
- `print()` removed from all trait surfaces; kept as an inherent `pub async
  fn` on `SharablePipeline` (zero callers, debug-only, as the review directed).
- Coordinator + `spawn_coordinator` bound to `BranchControl` only.
- `TestPipeline` implements both traits (integration tests need it in
  production source — anticipated default kept).

**Decisions (anticipated defaults + local calls)**

- Took the "move `Args` out of the lifecycle signature if cheap" option:
  `PipelineWrapper` now stores the full `Args` from `new(args)` (it stored
  `args.port` already), and `init()` takes no arguments. Consequence:
  `PipelineGuard::new(pipeline, signal)` lost its `args` parameter;
  `main.rs` and the e2e supervisor block updated.
- `init` also went `&mut self` → `&self` (both impls use interior
  mutability; `&mut` was ceremony). Rejected alternative: keeping `&mut
  self` for "lifecycle exclusivity" — it doesn't actually exclude anything
  through an `Arc`, and it forced `mut` bindings on every guard.

**Test results:** 23 unit + 9 integration green (counts unchanged from
baseline). e2e not run for C1 (pure refactor; it runs per C3 stage).

**Done-when check:** both traits exist ✓ · coordinator bound to
`BranchControl` only ✓ · `print` gone from trait surfaces ✓ · suite green
unchanged ✓.

## 2026-07-07 — C2 complete (`11017b0`, `00d474e`, `f483547`, `53f74e2`, `300250d`)

Plan: `docs/superpowers/plans/2026-07-07-c2-supervisor-module.md` (executed
directly, TDD).

**What changed**

- `TestPipeline` is now a controllable fake: `run()` parks until released
  (`finish_run`/`fail_run`, or `end`/`quit` exactly like EOS/forced-quit
  resolve the real `run()`); lifecycle calls are counted (`11017b0`).
- New `src/supervisor.rs` (`00d474e`): the whole supervision story in one
  module — init → run (on its own task; the real `run()` parks a worker in
  the GLib main loop) → explicit cleanup (`clean_up` + `signal.reset()`) →
  rerun with exponential backoff (1s base, ×2 per consecutive failure, 30s
  cap, reset on clean run). Shutdown via `tokio::sync::watch`: EOS then a
  5s-bounded join. Six unit tests (restart, reset-fails-waiters, shutdown
  ordering, dropped-sender, watchdog-quit restart, backoff).
- `Application::assemble` in `startup.rs` (`f483547`): coordinator +
  supervisor + server wired once; actix `.disable_signals()`;
  `run_until_stopped(stop)` fans one token out to supervisor + graceful
  HTTP stop. `main.rs` is parse → assemble → await Ctrl-C.
- `tests/signaling.rs` `spawn_app` (`53f74e2`) and the e2e (`300250d`) both
  run the production wiring; the e2e's copy-pasted supervision loop is
  gone; teardown is the production shutdown path under a 15s cap.
- `src/utils.rs` (`PipelineGuard`) deleted; `tokio_async_drop` dependency
  removed (`300250d`).

**Deviations from the plan**

- `utils.rs` deletion moved from plan-Task 3 to plan-Task 5 (same commit as
  the e2e rewire) — deleting it earlier would have broken the e2e build
  between commits, violating "suite green at every commit".
- The watchdog HTTP test needed no extra `cleanup_count` wait: its only
  post-trip assertion is `quit_count`, which the supervisor's reset cannot
  race. Verified by running the signaling suite 3× (green each time).

**Test results:** 30 unit + 9 integration green. `--ignored` e2e: first run
failed (exit 1, no hang — the bounded-teardown path worked), immediate
isolated rerun **passed with clean process exit**; signature matches the
documented environmental flake (whipclientsink/VideoToolbox resource
exhaustion on back-to-back runs), not a regression vs baseline.

**Manual smoke (double-Ctrl-C defect):** binary run against a dummy SRT
address; HTTP `/list` returned 200; **one** SIGINT → process exited in
~1s. Before C2 the first Ctrl-C left the process running headless.

**Done-when check:** `utils.rs` deleted ✓ · e2e uses production supervisor
✓ · one Ctrl-C exits cleanly (smoke-verified) ✓ · supervisor unit tests
exist ✓ · suite green ✓.

## 2026-07-07 — C5 complete (`d4b7a9c`, `e340c0d`, `9779f2f`, `8bf844f`, `69d8e51`)

Executed via subagent-driven development (plan:
`docs/superpowers/plans/2026-07-07-c5-hygiene.md`, 7 review items grouped
into 4 tasks; every task implementer-built and independently
review-approved; ledger in `.superpowers/sdd/progress.md`).

**What changed**

1. Dead deps removed (`d4b7a9c`): `derive_more`, `config`, `chrono`,
   `serde-aux`, `unicode-segmentation`, `validator`, `secrecy`, `toml`
   gone; `reqwest` no longer in `[dependencies]` (stays in dev-deps).
   Reviewer independently re-verified all were caller-less.
2. CI (`e340c0d`): `pull-request.yml` gains a `test` job — GStreamer apt
   list copied verbatim from `publish.yml` (untouched), rust-cache,
   `cargo test --all-targets` + `fmt --check` + `clippy -D warnings`;
   trigger widened from `types: [opened]` to the defaults (decision
   logged: old trigger never re-ran on pushed commits).
3. Fixes (`9779f2f`, test-first): WHIP `Location` is now the routable
   `/whip_sink/{id}` and `DELETE` on it removes the connection (new
   integration test — suite is now 30 unit + **10** integration);
   `SessionDescription::is_empty` deleted (verified caller-less);
   `/pipe/ /source/ /srt/` + `.DS_Store` gitignored, `.dot` debris
   deleted; "Pipeline is not missing" log now "Pipeline is not
   initialized". Note: the plan's test snippet had a bug (missing second
   PATCH leg); implementer corrected it — both hard assertions kept.
4. Docs (`8bf844f` + fix `69d8e51`): `CONTEXT.md` (glossary,
   channel/connection/branch terminology map, module map, decided
   constraints, env note) and `docs/adr/0001-signaling-plane-rebuild.md`
   (promoted decision table). One Important review finding (unsupported
   "not observed to matter in practice" claim) fixed with verifiable
   wording.

**Deviation from review doc:** item 5 offered "register a handler OR
return a reachable location" — did both halves coherently: Location
dropped the meaningless UUID segment AND a DELETE route was registered at
exactly that URL (rejected alternative: keeping the
`/whip_sink/{id}/{resource_id}` shape with a resource handler — the
resource id was random and stored nowhere, so it guarded nothing).

**Test results:** 30 unit + 10 integration green, 1 ignored e2e. Minor
findings parked for the final whole-branch review: reqwest test builds now
native-tls only; apt list (inherited verbatim) lacks `-y`/`libssl-dev`
(works on ubuntu-latest per publish.yml history); gitignore
trailing-slash style.

## 2026-07-07 — C3 complete (3a `c8be296`, 3b `5794880`, 3c `596b20a`)

Plan: `docs/superpowers/plans/2026-07-07-c3-pipeline-deepening.md`
(executed directly; e2e run in isolation after every stage).

**3a — lock private.** `Deref`/`DerefMut` to the `Arc<Mutex<...>>`
removed; `PipelineWrapper` module-private. The two await-under-guard
holes closed: `remove_branch` snapshots the pipeline handle before the
teardown awaits; `clean_up` takes the pipeline (and clears the stale
main loop) under the lock, then NULLs it unlocked. Audit of remaining
methods: guard held only across synchronous GStreamer calls.

**3b — Branch module.** `src/stream/branch.rs` is the single owner of
the per-connection element names, attach linking, pad-probe teardown
(moved verbatim), and the WHIP route/URL template: `WHIP_SINK_ROUTE` is
imported by `startup.rs`'s route table and instantiated by the WHIP
Location header + whipclientsink endpoint via `whip_sink_path` /
`whip_endpoint` — one template, grep-gated (no convention definition
outside branch.rs). `link_media`'s h264/h265 arms collapsed over a
parser table. New pure-fn unit test (suite now 31 unit).

**3c — main loop off the executor.** `run()` no longer blocks a tokio
worker in `glib::MainLoop::run()`: the loop runs on a named OS thread
('gst-main-loop') with the bus watch installed there; `run()` awaits a
oneshot completion signal. Removes the sync-in-async trap (the
documented current_thread e2e hang cause).

**3d:** not done, as planned (review: optional, revisit spec first).

**Test results per stage:** 3a: 30+10 green, e2e pass (18s; one
first-attempt environmental flake before the isolated pass, matching the
known signature). 3b: 31+10 green, e2e pass first try (17s). 3c: 31+10
green, e2e pass first try (18s) + one-Ctrl-C smoke re-verified (~1s exit)
since shutdown now crosses the thread boundary.

**Done-when check:** no `Deref` to the lock ✓ · one definition of branch
names + whip route ✓ · `cargo test` green ✓ · e2e at least as good as
baseline (passes in isolation, exits cleanly — baseline needed isolation
too) ✓.

## 2026-07-07 — C4 complete (`947a46c`)

Developed test-first (RED: mapping tests + a coordinator test with an
injected Transient failure; then GREEN).

**What changed**

- `BranchControl` speaks `PipelineError { NotReady, Transient(String),
  Fatal(String) }` (`src/stream/errors.rs`) — the anticipated default,
  exactly three variants. The 1s state-lock timeout converts to
  `Transient` via `From<timed_locks::Error>`; `Branch` attach/detach
  failures map to `Fatal` at the seam impl.
- Coordinator maps with `From<PipelineError> for SignalError`:
  `NotReady → NotReady` (503 + Retry-After), `Transient → PipelineBusy`
  (new variant, 503 + Retry-After), `Fatal → Pipeline` (500). No
  `SignalError::Pipeline` is built from `to_string()` flattening
  anywhere (grep-verified); retry semantics survive end-to-end.
- `MyError` deleted. SDP validation lives in `domain::SdpError`;
  GStreamer plumbing detail in `stream::StreamError`. Bonus:
  `From<SdpError> for SignalError` unwraps the message, fixing the
  doubled "Invalid SDP: Invalid SDP:" body prefix (a residual noted at
  the end of the signaling-rebuild run).
- Error-contract tests extended in `signal/errors.rs` (mapping + status +
  Retry-After + no-double-prefix); coordinator test pins 503+Retry-After
  for an injected transient add_branch failure.

**Decisions**

- `PipelineLifecycle` stays on `anyhow::Error`: its only consumer is the
  supervisor, which logs and restarts regardless of variant. Rejected
  alternative: typing the lifecycle too — no caller would branch on it.
- "Pipeline is not initialized" maps to `NotReady` in `add_branch`
  (between restarts = retryable) and `Transient` in `remove_branch`.

**C6 decision: not done.** The review says do it only if C4 makes typed
Offer/Answer fall out naturally. It did not: the domain split produced
`SdpError`, but direction policy remains two small, tested handler
checks; direction-typed newtypes would be new design work, not fallout.

**Test results:** 34 unit + 10 integration green (3 new unit tests);
`--ignored` e2e passes at final HEAD in isolation (18s, clean exit).

**Done-when check:** no `SignalError::Pipeline(String)` from
`to_string()` flattening ✓ · error-contract tests extended for the new
mapping ✓.
