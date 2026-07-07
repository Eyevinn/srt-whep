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
