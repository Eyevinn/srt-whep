# Idempotent DELETE Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `DELETE /channel/{id}` and `DELETE /whip_sink/{id}` idempotent — a delete of an already-gone resource returns `204 No Content` instead of `404`, while a real termination keeps returning `200 OK`.

**Architecture:** The idempotency policy lives at the HTTP edge (`src/routes/remove.rs`). The coordinator is unchanged: it still returns `Ok(())` when it tore a branch down and `Err(SignalError::NotFound)` when the id was unknown. The route handler maps `Ok(())` → `200`, `NotFound` → `204`, and propagates every other error unchanged (`503` retryable / `500` fatal).

**Tech Stack:** Rust, actix-web, tokio. Fake `TestPipeline` drives the HTTP integration tests (no GStreamer needed for these tests).

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-07-10-idempotent-delete-design.md`. Behavior contract, verbatim:
  - Connection existed, torn down → `200 OK` (unchanged).
  - Already gone / never existed → `204 No Content` (was `404`).
  - Transient teardown failure → `503 + Retry-After` (unchanged — MUST NOT collapse to success).
  - Coordinator gone / fatal pipeline error → `500` (unchanged).
- **Do NOT change** `src/signal/coordinator.rs` (its `remove_connection` still returns `NotFound` for a missing id) or `src/signal/errors.rs` (the `NotFound → 404` mapping stays; PATCH relies on it).
- **Test environment (macOS):** the integration tests link GStreamer dylibs; export this block before any `cargo` command or the binary aborts with `dyld: Library not loaded`:
  ```sh
  export PKG_CONFIG_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib/pkgconfig
  export PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/bin:$PATH
  export GST_PLUGIN_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
  export DYLD_LIBRARY_PATH=$DYLD_LIBRARY_PATH:$GST_PLUGIN_PATH
  export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib
  ```
- **Working directory:** the isolated worktree at `.claude/worktrees/idempotent-delete` (branch `idempotent-delete`). All paths below are relative to it.
- **Commit trailer:** end every commit message with:
  ```
  Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01WGVHhYpRBESwq7VLfTMLTn
  ```

---

### Task 1: Idempotent DELETE at the HTTP edge

**Files:**
- Modify: `src/routes/remove.rs` (the whole `remove_connection` handler body)
- Test: `tests/signaling.rs` (add `delete_is_idempotent`; trim the DELETE arm from `unknown_ids_return_404`)

**Interfaces:**
- Consumes: `SignalHandle::remove_connection(id: String) -> Result<(), SignalError>` (unchanged) which returns `Ok(())` on teardown, `Err(SignalError::NotFound(id))` for an unknown id, `Err(SignalError::PipelineBusy(_))` on a transient teardown failure, and `Err(SignalError::Unavailable)`/`Pipeline(_)` on fatal errors.
- Test helpers already in `tests/signaling.rs`: `spawn_app(functional_config()) -> (String, TestPipeline)`, `complete_exchange(&address, &pipeline, index).await -> String` (returns the connection id after a full WHEP↔WHIP exchange), `http_client() -> reqwest::Client`. `reqwest::StatusCode` is already imported at the top of the file. `pipeline.snapshot().removed` is a `Vec<String>` of torn-down ids.
- Produces: no new public API — only the HTTP status contract changes.

- [ ] **Step 1: Write the failing integration test**

Add this test to `tests/signaling.rs` (place it directly after `unknown_ids_return_404`, so the idempotent-delete behavior sits next to the 404 cases it replaces):

```rust
#[tokio::test]
async fn delete_is_idempotent() {
    let (address, pipeline) = spawn_app(functional_config());
    let client = http_client();

    let id = complete_exchange(&address, &pipeline, 0).await;

    // First DELETE terminates a live session: 200, branch torn down.
    let first = client
        .delete(format!("{}/channel/{}", address, id))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::OK, first.status());
    assert!(pipeline.snapshot().removed.contains(&id));

    // Repeat DELETE of the same id: already gone -> 204 no-op, not 404.
    let repeat = client
        .delete(format!("{}/channel/{}", address, id))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::NO_CONTENT, repeat.status());

    // DELETE of an id that never existed -> 204.
    let ghost = client
        .delete(format!("{}/channel/never-existed", address))
        .send()
        .await
        .unwrap();
    assert_eq!(StatusCode::NO_CONTENT, ghost.status());
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run (with the GStreamer env from Global Constraints exported):
```sh
cargo test --test signaling delete_is_idempotent -- --nocapture
```
Expected: FAIL. The first `DELETE` returns `200` and the branch is removed, but the repeat `DELETE` returns `404`, so the test panics at `assert_eq!(StatusCode::NO_CONTENT, repeat.status())` (`left: 204, right: 404`).

