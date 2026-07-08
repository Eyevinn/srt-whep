# srt-whep — Architecture Review & Deepening Opportunities

**Date:** 2026-07-07
**Baseline:** `main` @ `f1867d3` (signaling-plane rebuild merged). `cargo test`: 23 unit + 9 HTTP integration tests green; 1 `#[ignore]`d GStreamer e2e.
**Audience:** agents picking up individual candidates below. Each candidate is independently executable.

> **Test environment (macOS, this machine):** `cargo test` aborts with a dyld error unless you first run
> `export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib`

---

## Vocabulary

Used consistently below (from the improve-codebase-architecture skill):

- **Module** — anything with an interface and an implementation.
- **Interface** — everything a caller must know: types, invariants, error modes, ordering, config. Not just signatures.
- **Deep** — much behaviour behind a small interface; **shallow** — interface nearly as complex as the implementation.
- **Seam** — where an interface lives; a place behaviour can be swapped without editing in place. One adapter = hypothetical seam; two = real seam.
- **Locality** — change/bugs/knowledge concentrated in one place. **Leverage** — what callers get from depth.
- **Deletion test** — delete the module: if complexity vanishes it was a pass-through; if it reappears across N callers it was earning its keep.

Domain terms (no `CONTEXT.md` exists yet — see Candidate 5): **Connection** (one WHEP viewer's signaling lifecycle, owned by the coordinator), **Branch** (that connection's per-viewer GStreamer elements: `whipclientsink` + queues), **Loopback WHIP** (the in-process `whipclientsink` POSTing its offer back to our own HTTP server), **Coordinator** (the signaling actor), **Supervisor** (the restart loop that reruns the pipeline after EOS/error), **Watchdog** (consecutive-failure counter that forces a pipeline restart), **Sweep** (the coordinator's periodic deadline reaper).

---

## Constraints: decided questions — do not re-litigate

`docs/superpowers/specs/2026-07-07-signaling-plane-rebuild-design.md` is an approved design review (ADR-equivalent). These decisions stand:

1. **Keep the Loopback WHIP bridge.** It is load-bearing: it's why `stream/` never imports `signal/` and the dependency graph is acyclic. An in-process signaller is *explicitly deferred* ("revisit later") — do not propose it as part of any candidate below.
2. **The coordinator actor is the single owner of connection state *and* of branch add/remove calls, serialized by its mailbox.** Consequence (accepted trade-off): a slow GStreamer branch operation stalls the whole signaling plane (list/remove/sweep wait behind it). Do not un-serialize branch calls without formally revisiting the spec.
3. **Per-connection failure isolation + watchdog fallback** — implemented and pinned by tests. Don't change semantics.
4. **No new CLI flags for coordinator timeouts** was a scope decision for the rebuild, not a permanent ban; exposing `CoordinatorConfig` later is fair game.

## What is already deep — do not regress

- **Coordinator actor + state machine** (`src/signal/coordinator.rs`) — small interface (`SignalHandle`, 6 methods) hiding timeouts, sweeps, waiter cleanup, watchdog. Thoroughly unit-tested with a paused clock.
- **`SignalError` + `ResponseError` impl** (`src/signal/errors.rs`) — one deep, typed, tested error language mapping to the HTTP contract (incl. `Retry-After`).
- **Routes as thin adapters** (`src/routes/`) — parse/validate → `SignalHandle` → status. Deletion test: they'd reappear as HTTP glue; correct as-is.
- **GStreamer branch add/remove craft** (`src/stream/gst_pipeline.rs:90-210, 646-739`) — pad probes, pause/remove-pad/resume, `call_async_future` off the tokio thread. Hard-won behaviour; deepen *around* it (Candidate 3), don't rewrite it casually.
- **Test pyramid**: unit (paused clock) → HTTP integration (`tests/signaling.rs`, real server + `TestPipeline` fake) → `#[ignore]`d wedge-risk e2e (`tests/e2e_gstreamer.rs`).
- **`SessionDescription` newtype** (`src/domain/session_description.rs`) — parse-don't-validate; direction *facts* in the type, direction *policy* in handlers. Clean split.

---

## Candidate 1 — Split the `PipelineBase` seam into its two real interfaces

**Strength: Strong** · Effort: ~half a day · Risk: low (pure refactor, pinned by existing tests)

**Files:** `src/stream/pipeline.rs:70-82` (trait), `:85-148` (`TestPipeline`), `src/signal/coordinator.rs`, `src/signal/mod.rs:22`, `src/utils.rs`, `src/main.rs`, `tests/e2e_gstreamer.rs`.

### Problem

`PipelineBase` is one 10-method trait serving two disjoint callers:

- The **coordinator** uses exactly 4: `ready`, `add_connection`, `remove_connection`, `quit` (`coordinator.rs:124,135,231,275,281`).
- The **supervisor** (`PipelineGuard` + `main.rs`) uses the rest: `init`, `run`, `end`, `clean_up`.
- `print()` has **zero callers** anywhere — pure interface bloat.

This is a shallow interface by conflation: every implementor fakes 10 methods so each caller can use its half (`TestPipeline` implements 5 no-ops). The trait also drags `clap`-derived `Args` into its signature (`init(&mut self, args: &Args)`), coupling the seam to the CLI. And the two adapters have **divergent contracts** behind the same signature: `SharablePipeline::add_connection` re-checks `ready()` internally (`gst_pipeline.rs:91`); `TestPipeline::add_connection` doesn't — so unit tests can't catch a caller that skips the readiness check.

### Solution

Split along the real caller boundary; keep both seams real (two adapters each):

- **`BranchControl`** (name it in domain language) — the coordinator's seam: `ready`, `add_branch`, `remove_branch`, `quit`. Renaming `add_connection → add_branch` fixes a naming lie: the pipeline adds a *branch* for a connection, it doesn't own connections.
- **`PipelineLifecycle`** — the supervisor's seam: `init`, `run`, `end`, `clean_up`. Needed as a trait (not inherent methods) because Candidate 2 wants supervisor unit tests against a controllable fake.
- Delete `print()` from the trait; keep it as an inherent debug method on `SharablePipeline`.
- Move `Args` out of the lifecycle signature if cheap (e.g. `SharablePipeline` already holds its config from `new(args)`); otherwise defer to Candidate 3.
- Coordinator's generic bound becomes `P: BranchControl`; `TestPipeline` implements both traits but the branch fake shrinks to what tests assert.

### Before / after

```
BEFORE                                      AFTER
             PipelineBase (10 methods)                  BranchControl (4)      PipelineLifecycle (4)
            ┌──────────────────────────┐               ┌────────────────┐     ┌──────────────────┐
Coordinator │ ready  add_conn  rm_conn │  Coordinator ─▶│ ready add_br   │     │ init run         │◀─ Supervisor
     ─────▶ │ quit   init  run  end    │ ◀───── main    │ rm_br quit     │     │ end clean_up     │
            │ clean_up  print(dead)    │                └───────┬────────┘     └────────┬─────────┘
            └──────────────────────────┘                        │      implemented by   │
   both callers see all 10; fake fakes 10          SharablePipeline + TestPipeline (each seam: 2 adapters)
```

### Benefits

- **Leverage:** the coordinator's dependency shrinks 10 → 4 methods; what a signal-plane reader must understand about the pipeline drops to the branch contract.
- **Locality:** lifecycle ordering knowledge (`init` before `run` before anything) concentrates in the supervisor seam instead of leaking into every implementor.
- **Tests:** the fake stops carrying no-ops; Candidate 2's supervisor tests get the seam they need; the divergent `ready`-recheck contract becomes visible and decidable.

**Done when:** both traits exist with the coordinator bound to `BranchControl` only; `print` gone from all trait surfaces; whole suite green unchanged.

---

## Candidate 2 — Gather the Supervisor into one module (and fix shutdown)

**Strength: Strong** · Effort: ~1 day · Risk: medium (touches process lifecycle; e2e covers it)

**Files:** `src/main.rs:26-61`, `src/utils.rs` (`PipelineGuard`, whole file), `tests/e2e_gstreamer.rs:70-86`, `tests/signaling.rs:46-56`, `src/startup.rs`.

### Problem

The supervisor — "run the pipeline; on EOS/error clean up, reset signaling, rerun; on shutdown stop everything" — exists, but as scatter:

- The **restart loop** is inlined in `main()` (`main.rs:33-50`): `AtomicBool` + `loop` + fixed 1s sleep (no backoff).
- The **cleanup half** hides in a crate-root file named `utils.rs` as `PipelineGuard`, whose work happens in `Drop` via `tokio_async_drop!` (`utils.rs:43-53`) — async-in-Drop that **panics on a current-thread runtime** (documented cause of a known e2e hang; see project memory).
- The **reset contract** with the coordinator (`Command::Reset`) is invisible from main.
- Because the loop isn't callable, **`tests/e2e_gstreamer.rs:70-86` copy-pastes it verbatim** ("Supervise the pipeline exactly like main.rs does") — app wiring is now duplicated in three places (`main.rs`, `signaling.rs::spawn_app`, e2e).
- **Shutdown is defective:** actix installs its own SIGINT handler (default), so `run(...)?.await?` (`main.rs:53`) returns on the *first* Ctrl-C — then `signal::ctrl_c().await` (`main.rs:55`) waits for a **second** Ctrl-C before the pipeline loop is stopped. Between the two presses the process runs headless: SRT ingest alive, no HTTP. The coordinator is never actively shut down at all (it dies only when the last `SignalHandle` drops).

Deletion test on `PipelineGuard`: delete it and the run+cleanup+reset choreography reappears in `main` *and* the e2e test — it earns its keep, but it's misfiled and half a module.

### Solution

One `supervisor` module (e.g. `src/supervisor.rs`, absorbing and deleting `src/utils.rs`) that owns the whole story behind a small interface:

- `Supervisor::run(pipeline, args, signal, shutdown)` — the loop: `init` → `run` → *explicit* `cleanup()` (pipeline `clean_up` + `signal.reset()`, no `Drop` magic, `tokio_async_drop` dependency deleted) → restart with backoff, until the shutdown token fires.
- Shutdown becomes one signal, one owner: `main` listens for Ctrl-C **once**, actix gets `.disable_signals()`, and a `tokio_util`-style cancellation token (or `watch` channel) fans out to HTTP server stop + supervisor stop (EOS → join). Kills the double-Ctrl-C defect.
- An `assemble()` / `Application` builder (zero2prod idiom this repo descends from) that constructs pipeline + coordinator + server in one place, used by `main`, `spawn_app`, and the e2e test — the wiring exists once.

### Before / after

```
BEFORE                                          AFTER
main.rs ──inline loop──┐   e2e test ──copy of loop──┐        ┌─────────── supervisor ───────────┐
  AtomicBool, sleep(1s)│     "exactly like main.rs" │        │ loop{init→run→cleanup→reset}     │
        ▼              ▼                            ▼        │ backoff · shutdown token · join  │
  PipelineGuard::Drop (tokio_async_drop!) ── signal.reset    └──────▲──────────────▲────────────┘
  Ctrl-C #1 → actix stops · Ctrl-C #2 → pipeline stops         main─┘   e2e/spawn_app┘  (one wiring fn)
                                                               one Ctrl-C → token → server + supervisor
```

### Benefits

- **Locality:** restart policy, reset contract, and shutdown ordering live in one named module; the e2e test stops drifting from production wiring by construction.
- **Leverage:** `main.rs` shrinks to parse-args → assemble → await shutdown.
- **Tests:** with Candidate 1's `PipelineLifecycle` seam, extend `TestPipeline` with a controllable `run()` (blocks until released) and unit-test: restart-after-error, reset-sent-on-cleanup, shutdown-stops-loop, backoff. Today none of this is testable.
- Fixes a real user-facing defect (double Ctrl-C) and removes a panic-prone dependency (`tokio_async_drop`).

**Done when:** `src/utils.rs` deleted; e2e uses the production supervisor; one Ctrl-C exits cleanly (manually verified via `/run`-style smoke); supervisor unit tests exist; suite green.

---

## Candidate 3 — Deepen the pipeline module: encode lifecycle, hide the lock, name the Branch

**Strength: Strong (staged — do after 1 & 2)** · Effort: 2–4 days across stages · Risk: highest here; the `#[ignore]` e2e wedge test is the safety net — run it per stage

**Files:** `src/stream/gst_pipeline.rs` (all 740 lines), `src/stream/pipeline.rs`, `src/startup.rs:20`.

### Problem

The signaling rebuild deliberately excluded this module ("No GStreamer element logic changes"), so it still carries the pre-rebuild idioms. Its *implementation* is deep; its *interface* leaks four kinds of knowledge a caller must hold:

1. **The lock is public.** `SharablePipeline` `Deref`s to `Arc<Mutex<PipelineWrapper>>` (`gst_pipeline.rs:45-57`), so any caller can lock internals; helper docs say "MUST be called when the pipeline is in locked state" — invariants by comment. `remove_connection` holds the 1s `timed_locks::Mutex` **across `.await`s** of GStreamer async state changes (`:184-207`), so a slow teardown surfaces as spurious `LockTimeout` errors to *other* callers.
2. **Lifecycle by runtime error.** `pipeline: Option<Pipeline>` means every method begins with an is-it-initialized dance; ordering (`init → run → branch ops → end/quit → clean_up`) is enforced nowhere. Bonus bug: the not-initialized log line reads "Pipeline is not missing" (`:68`).
3. **The topology's interface is magic strings.** Static elements are found by name (`"demux"`, `"output_tee_video"`, `"audio-queue"`); per-connection Branch elements follow conventions (`"video-queue-{id}"`, `"whip-sink-{id}"`) that `add_connection`, `remove_connection`, and `init`'s callbacks must all independently agree on. The Loopback WHIP URL template `http://localhost:{port}/whip_sink/{id}` (`:120`) must match the route table in `startup.rs:20` — two files, agreement by coincidence.
4. **Blocking loop on an async worker.** `run()` calls `glib::MainLoop::run()` (`:545`) — synchronous — inside an `async fn`, pinning a tokio worker for the whole session. Already bit us: the e2e hangs on a current-thread runtime (known deferred issue).

Also: `link_media`'s h264/h265/audio arms are copy-paste triplets (`:314-400`), and `create_custom_queue` takes stringly-typed properties (`"0", "0", "no"`, `:615`).

### Solution (stages, each independently shippable)

- **3a — Make the lock private.** Remove `Deref`/`DerefMut`; all locking stays inside the module. Narrow critical sections so no guard is held across GStreamer `call_async_future` awaits (snapshot the elements you need, drop the guard, then await).
- **3b — Extract a `Branch` submodule.** One place owns the per-connection element names, construction, linking, state-sync, and the pad-probe teardown dance: `Branch::attach(&Pipeline, &id, port) -> Branch`, `Branch::detach(self)`. `add/remove_connection` become thin calls. Element-name conventions and the Loopback WHIP URL template become constants/fns here; `startup.rs` imports the same route constant (`WHIP_SINK_ROUTE`) so the contract lives in exactly one file. Dedup `link_media` arms with a small codec table (parser element name per caps prefix).
- **3c — Own the GLib main loop on a dedicated thread.** The module spawns/joins its own OS thread (or `spawn_blocking`) for `main_loop.run()`; `run()` becomes "await a completion signal" instead of blocking the executor. This unblocks running the e2e on any runtime flavor and removes the sync-in-async trap from the interface.
- **3d (optional, revisit spec first)** — encode lifecycle as a typestate or internal state enum so "called before init" is unrepresentable rather than a runtime error.

### Before / after

```
BEFORE                                            AFTER
 SharablePipeline = Arc<Mutex<Option<Pipeline>>>   SharablePipeline (lock private, no Deref)
   ▲ Deref exposes lock to everyone                  ├─ lifecycle: init/run(end/quit/clean_up)
   ├─ "video-queue-{id}" convention in 3 places      │    └─ glib MainLoop on its own thread
   ├─ whip URL here ↔ route table in startup.rs      └─ Branch module (the ONLY place that knows
   └─ guard held across GStreamer awaits                  element names, linking, teardown probe,
                                                          WHIP_SINK_ROUTE — shared with startup.rs)
```

### Benefits

- **Locality:** a branch bug means opening one file; renaming an element or changing the loopback route is a one-place change instead of a grep-and-pray.
- **Leverage:** callers (coordinator via `BranchControl`, supervisor via `PipelineLifecycle`) keep tiny interfaces while the module absorbs the locking + naming + threading knowledge.
- **Tests:** `Branch` naming/URL functions become pure and unit-testable; the wedge e2e keeps covering the live-pipeline behaviour; 3c makes that e2e runnable under more runtimes (today's known hang).

**Done when:** no `Deref` to the lock; one definition of branch names + whip route; `cargo test` green and the `--ignored` e2e passes at least as well as at baseline.

---

## Candidate 4 — Type the pipeline error seam (stop collapsing retryable into 500)

**Strength: Worth exploring** · Effort: ~half a day after Candidate 1 · Risk: low

**Files:** `src/domain/errors.rs` (`MyError`), `src/stream/pipeline.rs` (trait signatures), `src/signal/coordinator.rs:131,136,235`, `src/signal/errors.rs`.

### Problem

Three error languages coexist: `MyError` (a grab-bag shared by SDP parsing *and* GStreamer internals), `anyhow::Error` (the entire pipeline trait surface), and `SignalError` (the deep, HTTP-mapped one). The stream→signal seam is stringly-typed: the coordinator flattens everything to `SignalError::Pipeline(e.to_string())`. So a transient `LockTimeout` (retryable — exactly what `SignalError` encodes `Retry-After` semantics for), a `MissingElement` (a bug), and a real operation failure all become opaque 500s. `InvalidSDP` is duplicated across `MyError` and `SignalError`.

### Solution

- Give the `BranchControl` seam a small typed error: `PipelineError { NotReady, Transient(String), Fatal(String) }` (taxonomy over taxonomy-detail — three variants is enough for policy).
- Coordinator maps `Transient → SignalError::NotReady`-style 503 + `Retry-After`, `Fatal → Pipeline` 500; watchdog policy can then distinguish "pipeline wedged" from "caller raced a teardown".
- Split `MyError`: SDP validation error stays in `domain` (`SdpError`), GStreamer variants move into `stream`. Delete the `MyError` name.

### Benefits

- **Leverage:** retry semantics survive end-to-end instead of dying at the seam; the HTTP mapping stays concentrated in `SignalError` where it's already tested.
- **Locality:** each module's error vocabulary lives with the module; no more grab-bag.

**Done when:** no `SignalError::Pipeline(String)` construction from `to_string()` flattening; error-contract tests in `signal/errors.rs` extended for the new mapping.

---

## Candidate 5 — Hygiene: dead weight, CI, and a CONTEXT.md

**Strength: Strong (quick wins, high AI-navigability value)** · Effort: ~2 hours · Risk: minimal

**Files:** `Cargo.toml`, `.github/workflows/pull-request.yml`, `.gitignore`, `src/stream/pipeline.rs:81`, `src/domain/session_description.rs:36-38`, `src/routes/whip_handler.rs:24-31`, `src/stream/gst_pipeline.rs:68`, new `CONTEXT.md`.

### Problem / solution list

1. **Dead dependencies** (zero2prod template residue, verified unused in `src/`): `config`, `toml`, `secrecy`, `validator`, `serde-aux`, `unicode-segmentation`, `chrono`, `derive_more`. `reqwest` is main-deps but only used by tests → move to dev-deps. Remove and rebuild.
2. **CI never runs the tests.** The PR workflow only builds a Docker image. Add a `cargo test` job (install the same GStreamer apt packages as `publish.yml`; cache cargo). The whole 32-test suite currently only guards machines that remember the dyld export.
3. **`.gitignore` gap:** `pipe/`, `source/`, `srt/` in the repo root are untracked GStreamer `.dot` dump debris (`GST_DEBUG_DUMP_DOT_DIR` output). Ignore them (or point dumps at the already-ignored `/dot`), delete the debris.
4. **Dead code:** `PipelineBase::print` (no callers — folded into Candidate 1), `SessionDescription::is_empty` (no production callers; `parse` already rejects empty). `Established.since` is intentionally dormant (commented as a later task) — keep.
5. **Unroutable WHIP resource URL:** `whip_handler` returns `Location: /whip_sink/{conn_id}/{resource_id}` but no such route exists — a client honoring it 404s. Either register a handler (DELETE → `remove_connection`) or return the plain `/whip_sink/{conn_id}` location it can actually reach.
6. **Log typo:** `gst_pipeline.rs:68` logs "Pipeline is not missing" on the missing path.
7. **Create `CONTEXT.md`** with the domain glossary (top of this document is a starting point). Real drift exists to pin down: the HTTP surface says **channel**, the signal plane says **connection**, the stream plane says **branch** — one term per concept, documented, ends the drift. Future architecture/design skills in this repo read `CONTEXT.md` and `docs/adr/` (consider promoting the rebuild design spec's decision table into `docs/adr/` while at it).

---

## Candidate 6 — Direction-typed SDP (Offer/Answer)

**Strength: Speculative**

`SessionDescription` exposes direction as a queryable fact (`is_sendonly`) and the handlers enforce policy. Typed `SdpOffer`/`SdpAnswer` newtypes (parse-time direction) would let the coordinator's `Command`s say what they mean and make wrong-direction SDP unrepresentable past the edge. Marginal today: the policy sites are exactly two handler checks, both tested. Only worth doing if Candidate 4's domain-error split is happening anyway and the types fall out naturally. Do not do this first.

---

## Top recommendation

**Start with Candidate 1, then Candidate 2 in the same arc.** Rationale:

- Candidate 1 is a half-day, low-risk seam split fully pinned by existing tests — and Candidate 2's supervisor tests *require* the `PipelineLifecycle` seam it creates.
- Candidate 2 then completes the story this repo's own rebuild started: the signaling plane got a deep, tested module; the supervisor — its exact peer — is still scatter across `main.rs`, a misnamed `utils.rs`, and a copy-paste block in the e2e test. It also fixes the one live user-facing defect found in this review (double Ctrl-C shutdown) and removes the panic-prone `tokio_async_drop`.
- Candidate 3 is the biggest payoff (the 740-line module is where future feature work — codecs, multiple inputs — will land) but should sit on top of the supervisor/lifecycle seams, staged 3a→3c, with the wedge e2e run between stages.
- Candidate 5 can run in parallel with anything (independent files); do the CI test job first — it protects every other candidate.

Suggested order: **1 → 2 → 5 → 3a → 3b → 3c → 4 → (6 only if 4 invites it)**.

## Verification baseline for all candidates

```bash
export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
cargo test                                   # 23 unit + 9 integration, green at f1867d3
cargo test --test e2e_gstreamer -- --ignored --nocapture   # wedge e2e; requires GStreamer; known flaky/hang history
```
