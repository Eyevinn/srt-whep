# Documentation Rebuild Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebuild srt-whep's documentation as an intentional kernel (`README.md` + `CONTEXT.md`) plus linked layers, deleting dead assets and adding a connection-lifecycle walkthrough.

**Architecture:** Two kernel docs point to everything else. `README.md` is project/user-facing; `CONTEXT.md` is the agent map. Every retained user-facing doc must be reachable from one of them (the no-orphan invariant). One net-new doc, `docs/connection-lifecycle.md`, ports the durable lesson content (three-leg handshake + parked waiters).

**Tech Stack:** Markdown; mermaid diagrams (rendered by GitHub); git.

**Spec:** `docs/superpowers/specs/2026-07-10-documentation-rebuild-design.md`

## Global Constraints

- **Branch:** work on `docs/rebuild` (already created from `main`).
- **No-orphan invariant:** every retained user-facing doc must be reachable by a link from `README.md` or `CONTEXT.md`. (Exempt: `proposals/`, `superpowers/`, `plan.md`, tooling docs like `docs/diagram/README.md`, and the git-excluded `/teach` workspace.)
- **Do NOT modify** `docs/proposals/*`, `docs/superpowers/*` (except adding this plan), `docs/plan.md`, or any `/teach` workspace file.
- **Stage explicit paths only.** Never `git add -A` or `git add -u` broadly — this repo has been bitten by a broad add sweeping concurrent WIP.
- **Terminology discipline:** *channel* (HTTP) / *connection* (signal) / *branch* (stream). Do not invent a fourth term.
- **All commits use these trailers** (the "standard trailers"):
  ```
  Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>
  Claude-Session: https://claude.ai/code/session_01Ff7JKFLhnJreVrWa2wFWKk
  ```

---

### Task 1: Delete dead assets

**Files:**
- Delete: `docs/init.svg`, `docs/running.svg`, `docs/Example.gif`
- Delete: `scripts/AVC-baseline`, `scripts/AVC-high`, `scripts/AVC-main`, `scripts/Safari-offer`, `scripts/gst-commands`

**Interfaces:**
- Consumes: nothing.
- Produces: a clean asset tree; later tasks' link checks assume these files are gone.

- [ ] **Step 1: Verify nothing links to them (the safety gate)**

Run:
```bash
grep -rIn --exclude-dir=target --exclude-dir=.git \
  -e 'init\.svg' -e 'running\.svg' -e 'Example\.gif' \
  -e 'AVC-baseline' -e 'AVC-high' -e 'AVC-main' -e 'Safari-offer' -e 'gst-commands' \
  README.md CONTEXT.md docs src tests CHANGELOG.md Cargo.toml
```
Expected: **no output** (the files are not referenced anywhere). If any real markdown link or code reference appears, STOP and reconsider that specific file — do not delete something still in use.

- [ ] **Step 2: Delete the files**

Run:
```bash
git rm docs/init.svg docs/running.svg docs/Example.gif \
  scripts/AVC-baseline scripts/AVC-high scripts/AVC-main scripts/Safari-offer scripts/gst-commands
```

- [ ] **Step 3: Confirm only deletions are staged**

Run: `git diff --cached --name-status`
Expected: eight lines, each beginning with `D`, exactly the files above. Nothing else.

- [ ] **Step 4: Commit**

```bash
git commit -m "docs: delete dead assets (unused diagrams, superseded SDP fixtures)

$(printf 'Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>\nClaude-Session: https://claude.ai/code/session_01Ff7JKFLhnJreVrWa2wFWKk')"
```
(Or write the message with the standard trailers via your editor/HEREDOC.)

---

### Task 2: Create `docs/connection-lifecycle.md`

**Files:**
- Create: `docs/connection-lifecycle.md`

**Interfaces:**
- Consumes: existing docs it links to (`../CONTEXT.md`, `./architecture-evolution-shared-lock-to-actor.md`, `./adr/`, `src/signal/coordinator.rs`) — all already exist.
- Produces: the walkthrough doc that Tasks 3 and 4 link to.

- [ ] **Step 1: Write the file**

Create `docs/connection-lifecycle.md` with exactly this content:

````markdown
# Connection Lifecycle

*How one WHEP viewer goes from `POST /channel` to receiving live video — the
happy path, leg by leg. For the domain vocabulary see
[`CONTEXT.md`](../CONTEXT.md); for **why** the design is shaped this way, see
[`architecture-evolution-shared-lock-to-actor.md`](./architecture-evolution-shared-lock-to-actor.md).*