- [ ] **Step 3: Implement the idempotent handler**

Replace the entire body of `src/routes/remove.rs` with:

```rust
use crate::signal::{SignalError, SignalHandle};
use actix_web::{web, HttpResponse};

/// DELETE is idempotent. Tearing down a live connection returns `200 OK`
/// (the WHEP/WHIP-spec termination confirmation); a connection that is
/// already gone — a client retry, or a session the coordinator already
/// reaped — is a no-op that returns `204 No Content` instead of `404`.
/// Retryable teardown failures (`503`) and fatal errors (`500`) are
/// propagated unchanged.
#[tracing::instrument(name = "REMOVE", skip(signal))]
pub async fn remove_connection(
    path: web::Path<String>,
    signal: web::Data<SignalHandle>,
) -> Result<HttpResponse, SignalError> {
    let id = path.into_inner();
    match signal.remove_connection(id).await {
        Ok(()) => Ok(HttpResponse::Ok().finish()),
        Err(SignalError::NotFound(_)) => Ok(HttpResponse::NoContent().finish()),
        Err(e) => Err(e),
    }
}
```

- [ ] **Step 4: Run the new test to verify it passes**

Run:
```sh
cargo test --test signaling delete_is_idempotent -- --nocapture
```
Expected: PASS (`test delete_is_idempotent ... ok`).

- [ ] **Step 5: Trim the now-stale DELETE arm from `unknown_ids_return_404`**

In `tests/signaling.rs`, `unknown_ids_return_404` still asserts `404` for a DELETE of a ghost id — now handled by `delete_is_idempotent`. Delete that arm (the `client.delete(format!("{}/channel/ghost", address))` block and its `assert_eq!(404, response.status());`). Keep the POST `/whip_sink/ghost` → `404` and PATCH `/channel/ghost` → `404` arms. Add a one-line comment where the DELETE arm was:

```rust
    // DELETE of an unknown id is idempotent (204), covered by delete_is_idempotent.
```

- [ ] **Step 6: Run the full suite to verify everything is green**

Run:
```sh
cargo test
```
Expected: PASS. All lib + `signaling` integration tests pass (`tests/e2e_gstreamer.rs` stays `#[ignore]`d and does not run). Confirm `unknown_ids_return_404`, `list_and_delete_manage_the_connection_lifecycle`, and `whip_resource_location_is_routable_and_delete_removes_the_connection` are all green — the latter two still assert `200` on a successful DELETE and MUST remain unchanged.

- [ ] **Step 7: Commit**

```sh
git add src/routes/remove.rs tests/signaling.rs
git commit -m "$(cat <<'EOF'
feat(routes): idempotent DELETE — 204 for already-gone resource

DELETE /channel/{id} and /whip_sink/{id} now return 204 No Content when
the connection is already gone or never existed, instead of 404. A real
termination still returns 200 OK. Policy lives at the HTTP edge; the
coordinator and error mapping are unchanged. Retryable (503) and fatal
(500) paths are untouched.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
Claude-Session: https://claude.ai/code/session_01WGVHhYpRBESwq7VLfTMLTn
EOF
)"
```

---

## Notes for the reviewer

- The coordinator's `remove_connection` and its unit tests (`unknown_id_is_not_found_for_every_command`, `failed_delete_keeps_the_connection_retryable`) are deliberately untouched and must stay green. If either changed, the policy leaked out of the HTTP edge — reject.
- Splitting the shared handler into separate WHEP and WHIP routes is explicitly out of scope (follow-up change). Both routes gain idempotency uniformly here.
