# Idempotent DELETE

**Date:** 2026-07-10
**Status:** Approved — ready for implementation planning
**Scope:** WHEP/WHIP resource teardown (`DELETE /channel/{id}` and `DELETE /whip_sink/{id}`)

## Motivation

A client (or a flaky network) may issue a DELETE for a session that is already
gone — either because it retried, or because the coordinator itself already
reaped the connection (bus-watch runtime failure, handshake-timeout sweep).
Today that second DELETE returns `404 Not Found`, which reads as an error even
though the client's intent — "make sure this session is gone" — is already
satisfied.

We want DELETE to be idempotent in the sense that matters (RFC 9110): repeating
it leaves server state unchanged and returns a success code, so a retry or a
race with internal cleanup is never surfaced as a client error.

### Spec grounding

This is a **robustness/interop improvement, not a compliance fix**. Both specs
were checked:

- **WHIP (RFC 9725)** and the **WHEP draft (draft-ietf-wish-whep, §4.4)** each
  say only that the server responds with `200 OK` to confirm session
  termination.
- Neither spec addresses repeated DELETEs or an already-gone resource. Under
  standard HTTP semantics (RFC 9110) a `404` for a non-existent resource is
  legal, so today's behavior is already spec-compliant.

Because the specs are silent on the already-gone case, returning a success code
there is a free choice, not a divergence.

## Behavior contract

Both `DELETE /channel/{id}` (WHEP viewer) and `DELETE /whip_sink/{id}` (internal
loopback WHIP bridge) currently share one handler; this design keeps that shared
handler. (Splitting WHEP vs WHIP is a separate, follow-up change.)

| Situation                                   | Before               | After                          |
|---------------------------------------------|----------------------|--------------------------------|
| Connection existed, torn down               | `200 OK`             | `200 OK` (unchanged)           |
| Already gone / never existed                | `404 Not Found`      | **`204 No Content`**           |
| Teardown failed transiently                 | `503 + Retry-After`  | `503 + Retry-After` (unchanged)|
| Coordinator gone / fatal pipeline error     | `500`                | `500` (unchanged)              |

- `200` is reserved for a real termination — we actually tore down a live
  session. This is exactly what the WHIP/WHEP specs name for that case.
- `204` signals an idempotent no-op — there was nothing to terminate. A client
  can distinguish a first delete (`200`) from a redundant one (`204`); this is
  useful signal, not a violation of idempotency.
- The retryable (`503`) and fatal (`500`) paths are deliberately untouched. In
  particular, a transient teardown failure must NOT be collapsed into a success
  code — the caller should retry (see
  `failed_delete_keeps_the_connection_retryable`).

## Design: decide at the HTTP edge (Approach A)

The idempotency policy is an HTTP-contract decision, so it lives at the HTTP
boundary. The coordinator stays honest: `remove_connection` continues to return
`Ok(())` when it tore a branch down and `Err(NotFound)` when the id was not in
its map. The route handler translates that domain result into the HTTP contract
above.

### The one code change

`src/routes/remove.rs` — replace the current `?`-and-`200` body:

```rust
match signal.remove_connection(id).await {
    Ok(())                        => Ok(HttpResponse::Ok().finish()),         // 200: terminated a live session
    Err(SignalError::NotFound(_)) => Ok(HttpResponse::NoContent().finish()),  // 204: idempotent no-op
    Err(e)                        => Err(e),                                  // 503 (retryable) / 500 stay as-is
}
```

The coordinator already hands the route exactly the distinction it needs
(`Ok(())` vs `Err(NotFound)`), so no plumbing is added.

### Explicitly unchanged

- **`src/signal/coordinator.rs`** — `remove_connection` semantics are untouched;
  a missing id still yields `NotFound`. This is the payoff of deciding at the
  edge: the domain layer keeps telling the truth.
- **`src/signal/errors.rs`** — the `NotFound → 404` mapping stays, because
  **PATCH** (`answer_received`) still relies on it to return `404` for an
  unknown id. Only the DELETE route reinterprets `NotFound`.
- The retryable/fatal error paths.

### Why not decide in the coordinator

Making the coordinator return `Ok(())` for a missing id would bake an
HTTP-shaped decision into the domain layer, force a split of the
`unknown_id_is_not_found_for_every_command` unit test, and make both DELETE
routes share one hard-coded policy — working against the planned WHEP/WHIP
handler split, where each edge should pick its own policy. Deciding at the edge
avoids all three.

## Test plan

**Integration (`tests/signaling.rs`):**

- `unknown_ids_return_404` — remove the DELETE-of-ghost arm (currently asserts
  `404`). Keep the POST-ghost and PATCH-ghost arms asserting `404`. Rename if
  the remaining arms no longer justify the name.
- New `delete_is_idempotent` — establish a connection, then assert:
  1. DELETE the live id → `200`, and the branch is removed from the pipeline.
  2. DELETE the same id again → `204` (already gone).
  3. DELETE a never-existed ghost id → `204`.
- `list_and_delete_manage_the_connection_lifecycle` (successful DELETE) and
  `whip_resource_location_is_routable_and_delete_removes_the_connection`
  (successful DELETE) — **unchanged**; both still assert `200`.

**Unit (`src/signal/coordinator.rs`):** no changes.

- `unknown_id_is_not_found_for_every_command` — stays green; the coordinator
  still returns `NotFound` for a ghost remove.
- `failed_delete_keeps_the_connection_retryable` — stays green; transient
  teardown failure still yields `PipelineBusy`, not a success code.

## Out of scope

- Splitting the shared handler into distinct WHEP and WHIP routes (follow-up
  change #2). This design intentionally keeps the shared handler so both routes
  gain idempotency uniformly for now.
- Any change to the `200` success code, the `503`/`500` error paths, or the
  coordinator's teardown/retry logic.