## The three-leg handshake

srt-whep uses **server-initiated** WHEP: the browser POSTs an *empty* body and
the *server* supplies the SDP offer. That offer comes out of the GStreamer
pipeline through an in-process loopback WHIP bridge (see `CONTEXT.md` →
*Loopback WHIP*). Three HTTP requests carry a viewer to live video:

```mermaid
sequenceDiagram
    participant B as Browser (WHEP client)
    participant C as Coordinator (src/signal)
    participant W as whipsink (in the pipeline)

    B->>C: ① POST /channel (empty body)
    Note over C: add_branch(id)<br/>state = AwaitingOffer (① reply parked)
    C-->>W: branch attached; whipclientsink starts
    W->>C: ② POST /whip_sink/{id} (SDP offer)
    C-->>B: ① returns 201 + SDP offer
    Note over C: state = AwaitingAnswer (② reply parked)
    B->>C: ③ PATCH /channel/{id} (SDP answer)
    C-->>W: ② returns 201 + SDP answer
    C-->>B: ③ returns 204 No Content
    Note over C: state = Established — media flows
```

## The one non-obvious idea: parked waiters

Legs ① and ② cannot be answered when they arrive. When the browser POSTs
(leg ①), the SDP offer does not exist yet — the whipsink produces it only after
its branch is attached. So the coordinator does not reply; it **parks** the
reply channel inside the connection's state and returns to its loop:

```rust
// src/signal/coordinator.rs
enum ConnectionState {
    AwaitingOffer  { whep_reply: OfferReply,  deadline: Instant }, // leg ① parked here
    AwaitingAnswer { whip_reply: AnswerReply, deadline: Instant }, // leg ② parked here
    Established    { since: Instant },
}
```

When leg ② delivers the offer, the parked leg-① reply fires — that is the moment
the browser's original POST finally returns its `201`. Leg ② then parks in
turn, waiting for the browser's answer in leg ③. Each waiting state also carries
a `deadline`, so an abandoned handshake cannot park forever (the *sweep*
expires it — see `CONTEXT.md` → *Sweep*).

**Why this matters:** "viewer stuck / no response to the POST" almost always
means a parked waiter that never received its delivery. Knowing *where* each leg
parks tells you where to look.

## HTTP leg → command → state

| HTTP (route in `src/startup.rs`) | Caller | Command | Resulting state | Replies |
|---|---|---|---|---|
| `POST /channel` (empty body) | Browser | `CreateConnection` | `AwaitingOffer` | parked → `201` + SDP offer |
| `POST /whip_sink/{id}` (offer) | whipsink (loopback WHIP) | `OfferReceived` | `AwaitingAnswer` | parked → `201` + SDP answer |
| `PATCH /channel/{id}` (answer) | Browser | `AnswerReceived` | `Established` | immediate `204` |
| `DELETE /channel/{id}` or `/whip_sink/{id}` | either side | `RemoveConnection` | (removed) | immediate |
| `GET /list` | operator | `ListConnections` | (unchanged) | immediate JSON |

## Where to go next

- **Vocabulary and module map:** [`CONTEXT.md`](../CONTEXT.md)
- **Why an actor, the failure model, and the serialization trade-off:**
  [`architecture-evolution-shared-lock-to-actor.md`](./architecture-evolution-shared-lock-to-actor.md)
- **The decisions:** [`docs/adr/`](./adr/)
- **The code (and its `#[cfg(test)]` spec):** `src/signal/coordinator.rs`
````

- [ ] **Step 2: Verify every link target exists**

Run:
```bash
ls CONTEXT.md docs/architecture-evolution-shared-lock-to-actor.md docs/adr src/signal/coordinator.rs src/startup.rs
```
Expected: all listed without error.

- [ ] **Step 3: Commit**

```bash
git add docs/connection-lifecycle.md
git commit -m "docs: add connection-lifecycle walkthrough (three-leg handshake + parked waiters)

<standard trailers>"
```

---

### Task 3: Refine `CONTEXT.md` (glossary terms + walkthrough pointer)

**Files:**
- Modify: `CONTEXT.md`

**Interfaces:**
- Consumes: `docs/connection-lifecycle.md` (created in Task 2).
- Produces: a CONTEXT that names "parked waiter" and "reap" and points newcomers to the walkthrough.

- [ ] **Step 1: Add a newcomer pointer after the opening paragraph**

