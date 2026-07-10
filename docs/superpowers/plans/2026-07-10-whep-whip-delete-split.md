# Split WHEP vs WHIP DELETE Handlers Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the shared `remove_connection` DELETE handler into two intent-named, separately-instrumented handlers (`terminate_session` for WHEP, `remove_whip_sink` for WHIP), delegating to one shared idempotency helper — with the HTTP behavior unchanged.

**Architecture:** Behavior-preserving refactor at the HTTP edge. `src/routes/remove.rs` gains a private `delete` helper holding the PR #111 idempotency match; two thin public handlers wrap it, each with its own `#[tracing::instrument]` span and an intent log line. `src/startup.rs` repoints the two DELETE routes. Nothing else changes.

**Tech Stack:** Rust, actix-web, tokio, tracing. Fake `TestPipeline` drives the HTTP integration tests.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-10-whep-whip-delete-split-design.md`.
- **Behavior is unchanged** — the HTTP contract stays exactly as PR #111 left it: `200` (torn down) / `204` (already gone) / `503 + Retry-After` (transient) / `500` (fatal). This is purely an observability + naming refactor.
- **DRY:** the idempotency match lives in exactly ONE place (the private `delete` helper). Do NOT inline/duplicate it in the two handlers.
- **Do NOT change** `src/signal/coordinator.rs`, `src/signal/errors.rs`, or any existing test. The existing DELETE integration tests are the regression guard and must stay green **unchanged**.
- **No new tests** — there is no new behavior to test; a green existing suite is the proof the refactor preserved behavior.
- **Test environment (macOS) — REQUIRED before any `cargo` command**, or the test binary aborts with `dyld: Library not loaded`:
  ```sh
  export PKG_CONFIG_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib/pkgconfig
  export PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/bin:$PATH
  export GST_PLUGIN_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
  export DYLD_LIBRARY_PATH=$DYLD_LIBRARY_PATH:$GST_PLUGIN_PATH
  export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
  ```
- **Working directory:** the worktree `.claude/worktrees/whep-whip-delete-split` (branch `whep-whip-delete-split`, based on `idempotent-delete`). All paths below are relative to it.
- **Commit trailer** — end the commit message with:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01WGVHhYpRBESwq7VLfTMLTn
  ```

---

### Task 1: Split the DELETE handler

**Files:**
- Modify: `src/routes/remove.rs` (replace the whole file body)
- Modify: `src/startup.rs:25` and `:27` (repoint the two DELETE routes)

**Interfaces:**
- Consumes: `SignalHandle::remove_connection(id: String) -> Result<(), SignalError>` (unchanged coordinator method — NOT the route handler being replaced).
- Produces: two public handlers re-exported via `crate::routes::*` — `terminate_session(path: web::Path<String>, signal: web::Data<SignalHandle>) -> Result<HttpResponse, SignalError>` and `remove_whip_sink(...)` with the same signature. The private `delete` helper is not exported. The old public `remove_connection` route handler is removed.

- [ ] **Step 1: Rewrite `src/routes/remove.rs`**

Replace the entire contents of `src/routes/remove.rs` with:

```rust
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};

/// The shared idempotent-DELETE mapping (single source of the PR #111 policy):
/// `Ok(())` → 200 (terminated a live session), `NotFound` → 204 (already gone,
/// a no-op), and every other error propagates unchanged (503 retryable / 500
/// fatal). Both DELETE routes go through here so the contract cannot diverge.
async fn delete(id: String, signal: &SignalHandle) -> Result<HttpResponse, SignalError> {
    match signal.remove_connection(id).await {
        Ok(()) => Ok(HttpResponse::Ok().finish()),
        Err(SignalError::NotFound(_)) => Ok(HttpResponse::NoContent().finish()),
        Err(e) => Err(e),
    }
}

/// A WHEP viewer terminating its playback session (`DELETE /channel/{id}`).
#[tracing::instrument(name = "WHEP DELETE", skip(signal))]
pub async fn terminate_session(
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    tracing::info!("WHEP client terminating session {}", id);
    delete(id, &signal).await
}

/// The internal loopback whipclientsink tearing down its leg
/// (`DELETE /whip_sink/{id}`).
#[tracing::instrument(name = "WHIP SINK DELETE", skip(signal))]
pub async fn remove_whip_sink(
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    tracing::info!("Removing WHIP sink for connection {}", id);
    delete(id, &signal).await
}
```

