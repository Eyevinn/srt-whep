# Refactor Proposal: Signal-Plane Legibility & Config Hygiene

**Status:** Implemented (landed on main, commits 1bcb05c…e44005c)
**Date:** 2026-07-08
**Scope agreed with:** Kun Wu

## Problem Statement

The codebase has come out of the signaling rebuild (ADR 0001), the hardening
pass (ADR 0002), and the architecture-deepening pass in good structural shape,
but a residue of implicit structure remains that makes the system harder to
read and change than it needs to be — and the developer wants the codebase
positioned so that the eventual `whepserversink` migration (recorded as future
work in ADR 0001) becomes a small, safe step rather than a risky rewrite.

Concretely, from the developer's perspective:

1. **The connection state machine is implicit.** The coordinator's
   three-state connection lifecycle (awaiting offer → awaiting answer →
   established) has no single place where legal transitions are defined.
   Transitions are inlined in each command handler, and the "fail whichever
   waiter is parked on this connection" logic is written out three times,
   nearly verbatim, in the remove, reap, and reset paths. A reader cannot
   answer "what are the legal transitions?" without reading every handler.

2. **The real and fake pipelines disagree on the readiness contract.**
   Creating a connection checks pipeline readiness once in the coordinator and
   then a second time inside the real branch-add operation — two lock
   acquisitions with a time-of-check/time-of-use window between them. The test
   fake does *not* perform the internal re-check, so the fake and the real
   adapter honor different contracts, and unit tests are structurally unable
   to notice.

3. **The loopback WHIP port coupling is unasserted and the port types are
   inconsistent.** The CLI argument port is a 32-bit integer while the
   application's port is 16-bit. More importantly, the pipeline posts its WHIP
   offers back to the port from the CLI arguments, while the HTTP server binds
   whatever listener it is handed — nothing checks that these agree. A caller
   who wires them differently gets a silent 404 on every offer instead of a
   loud failure at startup.

4. **Small error-layer leaks.** The shared error-chain formatting helper
   lives in the domain module but is consumed by the stream layer's error
   types — a domain utility reaching across a layer boundary. The
   "invalid SDP" condition also exists as two separate variants in two error
   enums, reconciled by an awkward unwrap-style conversion that exists only to
   avoid a doubled message prefix.

5. **Operator knobs are unreachable.** The coordinator's six timing/watchdog
   configuration values can only ever take their defaults because there is no
   CLI surface for them. ADR 0001/0002 recorded this as deferred, explicitly
   "fair game later."

6. **The loopback bridge is not delimited.** The surfaces that exist *only*
   because egress uses a WHIP client element (the loopback route, the
   endpoint-URL builder, the callback-port coupling) are correct but not
   marked as one thing. When `whepserversink` lands, someone has to
   rediscover the full extent of what can be deleted.

## Solution

Four themes, all behavior-preserving except where noted:

1. **Make the state machine explicit** by moving each legal transition into a
   method on the connection-state type, so the entire legal-transition table
   lives in one impl block and the triplicated waiter-failure logic collapses
   into a single method. Handlers become thin: parse command → call
   transition → store result. Illegal transitions return a typed
   wrong-state error that maps to the same HTTP statuses as today.

2. **Fix the readiness contract at its seam** by making readiness enforcement
   internal and atomic to the branch-add operation: one lock acquisition,
   check and attach under the same guard, not-ready failures surfaced as the
   existing retryable error (still mapped to 503 + Retry-After). The test
   fake gains the identical gate, the coordinator drops its separate
   pre-check, and both implementors of the seam then honor one contract. The
   standalone readiness query stays on the trait because the e2e test polls
   it.

3. **Make the port coupling honest**: unify on 16-bit port types end to end,
   and have the application assembly point accept the pipeline's expected
   WHIP callback port as an explicit optional input, failing fast at startup
   if it disagrees with the port the listener actually bound. Test
   assemblies using the fake pipeline (which has no callback port) pass
   nothing and skip the check.

4. **Hygiene and positioning**: relocate the error-formatting helper to a
   crate-level home; make the signaling invalid-SDP error wrap the domain SDP
   error as a proper source; expose the coordinator config knobs as CLI flags
   with today's defaults; introduce the long-parked SDP offer/answer
   direction newtypes so the two SDP directions cannot be swapped at compile
   time; and finish with a documentation/consolidation pass that delimits
   every loopback-bridge-specific surface behind one clearly-marked boundary
   so the future `whepserversink` migration is a clean deletion.

## Commits

Each commit leaves the tree compiling, all non-ignored tests green, and is
independently revertable. Phases are ordered lowest-risk first; within a
phase, ordering is mandatory where noted.

### Phase 1 — Error hygiene (independent, smallest)

**Commit 1: Relocate the error-chain formatting helper to a crate-level
utility.** Move the shared "walk the source chain and format it" helper out
of the domain error module into a small crate-level errors utility, and update
the two consumers (domain and stream error Debug implementations) to import it
from there. Pure move; no behavior change; existing error-formatting tests
keep passing untouched.

