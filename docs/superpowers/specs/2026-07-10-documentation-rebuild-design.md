# Documentation Rebuild — Design

*Rebuild srt-whep's documentation as an intentional kernel-plus-layers set:
a project-facing `README.md` and an agent-facing `CONTEXT.md` as the kernel,
everything else either linked from the kernel, kept untouched as history, or
deleted. New narrative content (the connection-lifecycle walkthrough and the
"parked waiter" concept) is added on top.*

Date: 2026-07-10 · Status: approved (pending spec review)

---

## Goal

The docs have accreted: an excellent deep-dive (`architecture-evolution…`) sits
orphaned with zero inbound links, old diagrams and SDP fixtures linger unused,
and there is no newcomer-facing "how does a viewer actually connect today"
walkthrough. Rebuild around a small, curated **kernel** so every living document
is reachable and intentional, and fold in the genuinely-new teaching content
produced in the `/teach` lessons.

### Audience split (decided)

- **`README.md` = the project, for humans** — a user/operator can install and
  run without leaving the page; a contributor is one hop from the depth.
  (Comprehensive scope: option A.)
- **`CONTEXT.md` = the map, for agents** — glossary, terminology, module map,
  decided constraints, dev env.

## Non-goals / out of scope

- The interactive HTML lessons stay in the git-excluded `/teach` workspace; they
  are **not** moved into the repo. This effort ports only their durable prose.
- No changes to `docs/proposals/*`, `docs/superpowers/*`, or `docs/plan.md` —
  kept **as they are** (user decision). They are self-contained SDD history and
  are exempt from the no-orphan rule below.
- No code changes. Documentation only. (The `age_secs` hands-on from lesson 0004
  is a teaching exercise, not shipped.)
- No `CHANGELOG.md` entry required for the doc reshuffle.

---

## Architecture: kernel + layers

```
README.md  ──────────────┐   (project, humans)
                         ├── links to ──▶ CONTEXT.md, docs/adr/, architecture-evolution.md,
CONTEXT.md ──────────────┘                connection-lifecycle.md, guides
   (map, agents)
```

**The no-orphan invariant:** every retained user-facing document must be
reachable by a link from `README.md` or `CONTEXT.md`. This is the rule that
prevents re-accretion. (Exempt: the intentionally-untouched history in
`proposals/`, `superpowers/`, `plan.md`.)

## Disposition of every existing item

| Item | Action |
|---|---|
| `README.md`, `CONTEXT.md` | **Rewrite** (kernel) |
| `docs/adr/0001–0005` | Keep as-is; link from kernel |
| `docs/architecture-evolution-shared-lock-to-actor.md` | Keep; **fix orphan** (link from README + CONTEXT) |
| `docs/connection-lifecycle.md` | **New** — handshake walkthrough + parked waiters |
| `docs/supported_codecs.md`, `known_limitations.md`, `whep.md`, `useful_commands.md`, `LiveStreaming.md` | Keep; make discoverable from README |
| `docs/SRT_macOS.md` | **Delete after merging** its env-var content into the README OSX build section |
| `docs/srt-whep-coordinator-actor.gif` / `.excalidraw`, `docs/diagram/*`, `docs/screenshot.png`, `docs/preview.png`, `scripts/264`, `scripts/265` | Keep |
| `docs/init.svg`, `docs/running.svg`, `docs/Example.gif` | **Delete** (0 refs, superseded by the coordinator-actor GIF) |
| `scripts/AVC-baseline`, `AVC-high`, `AVC-main`, `Safari-offer`, `gst-commands` | **Delete** (0 refs; superseded by `tests/browser/` and `useful_commands.md`) |
| `docs/proposals/*`, `docs/superpowers/*`, `docs/plan.md` | **Keep untouched** (history) |

All deletions have been confirmed to have **zero inbound references** (grep of
README/CONTEXT/src/tests/docs/Cargo). Git history preserves them if ever needed.

---

## Content plan per document

### CONTEXT.md (refine, don't rewrite from zero)

Its structure is sound (glossary · terminology map · module map · decided
constraints · dev env). Changes:

- **Add two glossary terms** the code relies on but the doc omits:
  - **Parked waiter** — a oneshot reply sender stored *inside* a connection's
    state (`AwaitingOffer.whep_reply` / `AwaitingAnswer.whip_reply`) instead of
    being answered immediately; delivering the SDP later completes the parked
    HTTP request.
  - **Reap** — cleanup triggered by the pipeline bus reporting a branch runtime
    failure (`reap_branch`); drops the connection without feeding the watchdog.
    (Currently only "sweep" is a term; "reap" is described obliquely.)
