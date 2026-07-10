# Split WHEP vs WHIP DELETE handlers

**Date:** 2026-07-10
**Status:** Approved — ready for implementation planning
**Scope:** `src/routes/remove.rs`, `src/startup.rs`
**Depends on:** the idempotent-DELETE change (branch `idempotent-delete`, PR #111). This branch is based on `idempotent-delete`, not `main`, because it refactors the exact handler that change introduced.

## Motivation

Both DELETE routes currently dispatch to one shared handler, `remove_connection`:

- `DELETE /channel/{id}` — a WHEP viewer terminating its playback session (client-facing).
- `DELETE /whip_sink/{id}` — the internal loopback `whipclientsink` tearing down its leg.

They are indistinguishable in traces and logs (one `REMOVE` span) and the single
name says nothing about which caller it serves. This change splits them for two
reasons, both chosen explicitly:

- **Observability** — distinct tracing spans and log lines so a client
  terminating a session is separable from the internal sink teardown.
- **Clarity / maintainability** — two intent-revealing names that each say what
  the DELETE means.

This is **not** a behavior change. The HTTP contract is identical to what PR #111
established and must stay identical.

## Behavior contract — unchanged

Both routes keep the exact idempotent-DELETE contract from PR #111:

| Situation                                | Status                |
|------------------------------------------|-----------------------|
| Connection existed, torn down            | `200 OK`              |
| Already gone / never existed             | `204 No Content`      |
| Transient teardown failure               | `503 + Retry-After`   |
| Coordinator gone / fatal pipeline error  | `500`                 |

No status, path, coordinator, or error-mapping change. The idempotency match
stays in exactly one place so its safety invariant (a transient failure must not
collapse into a success code) cannot drift between the two routes.

## Design

Replace the single public `remove_connection` in `src/routes/remove.rs` with one
shared private helper plus two intent-named, separately-instrumented public
handlers.

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

`&signal` deref-coerces `web::Data<SignalHandle>` to `&SignalHandle` at the call
site (`web::Data<T>: Deref<Target = T>`); the implementer may use
`signal.get_ref()` if clearer.

Routing in `src/startup.rs` re-points the two DELETE routes:

```rust
.route("/channel/{id}", web::delete().to(terminate_session))   // was remove_connection
.route(WHIP_SINK_ROUTE,  web::delete().to(remove_whip_sink))    // was remove_connection
```

### File organization

All three (`delete` helper + both handlers) stay in `src/routes/remove.rs`, a
cohesive "deletion" module — least churn, and the shared helper's callers stay
together. `mod.rs` still `pub use remove::*`, which now re-exports
`terminate_session` and `remove_whip_sink` (the `delete` helper stays private).

Considered and rejected: co-locating `terminate_session` into `whep_handler.rs`
and `remove_whip_sink` into `whip_handler.rs`. That would help the future
whepserversink migration (ADR 0001) delete the WHIP path as one file, but that
migration was not a motivation here, and the split would separate the shared
helper from its callers.

## Test plan

**No new tests.** This is a behavior-preserving refactor; the existing DELETE
integration tests in `tests/signaling.rs` are exactly the regression guard and
must stay green **unchanged**:

- `delete_is_idempotent` — WHEP route: 200 / 204 / 204.
- `delete_with_transient_teardown_failure_returns_503` — the 503 safety invariant.
- `list_and_delete_manage_the_connection_lifecycle` — WHEP successful delete → 200.
- `whip_resource_location_is_routable_and_delete_removes_the_connection` — WHIP
  route successful delete → 200.

These already exercise both routes through HTTP, so a green run after the split
proves the contract is preserved. The observability change (span/log names) is
verified by inspection, not by brittle log-assertion tests.

The only code-level checks worth confirming during implementation:

- `remove_connection` (the old route handler name) has no remaining references
  after `startup.rs` is repointed. The `SignalHandle::remove_connection` method
  is a different symbol and stays.
- `cargo clippy` is clean (no unused imports, no dead code).

## Out of scope

- Any behavior/status change (that was PR #111).
- Co-locating handlers by protocol / the whepserversink WHIP-path removal.
- Per-route policy differences — the two handlers deliberately share `delete`.
