# Driver prompt — architecture deepening pass

You are driving the architecture-deepening pass for the srt-whep repo at `/Users/kunwu/Workspace/srt/srt-whep`, described in `docs/superpowers/reviews/2026-07-07-architecture-review.md`. Read that document in full before doing anything else — it defines the vocabulary, the binding constraints, six candidates with per-candidate acceptance criteria ("Done when"), and the execution order. Your job is to execute the candidates one at a time, in this order:

**C1 → C2 → C5 → C3a → C3b → C3c → C4** (C6 only if C4 makes it fall out naturally).

## Setup (once)

1. Create a working branch off `main`: `arch/deepening`.
2. Commit the two handoff docs (`docs/superpowers/reviews/*.md`) as the first commit so the artifacts are versioned.
3. Every shell that runs `cargo test` / `cargo run` needs:
   `export DYLD_FALLBACK_LIBRARY_PATH=/Library/Frameworks/GStreamer.framework/Versions/Current/lib`
   Without it the test binaries abort at dyld load.
4. Baseline to preserve: 23 unit + 9 integration tests green (at `f1867d3`). The `--ignored` GStreamer e2e has a known hang history — always run it with a timeout (~5 min) and report a hang as a finding; never wait indefinitely.

## Binding rules

- The review doc's **"Constraints: decided questions"** section is non-negotiable: keep the loopback-WHIP bridge; keep branch calls serialized inside the coordinator actor; keep per-connection isolation + watchdog semantics. If a change seems to require violating one of these, stop and ask — do not push through.
- Never regress anything in the **"What is already deep — do not regress"** list.
- Refactor candidates (C1, C5, most of C3) land with the existing suite green at **every** commit. New behaviour (C2's supervisor, C4's error mapping) is developed test-first.
- Treat each candidate's **"Done when"** line as the acceptance criteria. If you can't meet it as written, stop and report rather than redefining it.

## Working with superpowers skills

- **Brainstorming is already done.** The review doc's candidate sections are approved design output (design review 2026-07-07). Do not run `superpowers:brainstorming` interactively for work the review doc covers — go straight to `superpowers:writing-plans` (C2, C3) or a todo list (C1, C4, C5). Only brainstorm if you hit genuinely undesigned territory, and then via the decision policy below, not by interviewing an absent user.
- **Choose the execution mode per candidate, after writing the plan:**
  - **Execute directly** (executing-plans style, you hold the context) when tasks are coupled, ordered, or knowledge accrues: **C1** (one refactor rippling through trait + both impls + tests), **C2** (sequential TDD), **C3** (GStreamer context is expensive to rebuild; keep it in one head and run the e2e between stages), **C4**.
  - **`superpowers:subagent-driven-development`** (or dispatching-parallel-agents) when the plan's tasks are independent tickets with local verification: **C5** (seven disjoint hygiene items). Run its per-task spec-compliance and code review — while unattended, that review substitutes for the human's.
  - Rule of thumb: tasks sharing no files and no ordering → subagents; chapters of one story → direct.

## Decision policy

Classify every choice you hit:

1. **Decided** — anything in the review doc's constraints section. Never re-open, never ask.
2. **Anticipated** — take these defaults unless evidence contradicts them; record any deviation in the progress log:
   - C1: trait names `BranchControl` / `PipelineLifecycle`; rename `add_connection`→`add_branch`, `remove_connection`→`remove_branch` at the trait seam; `TestPipeline` stays in production source (integration tests require it).
   - C2: shutdown via a `tokio::sync::watch` channel (no new dependencies); actix gets `.disable_signals()`; the assembly fn returns a struct holding server + supervisor handle + `SignalHandle`.
   - C3b: `Branch` owns all per-connection element-name constants; `WHIP_SINK_ROUTE` const lives in `stream` and is imported by `startup.rs`.
   - C4: exactly three variants — `PipelineError { NotReady, Transient(String), Fatal(String) }`.
3. **Novel** — reversible + local + no public-contract change → decide, log the decision *and the rejected alternative*, continue. Contract-changing, constraint-touching, or destructive → stop condition: write the question plus your recommendation to the progress log, ask the user (AskUserQuestion if attended; otherwise notify and pause that candidate), and continue with a candidate that isn't blocked on it (C5 is independent of everything; C4 needs only C1; C2/C3 are ordered).

## Process per candidate

1. **Re-read** the candidate's section and open the files it lists — do not work from memory of the doc, especially after context compaction.
2. **Plan.** For C2 and C3 (the ≥1-day ones): write an implementation plan to `docs/superpowers/plans/` (repo convention: date-prefixed filename) before coding — use the superpowers writing-plans / executing-plans skills. For C1, C4, C5: a todo list is enough.
3. **Implement** in small commits on `arch/deepening`, subjects prefixed with the candidate: `refactor(c1): ...`, `feat(c2): ...`, `chore(c5): ...`.
4. **Verify.**
   - Always: full `cargo test`.
   - C3 stages: additionally run the `--ignored` e2e (with timeout) after each stage.
   - C2's shutdown fix: manual smoke test — build and run the binary (any dummy SRT address), press Ctrl-C **once**, confirm clean exit; before the fix it takes two.
   - C5's CI job: model the GStreamer apt install on `publish.yml`; validate the workflow YAML.
5. **Checkpoint.** Append a dated entry to `docs/superpowers/reviews/2026-07-07-architecture-review-progress.md`: candidate, what changed, test results, and any deviation from the review doc. Post a short summary. Then continue to the next candidate — don't wait for approval unless a stop condition applies.

## Stop conditions (pause and ask instead of proceeding)

- Acceptance criteria unreachable as written, or meeting them requires contradicting the constraints section.
- The e2e regresses relative to baseline.
- You discover the review's diagnosis is factually wrong (e.g. a "dead" item has a caller). Report what you actually found before acting on it.
- Anything requiring a force-push, history rewrite, or touching `publish.yml` release behaviour.

## Small print

- Leave the review doc itself frozen; record deviations only in the progress log.
- Do not commit the `pipe/`, `source/`, `srt/` `.dot`-dump debris; C5 gitignores and deletes them.
- If your context is compacted mid-run, re-orient from the progress log + review doc — they are the source of truth, not your memory of earlier turns.
- When all candidates are done (or you hit a stop), finish with: candidates completed, commits per candidate, final test counts vs baseline, and any deferred items — then recommend merge/PR next steps rather than merging to `main` yourself.
