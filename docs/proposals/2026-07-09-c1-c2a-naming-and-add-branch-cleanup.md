# Design: C1 (stream naming module) + C2a (add_branch self-cleanup)

**Status:** Designed, not started
**Date:** 2026-07-09
**Base commit:** c93f9b4 (line numbers below refer to this commit — re-verify before editing)
**Source candidates:** `docs/proposals/2026-07-09-architecture-deepening-candidates.md`, C1 + C2a
**Scope agreed with:** Kun Wu

This design covers two candidates from the architecture-deepening handover. They
are bundled deliberately: both work the stream plane's naming/attach seam, and
both need the same single manual GStreamer e2e run, so doing them together
amortizes that run. Everything here is behavior-preserving except one
deliberately inverted test (called out in C2a).

## Required reading / pinned constraints honored

- **CONTEXT.md** — channel ↔ connection ↔ branch terminology (no fourth name is
  introduced), module map, closed constraints.
- **ADR 0001–0004** — closed decisions. In particular: `src/stream` must never
  import `src/signal` (acyclic module graph); the loopback WHIP bridge stays;
  per-connection failure isolation + watchdog semantics are pinned by tests;
  `rswebrtc` comes from the GStreamer installation, never compiled in.
- **ADR 0002** — the half-attached-branch cleanup *semantics* (a partial attach
  is detached before the error reply) are pinned. C2a keeps those semantics and
  moves only the *location* of the detach.
- **2026-07-08 signal-plane proposal** — decided and implemented. Its Out of
  Scope names "the real-pipeline init decomposition … the hard-coded element
  names" as remaining debt; C1 is the *naming* slice of that only and does not
  decompose `init`'s topology.

## Problem Statement

### C1 — element naming is split across two files

Whether a pipeline bus error reaps one viewer's branch or kills the whole
pipeline hangs on a naming convention that is maintained by hand across two
files:

- **Name construction** lives in `src/stream/gst_pipeline.rs` as bare string
  literals: `"demux"` (`input_ready` at 57, the tsdemux element at ~222),
  `"video-queue"`/`"audio-queue"` (created at ~225–226, looked up in
  `insert_sink` at ~409/424), `"output_tee_video"`/`"output_tee_audio"`
  (created at ~297/323, looked up in `input_ready` at ~70–71).
- **Name classification** lives in `src/stream/branch.rs`: the `*_PREFIX`
  consts (40–44) and `branch_id_from_name` (56–61), which decides whether a bus
  error's source element belongs to one viewer's branch.

The load-bearing invariant is that a branch element is named exactly
`<core-name>-<id>` — the core queues are named exactly `video-queue`/
`audio-queue` (no id suffix), and **the trailing dash in the `video-queue-`/
`audio-queue-` prefixes is the only thing keeping a core-queue error fatal**
(see the comment at `branch.rs:46–55`). Because construction and classification
are independent literals, renaming `"video-queue"` in `gst_pipeline.rs` would
silently reclassify a fatal core-queue error as a per-viewer reap — dropping the
SRT ingest handling to a single bad peer with no compile-time or test warning.

### C2a — the coordinator guesses at stream-plane cleanup

`create_connection` (`src/signal/coordinator.rs:266–282`) matches
`PipelineError::Fatal` to guess whether `add_branch` left a half-attached
branch, then calls `remove_branch_bounded` to detach it:

```rust
if let Err(add_err) = self.pipeline.add_branch(id.clone()).await {
    if matches!(add_err, crate::stream::PipelineError::Fatal(_)) {
        // ... remove_branch_bounded(id) ...
    }
    let _ = reply.send(Err(add_err.into()));
    return;
}
```