In `CONTEXT.md`, immediately after the opening paragraph (the `srt-whep ingests…` paragraph) and before `## Domain glossary`, insert:

```markdown
> **New to the code?** [`docs/connection-lifecycle.md`](docs/connection-lifecycle.md)
> walks one viewer through the handshake step by step. This file is the
> reference map; [`docs/architecture-evolution-shared-lock-to-actor.md`](docs/architecture-evolution-shared-lock-to-actor.md)
> is the *why* behind the coordinator-actor design.
```

- [ ] **Step 2: Add the "Parked waiter" glossary entry**

In the `## Domain glossary` list, immediately after the **Coordinator** bullet, insert:

```markdown
- **Parked waiter** — a oneshot reply sender held *inside* a connection's
  state instead of being answered right away. The WHEP `POST /channel` reply
  parks in `AwaitingOffer` until the whipsink's offer arrives; the loopback
  WHIP `POST` reply parks in `AwaitingAnswer` until the browser's answer
  arrives. Delivering the SDP later completes the long-held HTTP request. See
  [`docs/connection-lifecycle.md`](docs/connection-lifecycle.md).
```

- [ ] **Step 3: Add the "Reap" glossary entry**

In the `## Domain glossary` list, immediately after the **Sweep** bullet, insert:

```markdown
- **Reap** — cleanup triggered by the pipeline's bus watch reporting a
  branch's *runtime* failure (`reap_branch`): the connection is dropped and its
  branch detached. Unlike the sweep, a reap deliberately does **not** feed the
  watchdog — a dead peer is a fact about one viewer, not a pipeline-health
  signal.
```

- [ ] **Step 4: Verify the additions and links**

Run:
```bash
grep -n 'Parked waiter\|Reap\|connection-lifecycle' CONTEXT.md
```
Expected: matches for all three, and the `connection-lifecycle` path appears at least twice.

- [ ] **Step 5: Commit**

```bash
git add CONTEXT.md
git commit -m "docs(context): add parked-waiter and reap glossary terms + walkthrough pointer

<standard trailers>"
```

---

### Task 4: Refresh `README.md` and fold in `SRT_macOS.md`

**Files:**
- Modify: `README.md`
- Delete: `docs/SRT_macOS.md`

**Interfaces:**
- Consumes: `docs/connection-lifecycle.md`, `docs/architecture-evolution-shared-lock-to-actor.md`, the guides.
- Produces: a README that satisfies the no-orphan invariant.

- [ ] **Step 1: Refresh the Architecture section's "where to read more" line**

In `README.md`, in the `## Architecture` section, replace the line:

```markdown
For the module map and domain glossary see [`CONTEXT.md`](./CONTEXT.md); the design decisions behind this shape are recorded in [`docs/adr/`](./docs/adr/).
```

with:

```markdown
New to the code? [`docs/connection-lifecycle.md`](./docs/connection-lifecycle.md) walks one viewer through the handshake. For the module map and domain glossary see [`CONTEXT.md`](./CONTEXT.md); the *why* behind the coordinator-actor design is in [`docs/architecture-evolution-shared-lock-to-actor.md`](./docs/architecture-evolution-shared-lock-to-actor.md), and the decisions in [`docs/adr/`](./docs/adr/).
```

- [ ] **Step 2: Fold the `SRT_macOS.md` delta into the OSX build section**

`docs/SRT_macOS.md`'s only content not already in the README OSX env block is `GIO_EXTRA_MODULES` (needed to run a local SRT *test source* on macOS). In `README.md`, in the `### OSX` section, immediately after the blockquote that ends `…break the WebRTC media path (see [`docs/adr/0003`]…)` (the one right before `Build with Cargo`), insert:

```markdown
To run a local SRT **test source** on macOS (for example the ffmpeg/GStreamer commands in [`docs/useful_commands.md`](./docs/useful_commands.md)), also export `GIO_EXTRA_MODULES` so GStreamer's gio TLS modules resolve:

```
export GIO_EXTRA_MODULES=/Library/Frameworks/GStreamer.framework/Libraries/gio/modules/
```
```

- [ ] **Step 3: Make the guides discoverable from the Tips section**

In `README.md`, at the end of the `## Tips for Successful Streaming` section (just before `## Discussion and Issues`), insert:

```markdown
See also: [test-stream commands](./docs/useful_commands.md), the [OBS → Twitch streaming guide](./docs/LiveStreaming.md), and [known issues and solutions](./docs/known_limitations.md).
```