**Commit 2: Make the signaling invalid-SDP error wrap the domain SDP error as
a source.** Replace the string-duplicating variant in the signaling error
enum with one that carries the domain SDP error as its source, deleting the
unwrap-style conversion that existed to dodge the doubled message prefix.
Preserve the HTTP 400 mapping and the externally visible response body
exactly; adjust the error-mapping unit tests to assert on the new structure
while keeping the same user-facing message assertions.

### Phase 2 — Explicit state machine (well-tested ground, mandatory internal order)

**Commit 3: Extract the waiter-failure logic into one method on the
connection state.** Add a consuming method on the connection-state type that
fails whichever reply waiter is parked (offer waiter, answer waiter, or no-op
for established) with a supplied error, and replace the three duplicated match
blocks in the remove, reap, and reset paths with calls to it. Add direct unit
tests for the method across all three states. All ~20 existing actor tests
pass unchanged.

**Commit 4: Introduce a typed wrong-state error and the offer-received
transition method.** Add a small typed error for illegal transitions
(converting into the existing signaling error and thus the same HTTP status
as today), plus a consuming transition method that takes the
awaiting-offer state to awaiting-answer. The coordinator's offer handler
delegates to it. Existing wrong-state actor tests pass unchanged.

**Commit 5: Add the answer-received transition method.** Same shape:
awaiting-answer to established, coordinator's answer handler delegates.
Existing tests pass unchanged.

**Commit 6: Add the creation constructor and a transition-table test.** Give
the state type a named constructor for its initial state, move any remaining
inline state writes in handlers into the impl block, and add one unit test
that enumerates every (state, event) pair and asserts exactly the legal set
succeeds. After this commit the impl block *is* the state machine
documentation.

### Phase 3 — SDP direction newtypes (parked item C6)

**Commit 7: Introduce offer and answer newtypes in the domain.** Two thin
wrappers over the existing validated session-description type, each with its
own parse constructor reusing the existing validation. Added alongside the
existing type, not yet consumed anywhere; domain unit tests cover
construction. Tree green because nothing else changed.

**Commit 8: Thread the direction newtypes through the signaling plane.**
Change the coordinator message payloads, the signal-handle method signatures,
the connection-state fields, and the HTTP handlers so that offers and answers
are distinct types end to end. Purely mechanical; the compiler drives the
edit. Integration tests pass unchanged since the wire format is untouched.

### Phase 4 — One readiness contract (mandatory internal order)

**Commit 9: Make the real branch-add atomic.** Restructure the real
pipeline's branch-add so the readiness condition is evaluated under the same
lock guard that performs the attach — one lock acquisition instead of two,
no check-to-use window. Externally identical behavior (not-ready still
surfaces as the retryable error). This is the only commit in the plan that
touches real-GStreamer code; the change is lock/control-flow only, no element
logic. Verified by one manual run of the ignored e2e test.

**Commit 10: Give the test fake the same gate.** The fake's branch-add now
returns the same not-ready retryable error when the fake is not ready,
mirroring the real adapter. Add a unit test that drives a create against a
not-ready fake and asserts the retryable classification — proving fake and
real honor one contract. Must land before Commit 11.

**Commit 11: Remove the coordinator's readiness pre-check.** The
create-connection handler stops calling the standalone readiness query and
relies on branch-add's internal gate. The existing "not ready yields 503 +
Retry-After" actor and integration tests must pass unchanged (the error now
originates one layer lower but classifies and maps identically). The
standalone readiness query remains on the trait for the e2e test's startup
polling.

### Phase 5 — Honest port coupling

**Commit 12: Unify port types on 16 bits.** Change the CLI-argument port,
the WHIP endpoint-URL builder, and the branch-attach parameter from 32-bit to
16-bit. Compiler-driven; no behavior change (all real ports already fit).

**Commit 13: Assert port agreement at assembly.** The application assembly
point gains an optional expected-callback-port input. When provided, it is
compared against the port the listener actually bound, and a mismatch fails
assembly with a clear message naming both ports. The production entry point
passes its configured port; the signaling integration tests (fake pipeline,
no callback) pass nothing; the e2e test passes its port and drops its
hand-alignment comment. Add one test asserting that a deliberate mismatch
fails fast.

### Phase 6 — Extras

**Commit 14: Expose the coordinator config as CLI flags.** Add flags for the
six coordinator knobs (handshake timeouts, sweep/teardown timing, watchdog
window and threshold) with defaults equal to today's hard-coded values, and
build the config from parsed arguments in the entry point. Add a test that
parsing with no flags reproduces the current defaults, so the "no flags means
no behavior change" property is pinned.

**Commit 15: Delimit the loopback bridge (prep for whepserversink).**
Consolidate and document every surface that exists only because egress is a
WHIP client — the loopback route template and its route registrations, the
endpoint-URL builder, the callback-port input from Commit 13 — under one
clearly-marked boundary with a module-level explanation stating that all of
it is deleted by the whepserversink migration. Add a pointer in ADR 0001's
future-work section to this marked boundary. No behavior change; mostly
documentation and possibly small code moves within the stream module.

## Decision Document