- **Add a "Go deeper" pointer** linking `connection-lifecycle.md` (the
  walkthrough) and `architecture-evolution.md` (the why).
- Verify all module-map file paths still resolve.

### README.md (rewrite, comprehensive — option A)

Preserve the valuable user/operator content; refresh architecture; fold in
`SRT_macOS.md`. Target section order:

1. Title, one-liner, badges, `screenshot.png`
2. Quick Demo (Open Source Cloud) — keep
3. **Architecture** (concise) — refresh prose, keep the coordinator-actor GIF,
   and link to `CONTEXT.md`, `connection-lifecycle.md`,
   `architecture-evolution.md`, and `docs/adr/`
4. Design Principles — keep (no-transcode default, server-initiated WHEP, SDP
   focus); keep the existing `whep.md` link here (it explains WHEP initiation modes)
5. Compliance Table — keep
6. Getting Started / Install — keep
7. Build from Source — OSX (**merge `SRT_macOS.md` env vars here**), Debian — keep
8. Docker — keep
9. Usage — keep
10. Testing — keep (browser check + `cargo test`), link `supported_codecs.md`
11. Tips for Successful Streaming — keep, link `useful_commands.md`, `LiveStreaming.md`, `known_limitations.md`
12. Discussion / License / Support / About — keep

### docs/connection-lifecycle.md (new — the one genuinely additive doc)

A concise, newcomer-facing "how a viewer connects **today**" walkthrough. Ported
from lesson 0001; prose only (no interactive widgets — those stay in `/teach`).

Sections:
- **The three-leg handshake** — `POST /channel` (empty body) → loopback WHIP
  offer at `POST /whip_sink/{id}` → `PATCH /channel/{id}` answer → Established.
  Rendered as a **mermaid** sequence diagram (repo already uses mermaid in
  `architecture-evolution.md`), replacing the lesson's inline SVG.
- **Parked waiters** — the one non-obvious idea: legs ① and ② do not reply
  immediately; their oneshot reply is parked in `ConnectionState` and completed
  by a later leg. This is where "stuck viewer / no response to POST" bugs live.
- **Route → command → state → reply table** — the compact reference from the
  lesson cheat sheet (which HTTP leg maps to which `Command`, resulting state,
  and when it replies).
- **Where to go next** — link `CONTEXT.md` (glossary),
  `architecture-evolution.md` (why the actor + the serialization trade-off),
  `docs/adr/`, and `src/signal/coordinator.rs` (code + its `#[cfg(test)]` spec).

**Explicitly NOT duplicated** (link instead of restating): the many-hands/one-map
actor rationale and the serialization trade-off — both already covered in
`architecture-evolution.md` §1–2 and §4 and `CONTEXT.md` decided constraints.
(These were lesson candidates L2a/L2b; they are already home.)

---

## Build order (staged; approve+land each before the next)

1. **Cleanup** — delete dead assets (`init.svg`, `running.svg`, `Example.gif`,
   `scripts/AVC-*`, `Safari-offer`, `gst-commands`). Confirm nothing links to
   them first.
2. **CONTEXT.md** — add the two glossary terms + the "go deeper" pointers.
3. **README.md** — rewrite around current architecture; merge `SRT_macOS.md`,
   then delete `SRT_macOS.md`.
4. **connection-lifecycle.md** — write the walkthrough; link it from README +
   CONTEXT.
5. **Relink + verify** — ensure every retained doc is reachable from the kernel;
   grep for links to deleted files (must be zero); confirm the `/teach`
   workspace remains git-excluded and untouched.

## Success criteria / verification

- No markdown link anywhere resolves to a deleted file (`init.svg`,
  `running.svg`, `Example.gif`, `SRT_macOS.md`, the deleted scripts).
- Every retained user-facing doc (`supported_codecs`, `known_limitations`,
  `whep`, `useful_commands`, `LiveStreaming`, `architecture-evolution`,
  `connection-lifecycle`, `CONTEXT`, `adr/*`) is reachable from `README.md` or
  `CONTEXT.md`.
- A newcomer can install and run srt-whep using only `README.md`.
- `connection-lifecycle.md` exists, renders its mermaid diagram, and is linked
  from both kernel docs.
- `git status` shows only the intended doc/script changes; `proposals/`,
  `superpowers/`, `plan.md`, and the `/teach` workspace are unchanged.

## Risks

- **Losing valuable README content in the rewrite.** Mitigation: rewrite is a
  restructure-and-refresh of existing sections, not a blank-page redraft; diff
  against the current README before landing.
- **Broken links after deletions.** Mitigation: the step-5 grep gate.
- **Accidental churn in the untouched history dirs.** Mitigation: stage explicit
  paths only; never `git add -A` (this repo has been bitten by a broad add).