Note: `delete(id, &signal)` relies on `web::Data<SignalHandle>` deref-coercing to `&SignalHandle`. If the compiler objects, use `delete(id, signal.get_ref()).await`.

- [ ] **Step 2: Repoint the two DELETE routes in `src/startup.rs`**

Change the two DELETE route registrations (currently both `.to(remove_connection)`):

```rust
            .route("/channel/{id}", web::delete().to(terminate_session))
```
and
```rust
            .route(WHIP_SINK_ROUTE, web::delete().to(remove_whip_sink))
```

Leave every other route (`/list`, POST `/channel`, OPTIONS, PATCH, POST `WHIP_SINK_ROUTE`) untouched. No import change is needed — `terminate_session` and `remove_whip_sink` arrive via the existing `use crate::routes::*;` glob.

- [ ] **Step 3: Confirm no dangling reference to the old route handler**

Run:
```sh
rg -n "\bremove_connection\b" src/
```
Expected: every remaining hit is the coordinator method — `signal.remove_connection(...)` (in `remove.rs` and `coordinator.rs`), the method definition in `src/signal/mod.rs`, `Command::RemoveConnection` dispatch, and coordinator tests. There must be **no** `.to(remove_connection)` in `startup.rs` and **no** free `pub async fn remove_connection` in `routes/`.

- [ ] **Step 4: Build and lint (with the GStreamer env exported)**

Run:
```sh
cargo build 2>&1 | tail -20
cargo clippy --all-targets 2>&1 | rg -E "warning|error" || echo "clippy clean"
```
Expected: builds with no errors; clippy reports no warnings (no unused imports, no dead code). If clippy flags anything introduced by this change, fix it.

- [ ] **Step 5: Run the full suite — existing behavior must be preserved**

Run:
```sh
cargo test > /tmp/whep-whip-split-test.log 2>&1; echo "EXIT=$?"
rg -E "test result|FAILED|^error|panicked|delete_is_idempotent|delete_with_transient|list_and_delete|whip_resource_location" /tmp/whep-whip-split-test.log
```
Expected: `EXIT=0`. All lib tests and all `signaling` integration tests pass, unchanged (52 lib + 14 signaling as of the base commit; `e2e_gstreamer` stays `#[ignore]`d). In particular `delete_is_idempotent`, `delete_with_transient_teardown_failure_returns_503`, `list_and_delete_manage_the_connection_lifecycle`, and `whip_resource_location_is_routable_and_delete_removes_the_connection` must all be `ok` — they exercise both split routes through HTTP and prove the contract is preserved.

- [ ] **Step 6: Commit**

```sh
git add src/routes/remove.rs src/startup.rs
git commit -m "$(cat <<'EOF'
refactor(routes): split DELETE into terminate_session + remove_whip_sink

The shared remove_connection handler becomes two intent-named handlers with
distinct tracing spans (WHEP DELETE / WHIP SINK DELETE), both delegating to a
single private `delete` helper that holds the idempotency mapping. HTTP
behavior is unchanged; the existing DELETE integration tests are the guard.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WGVHhYpRBESwq7VLfTMLTn
EOF
)"
```

---

## Notes for the reviewer

- This branch is stacked on `idempotent-delete`. Review only THIS task's diff (base = the spec commit `790d992`) and, for the whole-branch review, base against the `idempotent-delete` fork point (`04f4e12`) — NOT `merge-base main HEAD`, which would drag in all of PR #111's commits.
- The change must be behavior-preserving: if any existing DELETE test needed editing to stay green, that is a red flag — the refactor changed behavior and should be rejected.
- Verify the idempotency match appears exactly once (in `delete`), not duplicated across the two handlers.
