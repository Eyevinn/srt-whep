# C2 — Supervisor Module Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Gather the pipeline supervisor (run → cleanup → reset → rerun loop) into one `src/supervisor.rs` module behind a small interface, wire app assembly in one place used by `main.rs`, `tests/signaling.rs`, and the e2e test, and fix the double-Ctrl-C shutdown defect.

**Architecture:** A `Supervisor` struct owns the restart loop (init → run → explicit cleanup → backoff), listening on a `tokio::sync::watch` shutdown channel; on shutdown it sends EOS via `PipelineLifecycle::end()` and joins the run task (bounded). An `Application::assemble()` fn in `startup.rs` (zero2prod idiom) builds pipeline+coordinator+supervisor+server once; `main` sends one shutdown token on the first Ctrl-C. `src/utils.rs` (`PipelineGuard`, async-in-Drop via `tokio_async_drop`) is deleted.

**Tech Stack:** tokio (`watch`, `spawn`, `timeout` — all already-enabled features), actix-web (`.disable_signals()`, `ServerHandle::stop`), existing `PipelineLifecycle` seam from C1.

## Global Constraints

- Binding (review doc): keep loopback-WHIP bridge; branch calls stay serialized in the coordinator; per-connection isolation + watchdog semantics unchanged.
- New behaviour is developed test-first; existing suite (23 unit + 9 integration) green at every commit.
- Every test shell needs `export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib`.
- Anticipated defaults (driver): shutdown via `tokio::sync::watch` (no new deps); actix gets `.disable_signals()`; assembly fn returns a struct holding server + supervisor handle + `SignalHandle`.
- Commit subjects prefixed `feat(c2):` / `refactor(c2):` / `test(c2):`.
- The real `SharablePipeline::run()` blocks its worker thread in `glib::MainLoop::run()` until C3c; therefore the supervisor must run the pipeline on a **spawned task** and unblock it from outside via `end()` — never `select!` directly over `pipeline.run()`.

---

### Task 1: Controllable `TestPipeline` (lifecycle recording + gated `run`)

**Files:**
- Modify: `src/stream/pipeline.rs` (TestPipelineState, TestPipeline, its trait impls, unit test)

**Interfaces:**
- Produces: `TestPipelineState` gains `pub init_count: u32`, `pub run_count: u32`, `pub end_count: u32`, `pub cleanup_count: u32`. `TestPipeline` gains `pub fn finish_run(&self)` (release a parked `run()` with `Ok`), `pub fn fail_run(&self, msg: &str)` (release with `Err`). `run()` parks until released; `end()` and `quit()` also release it with `Ok` (EOS / forced-quit semantics, matching `SharablePipeline`).

- [ ] **Step 1: Extend the state + fake (test for the fake first)**

Add to the `tests` module in `src/stream/pipeline.rs`:

```rust
#[tokio::test]
async fn test_pipeline_run_parks_until_released() {
    let pipeline = TestPipeline::default();

    let runner = {
        let pipeline = pipeline.clone();
        tokio::spawn(async move { pipeline.run().await })
    };
    tokio::task::yield_now().await;
    assert_eq!(1, pipeline.snapshot().run_count);
    assert!(!runner.is_finished());

    pipeline.fail_run("boom");
    let result = runner.await.unwrap();
    assert_eq!("boom", result.unwrap_err().to_string());

    // end() releases a parked run with Ok (EOS semantics).
    let runner = {
        let pipeline = pipeline.clone();
        tokio::spawn(async move { pipeline.run().await })
    };
    tokio::task::yield_now().await;
    pipeline.end().await.unwrap();
    assert!(runner.await.unwrap().is_ok());

    pipeline.init().await.unwrap();
    pipeline.clean_up().await.unwrap();
    let snap = pipeline.snapshot();
    assert_eq!(1, snap.init_count);
    assert_eq!(2, snap.run_count);
    assert_eq!(1, snap.end_count);
    assert_eq!(1, snap.cleanup_count);
}
```

- [ ] **Step 2: Run it, verify it fails** (`cargo test test_pipeline_run_parks -- --nocapture` → compile error: no `finish_run`/counters)

- [ ] **Step 3: Implement**

```rust
#[derive(Clone, Debug, Default)]
pub struct TestPipelineState {
    pub ready: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub quit_count: u32,
    pub init_count: u32,
    pub run_count: u32,
    pub end_count: u32,
    pub cleanup_count: u32,
    next_run_error: Option<String>,
}

#[derive(Clone, Default)]
pub struct TestPipeline {
    state: Arc<std::sync::Mutex<TestPipelineState>>,
    run_gate: Arc<tokio::sync::Notify>,
}
```