This is a stream-plane implementation fact ("only `Fatal` can mean attach ran
partway") living in the signal plane. `stream/errors.rs` documents the error
taxonomy as pure retry policy; this caller has overloaded `Fatal` to also mean
"maybe half-attached". Concrete wrong case: `input_ready`'s missing-demux error
is `Fatal` and occurs *before* any attach (`gst_pipeline.rs:58`), so today's
code issues a spurious (harmless but wasteful — a bounded teardown on the
actor's critical path) detach of a branch that was never added.

## Solution

### C1 — one naming module, branch names derived from core stems

Add `src/stream/naming.rs`: pure string logic, no GStreamer types. It owns core
element-name constants, per-branch name constructors, and the classifier. The
design choice that matters is that **branch names derive from the core-name
stems**, so the trailing-dash relationship is structural rather than a
coincidence maintained across files:

```rust
//! Single source of truth for GStreamer element names in the stream plane, and
//! the classifier that decides whether a bus error belongs to one viewer's
//! branch (reap just that branch) or the core chain (fatal -> restart).
//!
//! Branch element names are DERIVED from the core stems, so the load-bearing
//! relationship -- a branch queue is exactly `<core-queue-name>-<id>`, and the
//! trailing dash is the only thing keeping a core-queue error fatal -- holds by
//! construction, not by hand across two files.

// Core (viewer-independent) elements. These names are shared across files.
pub(crate) const DEMUX: &str = "demux";
pub(crate) const VIDEO_QUEUE: &str = "video-queue";
pub(crate) const AUDIO_QUEUE: &str = "audio-queue";
pub(crate) const OUTPUT_TEE_VIDEO: &str = "output_tee_video";
pub(crate) const OUTPUT_TEE_AUDIO: &str = "output_tee_audio";

// Branch-only stems (no core element shares these names).
const WHIP_SINK_STEM: &str = "whip-sink";
const VIDEO_DECODER_STEM: &str = "avdec-h264"; // present only under --decode-video

pub(crate) fn video_queue_name(id: &str) -> String { format!("{VIDEO_QUEUE}-{id}") }
pub(crate) fn audio_queue_name(id: &str) -> String { format!("{AUDIO_QUEUE}-{id}") }
pub(crate) fn whip_sink_name(id: &str)   -> String { format!("{WHIP_SINK_STEM}-{id}") }
pub(crate) fn video_decoder_name(id: &str) -> String { format!("{VIDEO_DECODER_STEM}-{id}") }

/// If `name` is a per-viewer branch element, return the connection id it
/// belongs to. A branch element is exactly `<stem>-<id>`: strip the stem, then
/// REQUIRE the '-'. A core queue named exactly `video-queue` strips to "" and
/// the missing '-' makes it return None -- that is what keeps core errors fatal.
pub(crate) fn branch_id_from_name(name: &str) -> Option<&str> {
    for stem in [WHIP_SINK_STEM, VIDEO_QUEUE, AUDIO_QUEUE, VIDEO_DECODER_STEM] {
        if let Some(id) = name.strip_prefix(stem).and_then(|rest| rest.strip_prefix('-')) {
            return Some(id);
        }
    }
    None
}
```

The `.strip_prefix('-')` step enforces the trailing-dash invariant for every
branch element type at once, and the video/audio branch prefixes are now
literally the core consts + `-`, so the two can never drift.

Consumers:

- **`branch.rs`**: delete the four `*_PREFIX` consts and `branch_id_from_name`;
  `Branch`'s name methods delegate to `naming::*`. `whip_sink_path` and
  `whip_endpoint` stay in `branch.rs` — they are loopback-bridge URL surface,
  not element naming, and are the marked deletion boundary for the
  `whepserversink` migration.
- **`gst_pipeline.rs`**: replace the bare core-name literals (element creation,
  `by_name` lookups, and `MissingElement`/error-message strings) with
  `naming::*`.

**Scope guard:** only the naming slice. Do not decompose `init`'s topology, the
duplicated codec arms, or anything else in the 2026-07-08 Out of Scope.

### C2a — `add_branch` cleans up its own half-attached branch

Move the cleanup behind the seam. `add_branch` attaches under the state lock
(attach is synchronous and may hold the lock — `branch.rs:100–101`); on a real
attach failure it drops the guard and detaches its own half-built branch before
returning the error. `detach` awaits GStreamer state changes and must run with
the state lock released (`branch.rs:197`), so `add_branch` mirrors the
lock-then-detach shape `remove_branch` already uses (`gst_pipeline.rs:137–153`):

```rust
async fn add_branch(&self, id: String) -> Result<(), PipelineError> {
    // Attach under the state lock (attach is synchronous). Clone the pipeline
    // handle so that, if attach fails, we can detach the half-built branch
    // AFTER releasing the lock -- detach awaits GStreamer state changes and
    // must not run under the timed state lock.
    let (pipeline, attach_result) = {
        let pipeline_state = self.state.lock_err().await?;
        let pipeline = pipeline_state.pipeline.as_ref().ok_or(PipelineError::NotReady)?;
        if !Self::input_ready(pipeline)? {
            tracing::error!("Demux has no pad available. No connection can be added.");
            return Err(PipelineError::NotReady); // pre-attach; nothing to clean up
        }
        tracing::debug!("Add connection {} to pipeline", id);
        let result = Branch::for_id(&id).attach(
            pipeline,
            pipeline_state.args.port,
            pipeline_state.args.decode_video,
        );
        (pipeline.clone(), result)
    };

    if let Err(attach_err) = attach_result {
        // Attach ran partway: detach our own half-built branch so the caller
        // never reasons about cleanup. detach tolerates a half-built branch
        // (missing elements are skipped). Best-effort; the original attach
        // error is what we report.
        tracing::warn!("attach for {} failed ({}); detaching half-built branch", id, attach_err);
        if let Err(cleanup_err) = Branch::for_id(&id).detach(&pipeline).await {
            tracing::error!("cleanup after failed attach for {} also failed: {}", id, cleanup_err);
        }
        return Err(PipelineError::Fatal(attach_err.to_string()));
    }
    Ok(())
}
```

`input_ready`'s missing-demux `Fatal` propagates via `?` *before* the attach, so
the old spurious detach on that path disappears for free.

Coordinator: delete the `matches!(add_err, PipelineError::Fatal(_))` block.
`create_connection` becomes "on error, map and reply":

```rust
if let Err(add_err) = self.pipeline.add_branch(id.clone()).await {
    let _ = reply.send(Err(add_err.into()));
    return;
}
```

This removes the last non-test `PipelineError::` match under `src/signal/`;
error variants go back to meaning retry policy only.

## Decision Document

- **C1 classifier shape:** derive branch names from the core stems and enforce
  the separator with `strip_prefix('-')`, rather than collecting the existing
  literals side by side as independent consts. The derived form makes the
  core-vs-branch invariant hold by construction; the side-by-side form would
  leave the trailing-dash relationship as two strings a future edit could
  desync. Chosen the derived form.
- **C1 which core names to consolidate:** at minimum the names referenced in
  more than one place (`demux`, `video-queue`, `audio-queue`,
  `output_tee_video`, `output_tee_audio`). The single-use core names
  (`srt_source`, `input_tee`, `typefind`, `whep-queue`, `srt-queue`, the
  srtsink) may be added for a single source of truth but are not required by
  the "no literal in more than one place" goal; the implementation plan decides
  per-name. No GStreamer types enter `naming`.
- **C1 module home:** `src/stream/naming.rs`, `pub(crate)`. `stream` does not
  import `signal`, so the acyclic module graph (ADR 0001) is preserved — the
  module is pure string logic with no cross-plane dependency.
- **C2a cleanup location (ADR 0002):** semantics unchanged — a half-attached
  branch is still detached before the error reply. Only the location moves
  (coordinator → inside `add_branch`). Recorded in the commit message; ADR
  0002's row updated only if the wording warrants.
- **C2a optional dedup — deferred.** The handover offered folding the
  `has_video`/`has_audio` derivation (computed in `input_ready`, re-derived in
  `attach`) into one pass. Deliberately out of scope here to keep the diff
  single-purpose: the double derivation is cheap and unrelated to the leak. It
  can be a later candidate if it proves worth it.
- **C2a error precedence:** on a failed attach the *original* attach error is
  returned; a detach that also fails is logged, not surfaced. The detach is
  best-effort cleanup, not part of the caller-visible result.
- **Contract symmetry (2026-07-08 Phase 4):** the fake and real adapters must
  behave identically at the seam. `TestPipeline::add_branch` already leaves
  `added` empty on failure ("nothing attached"), so it honors the new contract
  with no change; the actor test that observed the coordinator's cleanup is
  rewritten (below).

## Testing Decisions

Testing posture (2026-07-08): assert external behavior at public seams — never
private state or call sequences. `naming` is pure logic and gets direct unit
tests; the seam behavior is asserted through the trait and through
`SignalHandle`/the actor.

- **C1 classifier tests** move into `naming` (from `branch.rs:345–369`). The
  load-bearing test asserts against the consts directly:
  `branch_id_from_name(naming::VIDEO_QUEUE).is_none()` and
  `..(naming::AUDIO_QUEUE)..`, plus `demux`/`srt_source` → `None`, and every
  `*_name(id)` round-trips back to `Some(id)`. This test breaks the instant the
  derive relationship is broken — it is the point of C1.
- **C1 constructor tests:** the name-construction assertions from
  `branch.rs`'s `names_and_paths_derive_from_one_convention` move to `naming`;
  the `whip_sink_path`/`whip_endpoint` assertions stay in `branch.rs`.
- **C2a — one deliberately inverted test.** `coordinator.rs:992`
  `add_branch_failure_detaches_the_half_attached_branch` currently asserts
  `snapshot().removed == ["a"]` (the coordinator issued the cleanup). It is
  renamed and inverted: create still fails and registers no connection, but the
  coordinator now issues no cleanup, so `snapshot().removed.is_empty()`. This is
  the single deliberate behavior change in the test suite and is the assertion
  that proves the spurious-detach path is gone.
- **C2a — fake-seam contract test.** After `fail_next_add_branch(Fatal)` and a
  failed `add_branch`, assert `snapshot().added.is_empty()` — pinning "a failed
  add leaves nothing attached" at the seam, matching the existing
  `add_branch_on_a_not_ready_fake_is_not_ready` for the `NotReady` case.
- **The real self-clean cannot be unit-tested** (no real GStreamer in unit
  tests). It is covered by code review plus the manual e2e run below; stated
  honestly rather than faked.

## Verification

- `cargo test --all-targets` green (unit + actor + `tests/signaling.rs`
  integration), with the GStreamer environment exported (framework-only
  `GST_PLUGIN_PATH`, per ADR 0003 — a second higher-versioned `rswebrtc` on the
  path shadows the working one).
- **One** manual, isolated run of the `#[ignore]`d `tests/e2e_gstreamer.rs` on
  macOS. Both candidates touch the real attach/detach path and the core element
  names, so this run is the gate proving real GStreamer still links branches and
  reaps them correctly.

## Delivery

- One feature branch (`refactor/stream-naming-and-add-branch-cleanup`),
  sequenced small commits, each leaving the tree compiling and non-ignored
  tests green. Lands as one PR.
- Suggested sequence (the implementation plan finalizes it): C1 first (the new
  `naming` module + `branch.rs` migration + tests, then point `gst_pipeline.rs`
  at the consts), then C2a (self-cleaning `add_branch` + coordinator deletion +
  the inverted/added tests). The one manual e2e run happens once after both
  land.

## Done when

- **C1:** no element-name string literal appears in more than one place; the
  core-vs-branch invariant is pinned by a unit test asserting against the
  consts; behavior unchanged (`cargo test --all-targets` green; one manual e2e
  run).
- **C2a:** `src/signal` contains no `matches!` on stream error variants (grep
  for `PipelineError::` under non-test `src/signal/` shows only the `From`
  conversion in `signal/errors.rs`); a failed attach detaches its own branch
  inside `add_branch`; actor + integration tests green; the manual e2e run
  confirms the real attach/detach path.

## Out of scope

- The `has_video`/`has_audio` derivation dedup (deferred, see Decision
  Document).
- Decomposing `gst_pipeline.rs`'s `init` topology beyond C1's naming slice.
- The `whepserversink` migration and everything else in the 2026-07-08
  proposal's and ADR 0001/0002's Out of Scope.
- The other candidates (C2b, C3–C7) in the handover.