- [ ] **Step 4: Delete `docs/SRT_macOS.md`**

Run: `git rm docs/SRT_macOS.md`

- [ ] **Step 5: Verify README links and the deletion**

Run:
```bash
grep -n 'SRT_macOS' README.md docs/*.md          # expected: no output
grep -n 'connection-lifecycle\|architecture-evolution\|useful_commands\|LiveStreaming\|known_limitations' README.md   # expected: all present
```
Expected: first grep empty; second grep shows all five link targets present.

- [ ] **Step 6: Commit**

```bash
git add README.md
git rm docs/SRT_macOS.md   # if not already staged
git commit -m "docs(readme): link the walkthrough + deep dive, fold in SRT_macOS, surface guides

<standard trailers>"
```

---

### Task 5: Final verification — no broken links, no orphans, history untouched

**Files:** none (verification only).

**Interfaces:**
- Consumes: all prior tasks.
- Produces: confidence the rebuild is consistent.

- [ ] **Step 1: Broken-link gate (references to deleted files)**

Run:
```bash
grep -rIn --exclude-dir=target --exclude-dir=.git \
  -e 'SRT_macOS' -e 'init\.svg' -e 'running\.svg' -e 'Example\.gif' \
  -e 'AVC-baseline' -e 'AVC-high' -e 'AVC-main' -e 'Safari-offer' -e 'gst-commands' \
  README.md CONTEXT.md docs
```
Expected: **no output** (ignore matches inside `docs/proposals/`, `docs/superpowers/`, or `docs/plan.md` if any appear — those are frozen history; but there should be none in README/CONTEXT/top-level docs).

- [ ] **Step 2: Orphan gate (every retained user-facing doc is reachable)**

Run:
```bash
for d in CONTEXT.md docs/connection-lifecycle.md docs/architecture-evolution-shared-lock-to-actor.md \
         docs/supported_codecs.md docs/known_limitations.md docs/whep.md \
         docs/useful_commands.md docs/LiveStreaming.md; do
  base=$(basename "$d")
  hits=$(grep -l "$base" README.md CONTEXT.md 2>/dev/null | wc -l | tr -d ' ')
  echo "$hits  $d"
done
```
Expected: every line shows `1` or `2` (each doc linked from README and/or CONTEXT). Any `0` is an orphan — add a link and re-run. (`docs/adr/` is reachable as a directory link from both; no per-file check needed.)

- [ ] **Step 3: Confirm frozen history and the /teach workspace are untouched**

Run:
```bash
git diff --name-only main..HEAD
```
Expected: only `README.md`, `CONTEXT.md`, `docs/connection-lifecycle.md`, the deleted assets, `docs/SRT_macOS.md`, and the spec/plan files under `docs/superpowers/`. **No** files under `docs/proposals/`, no pre-existing files under `docs/superpowers/plans|specs|reviews` other than the two we added, no `docs/plan.md`, and nothing from the `/teach` workspace (which is git-excluded anyway).

- [ ] **Step 4: Note on rendering**

Mermaid renders on GitHub natively; no local tooling is needed. Optionally open `docs/connection-lifecycle.md` in a Markdown previewer that supports mermaid to eyeball the sequence diagram.

- [ ] **Step 5 (only if Steps 1–3 required fixes): Commit the fixes**

```bash
git add <the fixed files>
git commit -m "docs: fix links/orphans found in final verification

<standard trailers>"
```
If Steps 1–3 passed clean, there is nothing to commit here.

---

## Self-Review

**Spec coverage:** Every spec section maps to a task — cleanup (T1), connection-lifecycle.md (T2), CONTEXT refinements (T3), README rewrite + SRT_macOS fold (T4), relink/verify (T5). The "keep untouched" and "keep+link" buckets are enforced by the Global Constraints and T5's gates.

**Placeholder scan:** No TBD/TODO; all doc content is given verbatim; verification steps have exact commands and expected output. Commit messages reference the standard-trailers block defined once in Global Constraints (DRY, not a placeholder).

**Type/name consistency:** State names (`AwaitingOffer`/`AwaitingAnswer`/`Established`), commands (`CreateConnection`/`OfferReceived`/`AnswerReceived`/`RemoveConnection`/`ListConnections`), routes (`/channel`, `/whip_sink/{id}`, `/list`), and file paths are used identically across the new doc, the CONTEXT entries, and the route table, and match the code read during design.