`run()` records `run_count`, awaits `run_gate.notified()`, then returns `Err` iff `next_run_error` was set (taking it). `finish_run`/`fail_run`/`end`/`quit` call `run_gate.notify_one()` (with `fail_run` setting `next_run_error` first). `init`/`clean_up`/`end` bump their counters. All existing methods (`ready`, `add_branch`, `remove_branch`, `set_ready`, `snapshot`) keep their behaviour; internal field accesses change `self.0` → `self.state`.

- [ ] **Step 4: `cargo test` — full suite green (23+1 new unit, 9 integration)**
- [ ] **Step 5: Commit** `test(c2): make TestPipeline lifecycle controllable`

### Task 2: `Supervisor` module, test-first

**Files:**
- Create: `src/supervisor.rs`
- Modify: `src/lib.rs` (add `pub mod supervisor;` — keep `utils` until Task 3)

**Interfaces:**
- Consumes: `PipelineLifecycle` (C1), `SignalHandle::reset()`, `TestPipeline` helpers from Task 1.
- Produces: `Supervisor::spawn(pipeline: P, signal: SignalHandle, shutdown: watch::Receiver<bool>) -> tokio::task::JoinHandle<()>` where `P: PipelineLifecycle + 'static`.

**Semantics (the module's whole story):**
1. Loop until the shutdown watch reads `true`: `init()` → run `pipeline.run()` **as a spawned task** → on completion `cleanup()` = `pipeline.clean_up()` + `signal.reset()` (explicit, no Drop magic).
2. On shutdown while running: call `pipeline.end()` (EOS pops the possibly-thread-blocked `run`), then join the run task **bounded** (5s; on timeout log an error and abandon the task — the process is exiting).
3. Restart delay: 1s base, doubling per consecutive failed run (an `Err` from init or run), capped at 30s, reset to base after a clean (EOS) run. The delay sleep is `select!`ed against shutdown so a token during backoff exits promptly.
4. Init failure counts as a failed run: still `cleanup()` (clean_up is a no-op on an uninitialized pipeline; reset fails in-flight handshakes, correct while restarting), then backoff.

- [ ] **Step 1: Write failing unit tests in `src/supervisor.rs`'s `#[cfg(test)]` module**

```rust
use super::Supervisor;
use crate::signal::{spawn_coordinator, CoordinatorConfig};
use crate::stream::TestPipeline;
use std::time::Duration;
use tokio::sync::watch;

async fn wait_until(pipeline: &TestPipeline, f: impl Fn(&crate::stream::TestPipelineState) -> bool) {
    for _ in 0..500 {
        if f(&pipeline.snapshot()) { return; }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition never reached: {:?}", pipeline.snapshot());
}

#[tokio::test(start_paused = true)]
async fn restarts_after_a_failed_run_and_resets_signaling() {
    let pipeline = TestPipeline::default();
    let signal = spawn_coordinator(pipeline.clone(), CoordinatorConfig::default());
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);

    wait_until(&pipeline, |s| s.run_count == 1).await;
    pipeline.fail_run("gst blew up");

    // cleanup ran: pipeline cleaned and signaling reset, then a new run starts.
    wait_until(&pipeline, |s| s.cleanup_count == 1).await;
    wait_until(&pipeline, |s| s.run_count == 2).await;
    assert_eq!(2, pipeline.snapshot().init_count);
}

#[tokio::test(start_paused = true)]
async fn reset_on_cleanup_fails_inflight_handshakes() {
    let pipeline = TestPipeline::default();
    pipeline.set_ready(true);
    let signal = spawn_coordinator(pipeline.clone(), CoordinatorConfig::default());
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);
    wait_until(&pipeline, |s| s.run_count == 1).await;

    let waiter = {
        let signal = signal.clone();
        tokio::spawn(async move { signal.create_connection("a".into()).await })
    };
    wait_until(&pipeline, |s| s.added.len() == 1).await;

    pipeline.fail_run("gst blew up");
    let result = waiter.await.unwrap();
    assert!(matches!(result, Err(crate::signal::SignalError::Unavailable)));
}

#[tokio::test(start_paused = true)]
async fn shutdown_sends_eos_joins_and_stops_the_loop() {
    let pipeline = TestPipeline::default();
    let signal = spawn_coordinator(pipeline.clone(), CoordinatorConfig::default());
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);
    wait_until(&pipeline, |s| s.run_count == 1).await;

    shutdown_tx.send(true).unwrap();
    sup.await.unwrap();

    let snap = pipeline.snapshot();
    assert_eq!(1, snap.end_count);     // graceful EOS was requested
    assert_eq!(1, snap.cleanup_count); // cleaned up exactly once
    assert_eq!(1, snap.run_count);     // and never restarted
}

#[tokio::test(start_paused = true)]
async fn backoff_doubles_on_consecutive_failures_and_resets_on_success() {
    let pipeline = TestPipeline::default();
    let signal = spawn_coordinator(pipeline.clone(), CoordinatorConfig::default());
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);
    let _sup = Supervisor::spawn(pipeline.clone(), signal.clone(), shutdown_rx);

    wait_until(&pipeline, |s| s.run_count == 1).await;
    let t0 = tokio::time::Instant::now();
    pipeline.fail_run("1st");
    wait_until(&pipeline, |s| s.run_count == 2).await;
    let first_gap = t0.elapsed();

    let t1 = tokio::time::Instant::now();
    pipeline.fail_run("2nd");
    wait_until(&pipeline, |s| s.run_count == 3).await;
    let second_gap = t1.elapsed();

    // Paused clock: gaps are timer-driven. Second delay must be ~double.
    assert!(second_gap >= first_gap * 2 - Duration::from_millis(200),
        "expected doubled backoff, got {:?} then {:?}", first_gap, second_gap);

    // A clean EOS run resets the backoff.
    let t2 = tokio::time::Instant::now();
    pipeline.finish_run();
    wait_until(&pipeline, |s| s.run_count == 4).await;
    let after_success_gap = t2.elapsed();
    assert!(after_success_gap < second_gap,
        "expected backoff reset, got {:?} after {:?}", after_success_gap, second_gap);
}
```

- [ ] **Step 2: Run, verify failure** (module doesn't exist yet)

- [ ] **Step 3: Implement `src/supervisor.rs`**

```rust
//! The pipeline supervisor: runs the pipeline, cleans up and resets
//! signaling when it stops, and reruns it with backoff — until shutdown.
use crate::signal::SignalHandle;
use crate::stream::PipelineLifecycle;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

const BASE_RESTART_DELAY: Duration = Duration::from_secs(1);
const MAX_RESTART_DELAY: Duration = Duration::from_secs(30);
const SHUTDOWN_JOIN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct Supervisor<P: PipelineLifecycle> {
    pipeline: P,
    signal: SignalHandle,
    shutdown: watch::Receiver<bool>,
}

impl<P: PipelineLifecycle + 'static> Supervisor<P> {
    pub fn spawn(pipeline: P, signal: SignalHandle, shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(Self { pipeline, signal, shutdown }.run())
    }

    async fn run(mut self) { /* loop per semantics above */ }
    async fn run_pipeline_until_stopped(&mut self) -> Result<(), anyhow::Error> { /* init + spawned run + shutdown select */ }
    async fn cleanup(&self) { /* clean_up + signal.reset, both logged on error */ }
}
```

Key implementation detail for `run_pipeline_until_stopped`:

```rust
self.pipeline.init().await?;
let run_task = tokio::spawn({ let p = self.pipeline.clone(); async move { p.run().await } });
tokio::select! {
    res = run_task /* JoinHandle is Unpin */ => res?,
    _ = wait_for_shutdown(&mut self.shutdown) => {
        let _ = self.pipeline.end().await;
        match tokio::time::timeout(SHUTDOWN_JOIN_TIMEOUT, run_task).await {
            Ok(joined) => joined??,
            Err(_) => tracing::error!("pipeline did not stop within {:?}; abandoning it", SHUTDOWN_JOIN_TIMEOUT),
        }
        Ok(())
    }
}
```

with `wait_for_shutdown` treating a dropped sender as shutdown:

```rust
async fn wait_for_shutdown(shutdown: &mut watch::Receiver<bool>) {
    // Err means the sender is gone: the application is tearing down.
    let _ = shutdown.wait_for(|&stop| stop).await;
}
```

- [ ] **Step 4: `cargo test` — all green**
- [ ] **Step 5: Commit** `feat(c2): add the pipeline supervisor module`

### Task 3: `Application::assemble` + main on one Ctrl-C; delete `utils.rs`

**Files:**
- Modify: `src/startup.rs` (add `Application`; `.disable_signals()` on the server)
- Modify: `src/main.rs` (parse-args → assemble → await shutdown)
- Delete: `src/utils.rs`; Modify: `src/lib.rs` (drop `pub mod utils;`)
- Modify: `Cargo.toml` (remove `tokio-async-drop`)

**Interfaces:**
- Produces (in `startup.rs`):

```rust
pub struct Application {
    server: Server,
    supervisor: JoinHandle<()>,
    signal: SignalHandle,
    shutdown: watch::Sender<bool>,
    port: u16,
}

impl Application {
    /// One wiring point: coordinator + supervisor + HTTP server.
    pub fn assemble<P>(listener: TcpListener, pipeline: P, config: CoordinatorConfig) -> Result<Self, std::io::Error>
    where P: BranchControl + PipelineLifecycle + 'static;
    pub fn port(&self) -> u16;
    pub fn signal(&self) -> SignalHandle;
    /// Serve until `stop` resolves (or the server dies), then orderly
    /// shutdown: token → supervisor (EOS→join), graceful server stop.
    pub async fn run_until_stopped(self, stop: impl std::future::Future<Output = ()>) -> Result<(), std::io::Error>;
}
```

`assemble` spawns the coordinator and the supervisor; `run()` keeps its current signature for compatibility but gains `.disable_signals()`.

- `main.rs` becomes:

```rust
let args = Args::parse();
// telemetry unchanged
let pipeline = SharablePipeline::new(args.clone());
let listener = TcpListener::bind(format!("0.0.0.0:{}", args.port)).expect("WHEP port is already in use");
let app = Application::assemble(listener, pipeline, CoordinatorConfig::default())?;
app.run_until_stopped(async { let _ = tokio::signal::ctrl_c().await; }).await?;
```

- [ ] **Step 1: Implement (wiring change is pinned by the whole existing suite; no new unit test — integration tests in Task 4 exercise `assemble`)**
- [ ] **Step 2: `cargo build` + `cargo test` green; `grep -r tokio_async_drop src/` empty**
- [ ] **Step 3: Commit** `refactor(c2): assemble the app in one place; one Ctrl-C shutdown; drop PipelineGuard`

### Task 4: `tests/signaling.rs` uses `Application::assemble`

**Files:**
- Modify: `tests/signaling.rs` (`spawn_app` returns a `TestApp { address, pipeline, _app: ... }`-style handle or keeps `(String, TestPipeline)` while holding the Application alive via `tokio::spawn(app.run_until_stopped(...))`)

**Details:** `spawn_app` builds `Application::assemble(listener, TestPipeline, config)`, spawns `run_until_stopped` with a never-resolving stop future (`std::future::pending()`), returns `(address, pipeline)` as before so the 9 tests don't churn. The supervisor is inert during tests because `TestPipeline::run()` parks. **Race guard:** in `watchdog_restarts_pipeline_after_consecutive_failures`, after the third 503, wait until `snapshot().cleanup_count >= 1` (supervisor finished the post-quit reset) before driving the recovery exchange.

- [ ] **Step 1: Rewire `spawn_app`; adjust the watchdog test's wait**
- [ ] **Step 2: `cargo test` — 9 integration green, run it 3× to shake races**
- [ ] **Step 3: Commit** `test(c2): signaling tests run on the production wiring`

### Task 5: e2e uses the production supervisor and shutdown

**Files:**
- Modify: `tests/e2e_gstreamer.rs` (drop `PipelineGuard` import and the copy-pasted loop; assemble + one stop signal; teardown = production shutdown under an outer timeout)

**Details:** replace coordinator/server/supervisor blocks with `Application::assemble(listener, pipeline.clone(), config)`; drive the scenario unchanged; teardown: send the stop signal (oneshot → stop future), `tokio::time::timeout(15s, app_task)`; on timeout report and `std::process::exit(1)` (keep the existing no-panic-teardown pattern and the source `set_state(Null)`).

- [ ] **Step 1: Rewire**
- [ ] **Step 2: `pkill -9 -f e2e_gstreamer-` then `cargo test --test e2e_gstreamer -- --ignored --nocapture` with a 5-min timeout, ONCE, in isolation. A hang or regression vs baseline is a stop-condition finding.**
- [ ] **Step 3: Commit** `test(c2): e2e runs the production supervisor and shutdown path`

### Task 6: Manual one-Ctrl-C smoke test

- [ ] **Step 1:** `cargo build` then run `target/debug/srt-whep -i 127.0.0.1:9988 -o 127.0.0.1:9989 -p 8203` in the background with the GStreamer env; wait ~3s; send **one** SIGINT to the process; confirm the process exits (waitpid) within ~10s and logs the shutdown line. Before the fix this required two SIGINTs.
- [ ] **Step 2:** Record the smoke result + checkpoint entry in the progress log; commit `docs(c2): progress log`.

## Self-Review

- Spec coverage: scatter gathered (Task 2), utils.rs deleted + tokio_async_drop gone (Task 3), wiring exists once for main/spawn_app/e2e (Tasks 3–5), double-Ctrl-C fixed + manually verified (Tasks 3, 6), supervisor unit tests incl. restart/reset/shutdown/backoff (Task 2). ✓
- Placeholders: Task 2 Step 3 shows the two nontrivial bodies (select + wait_for_shutdown); the loop body is fully specified by the semantics list. Task 3–5 are rewires of code shown in-plan or already in-repo. ✓
- Type consistency: `Supervisor::spawn(P, SignalHandle, watch::Receiver<bool>) -> JoinHandle<()>` used identically in Tasks 2–3; `TestPipeline` helpers named `finish_run`/`fail_run` throughout. ✓
