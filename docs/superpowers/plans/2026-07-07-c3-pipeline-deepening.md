# C3 — Pipeline Module Deepening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Deepen `src/stream/gst_pipeline.rs` in three independently shippable stages: (3a) make the lock private and stop holding it across GStreamer awaits, (3b) extract a `Branch` submodule that owns all per-connection element names and the loopback WHIP URL/route contract, (3c) move the GLib main loop onto a dedicated OS thread so `run()` stops pinning a tokio worker.

**Architecture:** The module keeps its two small trait surfaces (`BranchControl`, `PipelineLifecycle` from C1) while absorbing locking, naming, and threading knowledge. Stage 3d (lifecycle typestate) is explicitly deferred — the review marks it "optional, revisit spec first".

**Tech Stack:** gstreamer-rs, glib MainLoop, timed_locks, tokio oneshot.

## Global Constraints

- Binding: do not regress the "already deep" list — especially the branch add/remove craft (pad probes, pause/remove-pad/resume, `call_async_future` off the tokio thread) — deepen around it, don't rewrite it.
- Suite green at every commit (30 unit + 10 integration, 1 ignored e2e); **run the `--ignored` e2e after every stage** — in isolation, `pkill -9 -f e2e_gstreamer-` before/after, 5-min timeout; a hang or regression vs baseline is a stop condition.
- Every test shell: `export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib` (full GStreamer env for the e2e, incl. `GST_PLUGIN_PATH` with the local gst-plugins-rs build).
- Run `cargo fmt` before every commit (pre-commit hook aborts otherwise).
- Commit subjects prefixed `refactor(c3a):`, `refactor(c3b):`, `refactor(c3c):`.
- Anticipated defaults (driver): `Branch` owns all per-connection element-name constants; `WHIP_SINK_ROUTE` const lives in `stream` and is imported by `startup.rs`.

---

### Task 1 (3a): Make the lock private; no guard across GStreamer awaits

**Files:**
- Modify: `src/stream/gst_pipeline.rs`

**Interfaces:**
- Produces: `SharablePipeline` without `Deref`/`DerefMut`; `PipelineWrapper` no longer `pub`. External surface is exactly the two traits + `new` + `print`.

- [ ] **Step 1: Verify no external lock users.** `grep -rn 'lock_err\|PipelineWrapper' src/ tests/ --exclude=gst_pipeline.rs` — expected: no hits (the e2e and coordinator only use trait methods). If a hit exists, stop and reassess.
- [ ] **Step 2: Remove the seam leak.** Delete the `impl Deref`/`impl DerefMut` blocks and the `std::ops::{Deref, DerefMut}` import; change `pub struct PipelineWrapper` to `struct PipelineWrapper` (module-private). All internal `self.lock_err()` calls become `self.0.lock_err()`.
- [ ] **Step 3: Narrow `remove_branch`.** It currently holds the timed guard across the pad-probe teardown awaits, so a slow teardown surfaces as spurious `LockTimeout` to other callers. Snapshot what's needed, drop the guard, then await:

```rust
async fn remove_branch(&self, id: String) -> Result<(), Error> {
    // Snapshot the pipeline handle (cheap GObject ref), then release the
    // lock: the teardown dance awaits GStreamer state changes and must not
    // hold the state lock while it does.
    let pipeline = {
        let pipeline_state = self.0.lock_err().await?;
        pipeline_state
            .pipeline
            .as_ref()
            .ok_or(MyError::FailedOperation(
                "Pipeline is not initialized".to_string(),
            ))?
            .clone()
    };
    tracing::debug!("Remove connection {} from pipeline", id);
    // ... existing name construction and removal calls, unchanged, using
    // the local `pipeline` handle ...
}
```

- [ ] **Step 4: Narrow `clean_up`.** Take the pipeline out under the lock, drop the guard, then do the async `set_state(Null)`:

```rust
async fn clean_up(&self) -> Result<(), Error> {
    let pipeline = { self.0.lock_err().await?.pipeline.take() };
    if let Some(pipeline) = pipeline {
        pipeline
            .call_async_future(move |pipeline| {
                let _ = pipeline.set_state(gst::State::Null).inspect_err(|e| {
                    tracing::error!("Failed to clean pipeline up: {}", e);
                });
            })
            .await;
    }
    Ok(())
}
```

  (Also clear `main_loop` there if the old code left a stale one: set `main_loop = None` in the same locked scope.)