- **State machine shape:** transitions become consuming methods on the
  connection-state type (offer-received, answer-received, fail-waiter, plus a
  named initial-state constructor). No separate event enum or
  transition-table function — that was considered and rejected as too large a
  diff against a well-tested module. Illegal transitions return a typed
  wrong-state error that converts into the existing signaling error, keeping
  every HTTP status exactly as it is today.
- **Readiness contract:** enforcement lives inside branch-add, atomically
  under the operation's own lock. The coordinator no longer pre-checks. Both
  implementors of the branch-control seam honor the identical contract:
  branch-add against a not-ready pipeline yields the retryable not-ready
  error, mapping to 503 with Retry-After. The standalone readiness query is
  retained on the trait solely because the e2e test uses it for startup
  polling; it is no longer on any production code path for connection
  creation.
- **Port coupling:** ports are 16-bit everywhere. The assembly point takes
  the expected WHIP callback port as an *optional* input rather than adding a
  port accessor to the pipeline traits — deliberately, so the trait seams stay
  free of loopback-specific concerns and the whole mechanism deletes cleanly
  with the whepserversink migration. Fake-pipeline assemblies skip the check
  by passing nothing.
- **Error layering:** the layered error architecture (domain / stream /
  signal vocabularies with From-conversions) is intentional per the C4
  decision and stays. Only two things change: the chain-formatting helper
  moves to a crate-level home, and the signaling invalid-SDP variant wraps
  the domain error as a source instead of duplicating its message.
- **SDP direction newtypes:** the parked C6 item is included. Two wrappers
  over the validated session-description type, threaded through message
  payloads, handle signatures, state fields, and HTTP handlers. Wire format
  unchanged.
- **Coordinator config CLI:** all six knobs exposed, defaults identical to
  the current hard-coded values, so running with no flags is bit-for-bit
  today's behavior. This closes the deferral recorded in ADR 0001/0002.
- **Closed decisions honored:** the loopback WHIP bridge stays (its removal
  requires a new ADR per ADR 0001); the single coordinator actor continues to
  own connection state and serialize branch operations through its mailbox
  (accepted trade-off, ADR 0001/0002); runtime branch reaps still do not feed
  the watchdog; the rswebrtc plugin continues to come from the GStreamer
  installation only (ADR 0003/0004); the channel/connection/branch naming
  convention is preserved and no fourth name is introduced.

## Testing Decisions

- **What a good test is here:** assert external behavior at public seams —
  HTTP responses through the assembled application, coordinator behavior
  through the signal handle, seam contracts through the trait — never
  private state or call sequences. The existing suite already models this
  well; new tests follow the same style.
- **Prior art to imitate:** the coordinator actor tests (paused tokio clock +
  test fake, ~20 cases), the signaling integration tests (real actix server
  assembled through the production wiring with the fake pipeline), and the
  error-mapping unit tests asserting status codes and Retry-After headers.
- **New tests introduced by this plan:** direct unit tests on the
  waiter-failure method and the transition table (Phase 2); a contract test
  proving the fake's not-ready gate matches the real adapter's classification
  (Phase 4); a fail-fast test for the port-mismatch assertion (Phase 5); a
  defaults-equivalence test for the CLI flags (Phase 6).
- **GStreamer coverage:** the single real-GStreamer commit (Commit 9) is
  control-flow-only and is verified by one manual run of the ignored e2e test
  on macOS (using the documented framework environment); a second manual e2e
  run happens after Phase 5 since the e2e wiring changes. Everything else in
  the plan is covered by the CI-run unit and integration suites.

## Out of Scope

- **The whepserversink migration itself** (ADR 0001 future work). This plan
  only *positions* for it (Commit 15); the migration needs its own ADR and
  its own plan.
- **Decomposing the real pipeline's init topology** (the 275-line builder,
  the duplicated codec arms, the hard-coded element names). Deliberately
  excluded: it has zero CI coverage and the agreed testing posture ("extract
  pure logic first") deserves its own plan.
- **The lifecycle typestate** (parked item C3d: encoding init-before-run in
  types). Recorded as "revisit spec first"; not touched.
- **Age-based reaping of established connections.** The dormant
  established-since field stays as is (ADR 0002: reaping is event-driven).
- **Supervisor semantics, watchdog thresholds, backoff behavior, the list
  API's response shape, and every closed ADR decision** listed above.
- **Any transcoding, codec, or media-path behavior.**

## Further Notes

- **Mandatory orderings:** Commit 10 (fake gains the gate) must precede
  Commit 11 (coordinator drops the pre-check), otherwise the not-ready tests
  would pass vacuously in between. Phases 1, 2, 3 are mutually independent
  and could be reordered or landed as separate PRs; Phases 4 and 5 should
  land in the listed order because the e2e port-handling change in Commit 13
  assumes the readiness contract from Phase 4.
- **Estimated shape:** ~15 commits, all small; the largest single diff is the
  mechanical newtype threading in Commit 8.
- After this plan lands, the remaining known structural debt is exactly: the
  real-pipeline init decomposition (with its testing question) and the
  whepserversink ADR — a deliberately short list.
