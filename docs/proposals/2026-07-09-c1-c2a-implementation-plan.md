# C1 (stream naming module) + C2a (add_branch self-cleanup) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Concentrate all GStreamer element naming into one `src/stream/naming.rs` (branch names derived from core stems so the core-vs-branch invariant holds by construction), and move the half-attached-branch cleanup inside `add_branch` so the signal plane stops matching on stream error variants.

**Architecture:** Two candidates (C1, C2a) from `docs/proposals/2026-07-09-architecture-deepening-candidates.md`, designed in `docs/proposals/2026-07-09-c1-c2a-naming-and-add-branch-cleanup.md`. Both work the stream plane's naming/attach seam and share one manual GStreamer e2e run, so they land on one branch. All behavior-preserving except one deliberately inverted coordinator test.

**Tech Stack:** Rust, GStreamer (via `gstreamer` crate), tokio, actix-web. Tests: `cargo test` (unit + actor + `tests/signaling.rs`), plus the `#[ignore]`d `tests/e2e_gstreamer.rs` for real GStreamer.

## Global Constraints

- **Acyclic module graph (ADR 0001):** `src/stream` must never import `src/signal`. `naming` is pure string logic — no `signal` imports, no GStreamer types.
- **ADR 0002 semantics preserved:** a half-attached branch is detached before the error reply. C2a moves only the *location* of that detach (coordinator → inside `add_branch`), never the semantic.
- **No fourth naming term:** keep channel ↔ connection ↔ branch (CONTEXT.md). Do not rename these concepts.
- **Naming slice only:** C1 touches element *names*; do NOT decompose `gst_pipeline.rs`'s `init` topology or the codec arms (2026-07-08 proposal, Out of Scope).
- **Every commit is green AND warning-free on its staged tree:** pre-commit runs `cargo fmt -- --check`, `cargo check --all`, and `cargo clippy --all-targets --all-features --tests --benches -- -D warnings`, and it **stashes unstaged files first**. Therefore each task must `git add` ALL of its files together — a partially-staged task whose staged files reference unstaged edits will fail `cargo check`/clippy. No dead code at any commit (unused `pub(crate)` items warn → clippy fails).
- **macOS test environment (ADR 0003 / memory):** export before ANY `cargo build`/`test`/`run`:
  ```sh
  export PKG_CONFIG_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib/pkgconfig
  export PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/bin:$PATH
  export GST_PLUGIN_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
  export DYLD_LIBRARY_PATH=$DYLD_LIBRARY_PATH:$GST_PLUGIN_PATH
  export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
  ```
  Keep `GST_PLUGIN_PATH` at the framework lib ONLY — never prepend a local `gst-plugins-rs` build (it shadows the framework's stable `rswebrtc`).

---

### Task 1: Create the `stream::naming` module and migrate `branch.rs`

**Files:**
- Create: `src/stream/naming.rs`
- Modify: `src/stream/mod.rs:1-5` (declare `mod naming;`)
- Modify: `src/stream/branch.rs:40-61` (delete `*_PREFIX` consts + `branch_id_from_name`), `:19-23` (imports), `:75-89` (delegate name methods), `:108-172` (attach core-name lookups), `:327-370` (split tests)
- Modify: `src/stream/gst_pipeline.rs:10` (import), `:499` (classifier call site)

**Interfaces:**
- Produces (all `pub(crate)` in `src/stream/naming.rs`):
  - `const DEMUX: &str`, `const VIDEO_QUEUE: &str`, `const AUDIO_QUEUE: &str`, `const OUTPUT_TEE_VIDEO: &str`, `const OUTPUT_TEE_AUDIO: &str`, `const SRT_SOURCE: &str`
  - `fn video_queue_name(id: &str) -> String`, `fn audio_queue_name(id: &str) -> String`, `fn whip_sink_name(id: &str) -> String`, `fn video_decoder_name(id: &str) -> String`
  - `fn branch_id_from_name(name: &str) -> Option<&str>`
- Consumes: nothing from other tasks.

- [ ] **Step 1: Create `src/stream/naming.rs` with the module and its tests**

```rust
//! Single source of truth for GStreamer element names in the stream plane, and
//! the classifier that decides whether a bus error belongs to one viewer's
//! branch (reap just that branch) or the core demux->tee chain (fatal ->
//! supervisor restart).
//!
//! Branch element names are DERIVED from the core stems, so the load-bearing
//! relationship -- a branch queue is exactly `<core-queue-name>-<id>`, and the
//! trailing dash is the only thing keeping a core-queue error fatal -- holds by
//! construction here, not by hand across `gst_pipeline.rs` and `branch.rs`.
//!
//! Pure string logic: no GStreamer types, no `src/signal` dependency (the
//! acyclic module graph from ADR 0001 stays intact).

// Core (viewer-independent) elements. These names are referenced from more than
// one place, so they live here once.
pub(crate) const DEMUX: &str = "demux";
pub(crate) const VIDEO_QUEUE: &str = "video-queue";
pub(crate) const AUDIO_QUEUE: &str = "audio-queue";
pub(crate) const OUTPUT_TEE_VIDEO: &str = "output_tee_video";
pub(crate) const OUTPUT_TEE_AUDIO: &str = "output_tee_audio";
pub(crate) const SRT_SOURCE: &str = "srt_source";

// Branch-only stems (no core element shares these names).
const WHIP_SINK_STEM: &str = "whip-sink";
const VIDEO_DECODER_STEM: &str = "avdec-h264"; // present only under --decode-video

pub(crate) fn video_queue_name(id: &str) -> String {
    format!("{VIDEO_QUEUE}-{id}")
}

pub(crate) fn audio_queue_name(id: &str) -> String {
    format!("{AUDIO_QUEUE}-{id}")
}

pub(crate) fn whip_sink_name(id: &str) -> String {
    format!("{WHIP_SINK_STEM}-{id}")
}

pub(crate) fn video_decoder_name(id: &str) -> String {
    format!("{VIDEO_DECODER_STEM}-{id}")
}

/// If `name` is a per-viewer branch element, return the connection id it
/// belongs to. Recognizes the whip sink, the per-media queues, and the optional
/// `--decode-video` H264 decoder.
///
/// A branch element is exactly `<stem>-<id>`: strip the stem, then REQUIRE the
/// '-'. A core queue named exactly `video-queue` strips to "" and the missing
/// '-' makes it return `None` -- that is what keeps a core-queue error fatal.
///
/// The bus watch uses this to contain a dying branch's errors to that branch
/// (reaping just that connection) instead of quitting the whole pipeline, which
/// would drop the SRT ingest and every other viewer.
pub(crate) fn branch_id_from_name(name: &str) -> Option<&str> {
    for stem in [WHIP_SINK_STEM, VIDEO_QUEUE, AUDIO_QUEUE, VIDEO_DECODER_STEM] {
        if let Some(id) = name
            .strip_prefix(stem)
            .and_then(|rest| rest.strip_prefix('-'))
        {
            return Some(id);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn branch_names_derive_from_one_convention() {
        assert_eq!("whip-sink-abc", whip_sink_name("abc"));
        assert_eq!("video-queue-abc", video_queue_name("abc"));
        assert_eq!("audio-queue-abc", audio_queue_name("abc"));
        assert_eq!("avdec-h264-abc", video_decoder_name("abc"));
    }

    #[test]
    fn every_branch_element_maps_back_to_its_id() {
        // ALL of a viewer's elements -- the whip sink, its per-media queues, and
        // the optional decoder -- are recognized as branch-owned, so an error
        // from any of them is contained to that branch.
        for name in [
            whip_sink_name("abc"),
            video_queue_name("abc"),
            audio_queue_name("abc"),
            video_decoder_name("abc"),
        ] {
            assert_eq!(Some("abc"), branch_id_from_name(&name), "{name} not contained");
        }
    }

    #[test]
    fn core_element_names_never_classify_as_a_branch() {
        // The load-bearing invariant: a core element error must stay fatal.
        // Asserted against the consts (not literals) so this test breaks the
        // instant the derive-from-stem relationship is broken.
        for name in [DEMUX, VIDEO_QUEUE, AUDIO_QUEUE, OUTPUT_TEE_VIDEO, OUTPUT_TEE_AUDIO, SRT_SOURCE] {
            assert_eq!(None, branch_id_from_name(name), "{name} wrongly contained");
        }
    }
}
```

- [ ] **Step 2: Declare the module in `src/stream/mod.rs`**

Change lines 1-5 from:

```rust
mod branch;
mod errors;
mod gst_pipeline;
mod pipeline;
mod utils;
```

to (insert `mod naming;` in alphabetical position):

```rust
mod branch;
mod errors;
mod gst_pipeline;
mod naming;
mod pipeline;
mod utils;
```

- [ ] **Step 3: Run the naming tests to verify they pass**

Run (env exported per Global Constraints):
```sh
cargo test --lib stream::naming
```
Expected: PASS (3 tests). If `mod naming;` is missing you get "file not found for module"; if a const is missing you get a compile error.

- [ ] **Step 4: Migrate `branch.rs` to consume `naming`**

In `src/stream/branch.rs`, add the import near the other `use` lines (after line 23 `use crate::stream::errors::StreamError;`):

```rust
use crate::stream::naming;
```

Delete lines 40-44 (the four `*_PREFIX` consts) AND lines 46-61 (the `branch_id_from_name` doc comment + function) entirely — they now live in `naming`.

Replace the four `Branch` name methods (currently lines 75-89) with thin delegators:

```rust
    fn whip_sink_name(&self) -> String {
        naming::whip_sink_name(&self.id)
    }

    fn video_queue_name(&self) -> String {
        naming::video_queue_name(&self.id)
    }

    fn audio_queue_name(&self) -> String {
        naming::audio_queue_name(&self.id)
    }

    fn video_decoder_name(&self) -> String {
        naming::video_decoder_name(&self.id)
    }
```

In `attach`, replace the three core-element lookups with `naming` consts. Line ~108-110:

```rust
        let demux = pipeline
            .by_name(naming::DEMUX)
            .ok_or(StreamError::MissingElement(naming::DEMUX.to_string()))?;
```

Line ~134-136:

```rust
            let output_tee_video = pipeline
                .by_name(naming::OUTPUT_TEE_VIDEO)
                .ok_or(StreamError::MissingElement(naming::OUTPUT_TEE_VIDEO.to_string()))?;
```

Line ~170-172:

```rust
            let output_tee_audio = pipeline
                .by_name(naming::OUTPUT_TEE_AUDIO)
                .ok_or(StreamError::MissingElement(naming::OUTPUT_TEE_AUDIO.to_string()))?;
```

- [ ] **Step 5: Split `branch.rs`'s tests — keep only the loopback-path test**

The name-construction and classifier assertions moved to `naming`. Replace the entire `#[cfg(test)] mod tests { ... }` block (currently lines 327-370) with just the loopback-path test (whip_sink_path/whip_endpoint stay in `branch.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_paths_derive_from_the_route_template() {
        assert_eq!("/whip_sink/abc", whip_sink_path("abc"));
        assert_eq!(
            "http://localhost:8000/whip_sink/abc",
            whip_endpoint(8000, "abc")
        );
    }
}
```

- [ ] **Step 6: Point `gst_pipeline.rs` at `naming` for the classifier**

In `src/stream/gst_pipeline.rs`, change the import at line 10 from:

```rust
use crate::stream::branch::{branch_id_from_name, Branch};
```

to:

```rust
use crate::stream::branch::Branch;
use crate::stream::naming;
```

Then update the classifier call site in `run`'s bus watch (line ~499) from:

```rust
                        if let Some(id) = branch_id_from_name(obj.name().as_str()) {
```

to:

```rust
                        if let Some(id) = naming::branch_id_from_name(obj.name().as_str()) {
```

(The bare core-name literals in `gst_pipeline.rs` are migrated in Task 2. Leaving them here now is fine — the tree still compiles and the naming consts are all consumed by `branch.rs`, so there is no dead code.)

- [ ] **Step 7: Run the full unit + actor + integration suite**

Run:
```sh
cargo test --all-targets
```
Expected: PASS (all non-ignored tests, including `stream::naming::tests` and the actor/integration suites). No warnings.

- [ ] **Step 8: Commit**

```sh
git add src/stream/naming.rs src/stream/mod.rs src/stream/branch.rs src/stream/gst_pipeline.rs
git commit -m "refactor(stream): concentrate element naming into stream::naming (C1, part 1)

Branch names now derive from the core stems, so branch_id_from_name and the
core element names can never drift: a core queue named exactly video-queue
strips to \"\" and the missing '-' keeps its errors fatal. branch.rs delegates
to naming; the classifier moves out of branch.rs.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01878UYRATgE2bu8HXUsEUAC"
```

---

### Task 2: Migrate `gst_pipeline.rs` element-name literals to `naming`

**Files:**
- Modify: `src/stream/gst_pipeline.rs:55-73` (`input_ready`), `:209-226` (element creation), `:296-323` (output tees), `:405-433` (`insert_sink`)

**Interfaces:**
- Consumes: `naming::{DEMUX, VIDEO_QUEUE, AUDIO_QUEUE, OUTPUT_TEE_VIDEO, OUTPUT_TEE_AUDIO, SRT_SOURCE}` from Task 1 (`use crate::stream::naming;` already added in Task 1 Step 6).
- Produces: nothing new.

- [ ] **Step 1: Replace the core-name lookups in `input_ready`**

In `src/stream/gst_pipeline.rs`, `input_ready` (lines ~55-73). Line ~57-58:

```rust
        let demux = pipeline
            .by_name(naming::DEMUX)
            .ok_or_else(|| PipelineError::Fatal(format!("Failed to find element: {}", naming::DEMUX)))?;
```

Lines ~70-71:

```rust
        let video_ready = !has_video || pipeline.by_name(naming::OUTPUT_TEE_VIDEO).is_some();
        let audio_ready = !has_audio || pipeline.by_name(naming::OUTPUT_TEE_AUDIO).is_some();
```

- [ ] **Step 2: Replace the element-creation names in `init`**

The `srtsrc` name (line ~210):

```rust
        let src = gst::ElementFactory::make("srtsrc")
            .name(naming::SRT_SOURCE)
```

The tsdemux name (line ~220-222):

```rust
        let tsdemux = gst::ElementFactory::make("tsdemux")
            .name(naming::DEMUX)
```

The two core queues (lines ~225-226):

```rust
        let video_queue = Self::create_custom_queue(naming::VIDEO_QUEUE, "0", "0", "no")?;
        let audio_queue = Self::create_custom_queue(naming::AUDIO_QUEUE, "0", "0", "no")?;
```

The two output tees (lines ~296-298 and ~323-325):

```rust
                        let output_tee_video = gst::ElementFactory::make("tee")
                            .name(naming::OUTPUT_TEE_VIDEO)
                            .build()?;
```

```rust
                        let output_tee_audio = gst::ElementFactory::make("tee")
                            .name(naming::OUTPUT_TEE_AUDIO)
                            .build()?;
```

- [ ] **Step 3: Replace the core-queue lookups in `insert_sink`**

In `insert_sink` (the `tsdemux.connect_pad_added` closure). The audio branch (lines ~408-416):

```rust
                    let audio_queue = pipeline
                        .by_name(naming::AUDIO_QUEUE)
                        .ok_or(StreamError::MissingElement(naming::AUDIO_QUEUE.to_string()))?;
                    let sink_pad =
                        audio_queue
                            .static_pad("sink")
                            .ok_or(StreamError::MissingElement(format!(
                                "{}'s sink pad",
                                naming::AUDIO_QUEUE
                            )))?;
```

The video branch (lines ~424-432):

```rust
                    let video_queue = pipeline
                        .by_name(naming::VIDEO_QUEUE)
                        .ok_or(StreamError::MissingElement(naming::VIDEO_QUEUE.to_string()))?;
                    let sink_pad =
                        video_queue
                            .static_pad("sink")
                            .ok_or(StreamError::MissingElement(format!(
                                "{}'s sink pad",
                                naming::VIDEO_QUEUE
                            )))?;
```

- [ ] **Step 4: Verify no shared core-name literals remain**

Run:
```sh
grep -nE '"(demux|video-queue|audio-queue|output_tee_video|output_tee_audio|srt_source)"' src/stream/gst_pipeline.rs src/stream/branch.rs
```
Expected: NO matches (every shared core name now flows through `naming`). The single-use core names left as literals in `gst_pipeline.rs` (`input_tee`, `typefind`, `whep-queue`, `srt-queue`) are intentionally not consolidated — they appear in exactly one place.

- [ ] **Step 5: Run the full suite**

Run:
```sh
cargo test --all-targets
```
Expected: PASS, no warnings. (Behavior is unchanged — element names are byte-for-byte identical, just sourced from consts.)

- [ ] **Step 6: Commit**

```sh
git add src/stream/gst_pipeline.rs
git commit -m "refactor(stream): source core element names from stream::naming (C1, part 2)

No element-name string literal appears in more than one place now; renaming a
core element is a one-line change in naming.rs and the classifier follows.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01878UYRATgE2bu8HXUsEUAC"
```

---

### Task 3: `add_branch` self-cleans; delete the coordinator's cleanup guess (C2a)

**Files:**
- Modify: `src/stream/gst_pipeline.rs:97-121` (`add_branch`)
- Modify: `src/signal/coordinator.rs:266-282` (delete the `matches!(Fatal)` block), `:991-1014` (invert the test)
- Modify: `src/stream/pipeline.rs:260-273` (add a fake-seam contract test)
- Modify: `docs/adr/0002-signaling-plane-hardening.md:20` (clarifying addendum)

**Interfaces:**
- Consumes: `Branch::attach`/`Branch::detach` (unchanged), `PipelineError` (unchanged).
- Produces: `add_branch` now detaches its own half-attached branch on failure; the coordinator no longer issues cleanup after a failed add.

- [ ] **Step 1: Add the fake-seam contract test in `pipeline.rs`**

In `src/stream/pipeline.rs`, inside the existing `#[cfg(test)] mod tests`, add this test right after `add_branch_on_a_not_ready_fake_is_not_ready` (after line 273):

```rust
    #[tokio::test]
    async fn failed_add_branch_leaves_nothing_attached_on_the_fake() {
        let pipeline = TestPipeline::default();
        pipeline.set_ready(true);
        pipeline.fail_next_add_branch(PipelineError::Fatal("attach blew up".into()));

        assert!(matches!(
            pipeline.add_branch("a".to_string()).await,
            Err(PipelineError::Fatal(_))
        ));
        // Same observable contract as the real adapter: a failed add attaches
        // nothing, so no external cleanup is ever needed.
        assert!(pipeline.snapshot().added.is_empty());
    }
```

- [ ] **Step 2: Run it — passes now (characterizes the fake's existing behavior)**

Run:
```sh
cargo test --lib stream::pipeline::tests::failed_add_branch_leaves_nothing_attached_on_the_fake
```
Expected: PASS. This pins the seam contract; the fake already honors it (it returns the error before recording the add).

- [ ] **Step 3: Invert the coordinator test — make it RED**

In `src/signal/coordinator.rs`, replace the test `add_branch_failure_detaches_the_half_attached_branch` (lines ~991-1014) with:

```rust
    #[tokio::test(start_paused = true)]
    async fn failed_add_registers_nothing_and_needs_no_coordinator_cleanup() {
        use crate::stream::PipelineError;

        let pipeline = ready_pipeline();
        pipeline.fail_next_add_branch(PipelineError::Fatal("attach blew up".into()));
        let tx = spawn_actor(pipeline.clone(), test_config());

        let (whep_tx, whep_rx) = oneshot::channel();
        tx.send(Command::CreateConnection {
            id: "a".into(),
            reply: whep_tx,
        })
        .await
        .unwrap();

        // The create fails and the id is never registered...
        assert!(whep_rx.await.unwrap().is_err());
        tokio::task::yield_now().await;

        // ...and the coordinator issues NO cleanup: add_branch owns detaching
        // its own half-attached branch now, so a spurious remove_branch here
        // would be a bug (and today's matches!(Fatal) block causes exactly one).
        assert!(pipeline.snapshot().removed.is_empty());
        assert!(list_ids(&tx).await.is_empty());
    }
```

Run:
```sh
cargo test --lib signal::coordinator::tests::failed_add_registers_nothing_and_needs_no_coordinator_cleanup
```
Expected: FAIL — the assertion `removed.is_empty()` fails because the current coordinator still calls `remove_branch_bounded` after a `Fatal` add (records `removed == ["a"]`).

- [ ] **Step 4: Restructure `add_branch` to self-clean (real adapter)**

In `src/stream/gst_pipeline.rs`, replace the whole `add_branch` method (lines ~90-121, the doc comment may stay) with:

```rust
    async fn add_branch(&self, id: String) -> Result<(), PipelineError> {
        // Attach under the state lock (attach is synchronous and may hold it).
        // Clone the pipeline handle so that, if attach fails, we can detach the
        // half-built branch AFTER releasing the lock -- detach awaits GStreamer
        // state changes and must not run under the 1s timed state lock.
        let (pipeline, attach_result) = {
            let pipeline_state = self.state.lock_err().await?;
            // No pipeline means we are between supervisor restarts: retryable.
            let pipeline = pipeline_state
                .pipeline
                .as_ref()
                .ok_or(PipelineError::NotReady)?;

            if !Self::input_ready(pipeline)? {
                tracing::error!("Demux has no pad available. No connection can be added.");
                return Err(PipelineError::NotReady); // pre-attach: nothing to clean up
            }

            tracing::debug!("Add connection {} to pipeline", id);
            let attach_result = Branch::for_id(&id).attach(
                pipeline,
                pipeline_state.args.port,
                pipeline_state.args.decode_video,
            );
            (pipeline.clone(), attach_result)
        };

        if let Err(attach_err) = attach_result {
            // Attach ran partway: detach our own half-built branch so the caller
            // never has to reason about stream-plane cleanup (ADR 0002 -- the
            // semantic is unchanged; only the location moved here from the
            // coordinator). detach tolerates a half-built branch (missing
            // elements are skipped). Best-effort: the original attach error is
            // what we report.
            tracing::warn!(
                "attach for {} failed ({}); detaching half-built branch",
                id,
                attach_err
            );
            if let Err(cleanup_err) = Branch::for_id(&id).detach(&pipeline).await {
                tracing::error!(
                    "cleanup after failed attach for {} also failed: {}",
                    id,
                    cleanup_err
                );
            }
            return Err(PipelineError::Fatal(attach_err.to_string()));
        }
        Ok(())
    }
```

- [ ] **Step 5: Delete the coordinator's cleanup guess**

In `src/signal/coordinator.rs`, `create_connection` (lines ~266-282). Replace the `if let Err(add_err) = ... { ... }` block so the `matches!(Fatal)` cleanup is gone:

```rust
        if let Err(add_err) = self.pipeline.add_branch(id.clone()).await {
            // add_branch owns detaching a half-attached branch (ADR 0002); the
            // coordinator just maps the error. Error variants mean retry policy
            // only again -- no matching on stream error variants here.
            let _ = reply.send(Err(add_err.into()));
            return;
        }
```

- [ ] **Step 6: Verify the RED test is now GREEN and no signal-plane match remains**

Run:
```sh
cargo test --lib signal::coordinator::tests::failed_add_registers_nothing_and_needs_no_coordinator_cleanup
grep -rnE 'matches!\s*\([^)]*PipelineError|PipelineError::' src/signal/ | grep -v '#\[cfg(test)\]'
```
Expected: the test PASSES. The grep should show NO non-test occurrence of `PipelineError::` under `src/signal/` except (if present) the `From` conversion in `src/signal/errors.rs`. If clippy later flags an unused `use ...PipelineError` at the top of `coordinator.rs`, remove it (the deleted block used the fully-qualified `crate::stream::PipelineError::Fatal`, so there should be none to remove).

- [ ] **Step 7: Add the ADR 0002 clarifying addendum**

In `docs/adr/0002-signaling-plane-hardening.md`, line 20 (the "Half-attached branch on `add_branch` failure" row), append to the cell after "`detach` tolerates a half-built branch.":

```
 As of the C1+C2a refactor (2026-07-09) this detach lives inside `add_branch` itself rather than in the coordinator; the semantic — detached before the error reply — is unchanged.
```

- [ ] **Step 8: Run the full suite**

Run:
```sh
cargo test --all-targets
```
Expected: PASS, no warnings. (The real self-clean path is not exercised by unit tests — it is verified in Task 4.)

- [ ] **Step 9: Commit**

```sh
git add src/stream/gst_pipeline.rs src/signal/coordinator.rs src/stream/pipeline.rs docs/adr/0002-signaling-plane-hardening.md
git commit -m "refactor: add_branch cleans up its own half-attached branch (C2a)

On attach failure, add_branch detaches its own half-built branch before
returning, so the coordinator's matches!(Fatal) -> remove_branch guess is
deleted and stream error variants mean retry policy only again. ADR 0002
semantics unchanged (detach before the error reply); only the location moved.
Also fixes a spurious detach on the pre-attach missing-demux Fatal path.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01878UYRATgE2bu8HXUsEUAC"
```

---

### Task 4: Full verification — suite + one manual GStreamer e2e run

**Files:** none (verification only).

**Interfaces:**
- Consumes: the completed C1 + C2a changes from Tasks 1-3.
- Produces: confidence that real GStreamer still links branches and reaps them, and a clean tree ready for PR.

- [ ] **Step 1: Confirm the whole non-ignored suite is green**

Run (env exported per Global Constraints):
```sh
cargo test --all-targets
```
Expected: PASS, zero warnings.

- [ ] **Step 2: Build the e2e test binary**

Run:
```sh
pkill -9 -f e2e_gstreamer- 2>/dev/null; true
cargo test --test e2e_gstreamer --no-run
```
Expected: compiles; prints the path to the `e2e_gstreamer-<hash>` binary.

- [ ] **Step 3: Run the e2e test ONCE, in isolation**

Run (single-threaded, isolated — it drives a real WebRTC whipclientsink + hardware encoder; back-to-back runs can starve those resources):
```sh
cargo test --test e2e_gstreamer -- --ignored --nocapture --test-threads=1
pkill -9 -f e2e_gstreamer- 2>/dev/null; true
```
Expected: `test result: ok. 1 passed` and a clean exit (no orphaned process holding SRT 9911 / HTTP 8199). This proves the C1 rename kept branch classification correct and the C2a self-clean detaches a real half-attached branch. If it fails on `503 "Timed out waiting for the SDP offer"`, that is the known environmental flake — `pkill -9 -f e2e_gstreamer-` and run once more in isolation.

- [ ] **Step 4: Confirm the working tree is clean**

Run:
```sh
git status --short
```
Expected: no un-committed changes to `src/` or `docs/adr/` (the pre-existing untracked/modified proposal docs from before this branch may still show — leave them).

- [ ] **Step 5: Push the branch and open the PR (only when the user asks)**

Do NOT push or open a PR without explicit user confirmation. When asked:
```sh
git push -u origin refactor/stream-naming-and-add-branch-cleanup
gh pr create --fill --base main
```
PR body should note: bundles C1 + C2a from the architecture-deepening handover; behavior-preserving except the one inverted coordinator test; verified by `cargo test --all-targets` + one manual e2e run on macOS.

---

## Self-Review

**1. Spec coverage** (against `docs/proposals/2026-07-09-c1-c2a-naming-and-add-branch-cleanup.md`):
- C1 naming module (consts + constructors + classifier, derive-from-stem) → Task 1 Step 1. ✅
- branch.rs migration (delete consts/classifier, delegate, attach lookups, split tests) → Task 1 Steps 4-5. ✅
- gst_pipeline.rs literal migration → Task 2. ✅
- C1 done-when (no shared literal in >1 place; invariant pinned against consts) → Task 2 Step 4 grep, Task 1 Step 1 `core_element_names_never_classify_as_a_branch`. ✅
- C2a add_branch self-clean + coordinator deletion → Task 3 Steps 4-5. ✅
- C2a inverted coordinator test + fake-seam contract test → Task 3 Steps 1-3. ✅
- C2a done-when (no `matches!` on stream variants in signal) → Task 3 Step 6 grep. ✅
- ADR 0002 addendum → Task 3 Step 7. ✅
- Verification: full suite + one manual e2e run → Task 4. ✅
- Deferred dedup (has_video/has_audio) → not in any task, matching the spec's Out of Scope. ✅

**2. Placeholder scan:** every code step shows complete code; every run step shows the exact command and expected result. No TBD/TODO. ✅

**3. Type consistency:** `naming::branch_id_from_name` / `video_queue_name` / `audio_queue_name` / `whip_sink_name` / `video_decoder_name` and the consts `DEMUX`/`VIDEO_QUEUE`/`AUDIO_QUEUE`/`OUTPUT_TEE_VIDEO`/`OUTPUT_TEE_AUDIO`/`SRT_SOURCE` are named identically in the Interfaces block (Task 1) and every consumer (Tasks 1-2). `add_branch`'s signature (`async fn add_branch(&self, id: String) -> Result<(), PipelineError>`) is unchanged. `TestPipeline::fail_next_add_branch`, `snapshot().added`/`.removed`, `ready_pipeline()`, `spawn_actor`, `test_config()`, `list_ids` all match existing helpers used elsewhere in their test modules. ✅