- [ ] **Step 5:** Audit the remaining methods: `ready`/`add_branch`/`end`/`quit` hold the guard only across synchronous GStreamer calls — acceptable; `init`/`run` already release before blocking. No changes unless the audit finds an await under guard.
- [ ] **Step 6:** Full `cargo test`; then the e2e (isolated, timeout, pkill). Expected: 30+10 green; e2e passes/exits cleanly.
- [ ] **Step 7: Commit** `refactor(c3a): make the pipeline lock private and never hold it across awaits`

### Task 2 (3b): Extract the `Branch` submodule; one definition of names + WHIP route

**Files:**
- Create: `src/stream/branch.rs`
- Modify: `src/stream/gst_pipeline.rs` (add/remove_branch become thin calls; `link_media` codec table), `src/stream/mod.rs` (add `mod branch; pub use branch::WHIP_SINK_ROUTE; pub use branch::whip_sink_path;`), `src/startup.rs` (route table uses the const), `src/routes/whip_handler.rs` (Location uses `whip_sink_path`)

**Interfaces:**
- Produces (in `src/stream/branch.rs`):

```rust
/// The actix route template for the loopback WHIP endpoint — the single
/// definition shared by the HTTP route table, the Location header, and
/// the whipclientsink's endpoint URL.
pub const WHIP_SINK_ROUTE: &str = "/whip_sink/{id}";

/// Path of one connection's WHIP resource (route template instantiated).
pub fn whip_sink_path(id: &str) -> String;

pub(crate) struct Branch { /* id + derived element names */ }
impl Branch {
    pub(crate) fn for_id(id: &str) -> Branch;
    /// Create + link + state-sync this viewer's elements (whipsink,
    /// per-media queues) onto the pipeline's output tees. Synchronous;
    /// caller may hold the pipeline state lock.
    pub(crate) fn attach(&self, pipeline: &Pipeline, port: u32) -> Result<(), Error>;
    /// The teardown dance (tee pad probe, pause/remove-pad/resume,
    /// call_async_future). Async; caller must NOT hold the state lock.
    pub(crate) async fn detach(&self, pipeline: &Pipeline) -> Result<(), Error>;
}
```

- [ ] **Step 1 (TDD for the pure functions): failing unit tests in `branch.rs`:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_paths_derive_from_one_convention() {
        let branch = Branch::for_id("abc");
        assert_eq!("whip-sink-abc", branch.whip_sink_name());
        assert_eq!("video-queue-abc", branch.video_queue_name());
        assert_eq!("audio-queue-abc", branch.audio_queue_name());
        assert_eq!("/whip_sink/abc", whip_sink_path("abc"));
        assert!(whip_endpoint(8000, "abc").ends_with("/whip_sink/abc"));
        assert_eq!("http://localhost:8000/whip_sink/abc", whip_endpoint(8000, "abc"));
    }
}
```

  (`whip_endpoint(port, id)` is `pub(crate)` — used by `attach` for the signaller property; derive both it and `whip_sink_path` from `WHIP_SINK_ROUTE.replace("{id}", id)` so there is literally one template.)
- [ ] **Step 2:** Run: fails to compile (module missing).
- [ ] **Step 3: Implement `branch.rs`** by moving code, not rewriting: `attach` = the body of today's `add_branch` after its ready/lock preamble (whipsink creation, endpoint property, video/audio tee linking, `sync_state_with_parent` calls); `detach` = the bodies of `remove_branch_from_pipeline` + `remove_element_from_pipeline` (move both helpers into `branch.rs` as private fns, unchanged — this is the "hard-won behaviour", do not alter the sequencing). `gst_pipeline.rs`'s trait methods become: ready-check + lock/snapshot + `Branch::for_id(&id).attach(pipeline, port)` / `.detach(&pipeline).await`.
- [ ] **Step 4: One-template contract.** `startup.rs`: both `/whip_sink` routes use `WHIP_SINK_ROUTE` (import via `crate::stream::WHIP_SINK_ROUTE`); `whip_handler.rs`: Location header becomes `whip_sink_path(&conn_id)`. The integration tests keep their literal `"/whip_sink/{id}"` strings — they pin the public contract.
- [ ] **Step 5: Codec-table dedup in `link_media`** (inside `init`'s no-more-pads callback): the h264/h265 arms differ only in parser element name + a warning; table them:

```rust
let video_parser = if media_type.starts_with("video/x-h264") {
    Some("h264parse")
} else if media_type.starts_with("video/x-h265") {
    tracing::warn!("H.265(HEVC) streams can be linked but are not fully supported yet");
    Some("h265parse")
} else {
    None
};
```

  One shared video-arm body builds `[video_queue, parser, output_tee_video, fakesink]`; the audio arm stays as-is (structurally different).
- [ ] **Step 6:** Full `cargo test` (unit count grows by 1: 31+10); e2e (isolated, timeout, pkill). Grep gate: `grep -rn '"whip-sink-\|"video-queue-\|"audio-queue-\|/whip_sink/' src/ | grep -v branch.rs` must show only the route-table/Location usages via the const/fn (no second definition of the conventions; the `run()` bus-watch's `whip-sink-` prefix check moves to a `Branch::is_branch_element_name(&str)` helper or uses the sink-name fn's prefix).
- [ ] **Step 7: Commit** `refactor(c3b): Branch module owns element names, linking, teardown, and the WHIP route contract`

### Task 3 (3c): GLib main loop on a dedicated thread

**Files:**
- Modify: `src/stream/gst_pipeline.rs` (`run()` only)

**Interfaces:**
- Unchanged trait surface: `run()` still resolves at EOS/fatal-error/quit. It just stops blocking a tokio worker while waiting.

- [ ] **Step 1: Rewrite `run()`:** everything after storing the main loop moves onto a named OS thread; the async fn awaits completion:

```rust
async fn run(&self) -> Result<(), Error> {
    let (bus, main_loop) = {
        let mut pipeline_state = self.0.lock_err().await?;
        let pipeline = pipeline_state
            .pipeline
            .as_ref()
            .ok_or(MyError::FailedOperation(
                "Pipeline called before initialization".to_string(),
            ))?;
        let bus = pipeline.bus().unwrap();
        let main_loop = glib::MainLoop::new(None, false);
        pipeline_state.main_loop = Some(main_loop.clone());
        (bus, main_loop)
    };

    let (done_tx, done_rx) = tokio::sync::oneshot::channel::<Result<(), Error>>();
    // The GLib main loop is synchronous; parking it on a tokio worker
    // starves the runtime (the documented e2e hang on current_thread).
    // It gets its own OS thread; run() awaits its completion signal.
    std::thread::Builder::new()
        .name("gst-main-loop".to_string())
        .spawn(move || {
            match bus.add_watch(/* the existing watch closure, unchanged */) {
                Ok(_bus_watch) => {
                    main_loop.run(); // _bus_watch lives until here
                    let _ = done_tx.send(Ok(()));
                }
                Err(e) => {
                    let _ = done_tx.send(Err(e.into()));
                }
            }
        })?;

    done_rx
        .await
        .map_err(|_| MyError::FailedOperation("GLib main loop thread died".to_string()))??;
    Ok(())
}
```

  The watch closure body is copied verbatim (EOS → quit; whip-branch errors contained; other errors → quit). The `is_branch_element_name` helper from 3b keeps the containment check tied to the naming convention.
- [ ] **Step 2:** Full `cargo test`; e2e (isolated, timeout, pkill) — this stage is exactly what the known current_thread hang was about; the e2e must still pass and exit cleanly on multi_thread.
- [ ] **Step 3: Commit** `refactor(c3c): run the GLib main loop on a dedicated thread`

## Self-Review

- Stage independence: each task compiles + passes suite alone; e2e gate after each. ✓
- 3d explicitly not planned (review: revisit spec first). ✓
- The pad-probe/`call_async_future` teardown sequencing is moved verbatim in 3b, never redesigned. ✓
- One-template check in 3b Step 6 enforces the "Done when: one definition of branch names + whip route". ✓
