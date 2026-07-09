# Automated Browser-Driven WHEP Test — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A first-class, repo-level, one-shot command that brings up srt-whep + an SRT source, drives the real installed Google Chrome through the full WHEP handshake, checks the connection stages, and exits 0/1 with both a human report and structured JSON.

**Architecture:** Clean split — **shell owns process lifecycle** (`run.sh` orchestrates bring-up → serve player → drive → teardown), **Node owns the browser** (`drive-chrome.mjs` launches real Chrome via Puppeteer, clicks Connect on the existing diagnostic player, scrapes the stage DOM). Pass/fail logic is a pure, unit-tested module (`verdict.mjs`) so it can be TDD'd without a browser. Reuses the codec-test skill's hard-won bring-up scripts (promoted into the repo) so the tsdemux link-race retry comes for free.

**Tech Stack:** Bash, Node 20+ (ESM `.mjs`), `puppeteer-core` (drives system Chrome — no bundled Chromium download), `node --test` (built-in test runner), Python `http.server` (serves the player), existing GStreamer + srt-whep release binary.

## Global Constraints

- **Platform:** macOS only. GStreamer framework at `/Library/Frameworks/GStreamer.framework/Versions/Current`; `lib/env.sh` exports the framework env. Do NOT prepend a locally-built gst-plugins-rs to `GST_PLUGIN_PATH` (shadows the framework's matched rswebrtc — see `docs/adr/0003`).
- **Browser:** the real system **Google Chrome** via explicit `executablePath` (default `/Applications/Google Chrome.app/Contents/MacOS/Google Chrome`, override with `CHROME_PATH`), NOT Puppeteer's bundled Chromium — Chromium builds often lack the proprietary H.264 decoder and would report `framesDecoded: 0` on a good stream. srt-whep outputs H.264.
- **Dependency:** `puppeteer-core` only, isolated in `tests/browser/`. `tests/browser/node_modules/` is gitignored. The Rust crate, `Cargo.*`, and `cargo test` are untouched (`cargo` ignores `tests/browser/` — no `*.rs` there).
- **Media gate (the crux):** a run passes only if `connectionState === "connected"` AND `framesDecoded` is **strictly increasing and > 0** across the media-wait window (default **5s**, `--timeout`). This distinguishes real playout from "connected but zero media" (the version-skew bug) and from a single stuck frame.
- **Output:** human per-stage report to stdout + structured JSON to `target/codec-test/whep-auto-<profile>.json` (already-gitignored dir) + exit code `0` (all gates pass) / `1` (any fail or timeout).
- **Skill files are untracked:** `.claude/skills/run-srt-whep-codec-test/` is git-ignored. Promoting its scripts = plain `mv` + `git add` at the new path (NOT `git mv`). Editing its `SKILL.md` produces no git change (nothing to commit for it).
- **Branch:** do this work on a feature branch (`feat/browser-whep-test`), not `main`. Create it before Task 1.
- **Commits:** conventional-commit style (`feat:`/`test:`/`docs:`/`chore:`). Per the environment's git rules, every commit message ends with the two trailers (`Co-Authored-By:` and `Claude-Session:`); commit `-m` lines below omit them for brevity — append them at commit time.

---

## File Structure

```
tests/browser/
  README.md              # how to run, prerequisites, reading output       (Task 5)
  package.json           # pins puppeteer-core                              (Task 4)
  package-lock.json      # committed lockfile                               (Task 4)
  run.sh                 # ONE-SHOT orchestrator (bash)                     (Task 5)
  drive-chrome.mjs       # Puppeteer driver (Node)                          (Task 4)
  verdict.mjs            # pure pass/fail + parsing + report logic          (Task 3)
  verdict.test.mjs       # node:test unit tests for verdict.mjs             (Task 3)
  lib/
    env.sh               # macOS GStreamer framework env      (moved, Task 1)
    bringup.sh           # source + srt-whep, tsdemux retry    (moved, Task 1)
    server.sh            # srt-whep listener launcher          (moved, Task 1)
    source-x264.sh       # x264 SRT test source                (moved, Task 1)
    source-vt.sh         # VideoToolbox live source            (moved, Task 1)
    source-file.sh       # VideoToolbox file-loop source       (moved, Task 1)
  player/
    index.html           # diagnostic WHEP player              (moved, Task 1)
```

`.gitignore` gains `tests/browser/node_modules/` (Task 1). The skill dir keeps only its (untracked) `SKILL.md`, repointed at these paths (Task 2).

---

## Task 1: Promote bring-up scripts + player into the repo

Move the six shell scripts and the diagnostic player out of the untracked skill dir into `tests/browser/`, preserving their sibling layout so every `$DIR`-relative reference keeps working. Ignore `node_modules`.

**Files:**
- Create dir: `tests/browser/lib/`, `tests/browser/player/`
- Move (plain `mv`): `.claude/skills/run-srt-whep-codec-test/scripts/{env.sh,bringup.sh,server.sh,source-x264.sh,source-vt.sh,source-file.sh}` → `tests/browser/lib/`
- Move (plain `mv`): `.claude/skills/run-srt-whep-codec-test/player/index.html` → `tests/browser/player/index.html`
- Modify: `.gitignore` (append one line)

**Interfaces:**
- Produces: `tests/browser/lib/bringup.sh <profile> [source-script]` — brings up srt-whep (WHEP on `:8000`, SRT listener `:1234`) + an SRT source, retries the tsdemux race, leaves both running disowned, exits 0 on success. `tests/browser/lib/env.sh` — sourced for the macOS GStreamer framework env. `tests/browser/player/index.html` — served over http; the WHEP handshake page with stable DOM ids (`#url`, `#play`, `#pc`, `#ice`, `#frames`, `#bytes`, `#codec`, `#size`, `#log`, `#offer`, `#answer`).

- [ ] **Step 1: Create the feature branch**

```bash
cd /Users/kunwu/Workspace/srt/srt-whep
git checkout -b feat/browser-whep-test
```

- [ ] **Step 2: Create the target directories**

```bash
mkdir -p tests/browser/lib tests/browser/player
```

- [ ] **Step 3: Move the scripts and player (plain mv — the skill dir is untracked)**

```bash
SK=.claude/skills/run-srt-whep-codec-test
mv "$SK"/scripts/env.sh          tests/browser/lib/env.sh
mv "$SK"/scripts/bringup.sh      tests/browser/lib/bringup.sh
mv "$SK"/scripts/server.sh       tests/browser/lib/server.sh
mv "$SK"/scripts/source-x264.sh  tests/browser/lib/source-x264.sh
mv "$SK"/scripts/source-vt.sh    tests/browser/lib/source-vt.sh
mv "$SK"/scripts/source-file.sh  tests/browser/lib/source-file.sh
mv "$SK"/player/index.html       tests/browser/player/index.html
chmod +x tests/browser/lib/*.sh
```

- [ ] **Step 4: Ignore node_modules**

Append to `.gitignore`:

```
tests/browser/node_modules
```

- [ ] **Step 5: Verify the scripts are syntactically valid and env sources cleanly**

Run:
```bash
bash -n tests/browser/lib/bringup.sh && \
bash -n tests/browser/lib/server.sh && \
bash -n tests/browser/lib/source-x264.sh && \
sh -n tests/browser/lib/env.sh && \
echo "syntax OK"
```
Expected: `syntax OK`

- [ ] **Step 6: Verify the player's relative path from bringup.sh still resolves**

`bringup.sh` prints a hint using `$DIR/../player`. From `tests/browser/lib/`, `../player/index.html` must exist:
```bash
test -f tests/browser/lib/../player/index.html && echo "player path OK"
```
Expected: `player path OK`

- [ ] **Step 7: Commit**

```bash
git add tests/browser/lib tests/browser/player .gitignore
git commit -m "test(browser): promote WHEP bring-up scripts + diagnostic player into the repo"
```

---

## Task 2: Repoint the codec-test skill at the promoted files

Update the skill's (untracked) `SKILL.md` so its manual quick-start references the repo copies instead of the now-removed `scripts/`/`player/` under the skill. No git commit results — the file is gitignored — but the skill must still work.

**Files:**
- Modify: `.claude/skills/run-srt-whep-codec-test/SKILL.md` (untracked)

**Interfaces:**
- Consumes: `tests/browser/lib/bringup.sh`, `tests/browser/lib/source-file.sh`, `tests/browser/player/` (from Task 1).

- [ ] **Step 1: Repoint the Quick-start block**

In `SKILL.md`, replace the Quick-start shell block:

```sh
S=.claude/skills/run-srt-whep-codec-test
# 1. Bring up srt-whep + a source of the given profile (retries past the tsdemux race)
"$S/scripts/bringup.sh" constrained-baseline            # x264 profile
# 2. Serve the diagnostic player (separate terminal)
( cd "$S/player" && python3 -m http.server 8080 --bind 127.0.0.1 )
# 3. Open http://localhost:8080/ in Chrome AND Safari, click ▶ Connect
```

with:

```sh
# Automated (Chrome, one-shot): bring up + drive + check stages + teardown
tests/browser/run.sh --profile constrained-baseline

# Manual (Chrome AND Safari, read the panel by hand):
# 1. Bring up srt-whep + a source of the given profile (retries past the tsdemux race)
tests/browser/lib/bringup.sh constrained-baseline       # x264 profile
# 2. Serve the diagnostic player (separate terminal)
( cd tests/browser/player && python3 -m http.server 8080 --bind 127.0.0.1 )
# 3. Open http://localhost:8080/ in Chrome AND Safari, click ▶ Connect
```

- [ ] **Step 2: Repoint the VideoToolbox / file-source examples**

Replace the two `"$S/scripts/bringup.sh" …` VideoToolbox examples:

```sh
"$S/scripts/bringup.sh" main
"$S/scripts/bringup.sh" high
```
and
```sh
"$S/scripts/bringup.sh" constrained_high "$S/scripts/source-file.sh"
```

with:

```sh
tests/browser/lib/bringup.sh main
tests/browser/lib/bringup.sh high
```
and
```sh
tests/browser/lib/bringup.sh constrained_high tests/browser/lib/source-file.sh
```

- [ ] **Step 3: Repoint the teardown block's http.server line (paths unchanged, still 8080)**

The teardown `pkill` lines are process-name based and need no change. Confirm the doc no longer references `$S/scripts` or `$S/player` anywhere:

```bash
grep -nE '\$S/(scripts|player)|skills/run-srt-whep-codec-test/(scripts|player)' \
  .claude/skills/run-srt-whep-codec-test/SKILL.md || echo "no stale skill paths"
```
Expected: `no stale skill paths`

- [ ] **Step 4: Verify the repointed paths exist**

```bash
test -x tests/browser/lib/bringup.sh && test -f tests/browser/player/index.html && echo "repoint OK"
```
Expected: `repoint OK`

(No commit — `SKILL.md` is gitignored. Deliverable is the working, repointed skill.)

---

## Task 3: Pure verdict logic (TDD)

Build the browser-free module that parses the player's DOM/log strings and decides pass/fail, with unit tests first. This is where the media-gate semantics (including the version-skew `0 → 0` case) are locked down.

**Files:**
- Create: `tests/browser/verdict.mjs`
- Test: `tests/browser/verdict.test.mjs`

**Interfaces:**
- Produces (imported by Task 4's `drive-chrome.mjs`):
  - `parseOfferStatus(log: string): number | null` — first 3-digit HTTP status after a `→` in the player log.
  - `videoRejected(log: string): boolean` — true if the log has the `ANSWER REJECTED VIDEO` line.
  - `parseVideoBytes(bytesText: string): number` — the video half of the player's `"v / a"` bytes field.
  - `framesVerdict(first: number, last: number): { first, last, ok: boolean }` — `ok` iff `last > 0 && last > first`.
  - `computeVerdict(r: {profile, offerStatus, connection, log, framesFirst, framesLast, videoBytes, codec, frameSize}): { pass: boolean, profile: string, failedStage: string|null, stages: object }`.
  - `formatReport(v: verdict, log: string): string` — the human per-stage report.

- [ ] **Step 1: Write the failing tests**

Create `tests/browser/verdict.test.mjs`:

```js
import test from 'node:test';
import assert from 'node:assert/strict';
import {
  parseOfferStatus, videoRejected, parseVideoBytes, framesVerdict, computeVerdict,
} from './verdict.mjs';

test('parseOfferStatus reads the first 3-digit status after an arrow', () => {
  assert.equal(parseOfferStatus('POST …\n  → 201 Created\nsetRemote…'), 201);
  assert.equal(parseOfferStatus('  → 503 Service Unavailable'), 503);
  assert.equal(parseOfferStatus('no status here'), null);
});

test('videoRejected detects the port-0 rejection line', () => {
  assert.equal(videoRejected('… ⚠️ ANSWER REJECTED VIDEO (m=video port 0) …'), true);
  assert.equal(videoRejected('all good'), false);
});

test('parseVideoBytes reads the video half of "v / a"', () => {
  assert.equal(parseVideoBytes('240113 / 15022'), 240113);
  assert.equal(parseVideoBytes('0 / 0'), 0);
  assert.equal(parseVideoBytes(''), 0);
});

test('framesVerdict requires climbing and > 0', () => {
  assert.equal(framesVerdict(5, 187).ok, true);   // real playout
  assert.equal(framesVerdict(0, 0).ok, false);     // version-skew: connected, no media
  assert.equal(framesVerdict(1, 1).ok, false);     // stuck single frame
});

test('computeVerdict passes only when every gate is ok', () => {
  const good = computeVerdict({
    profile: 'constrained-baseline', offerStatus: 201, connection: 'connected',
    log: '  → 201 Created', framesFirst: 5, framesLast: 187,
    videoBytes: 240113, codec: 'video/H264', frameSize: '1280x720',
  });
  assert.equal(good.pass, true);
  assert.equal(good.failedStage, null);

  const noMedia = computeVerdict({
    profile: 'x', offerStatus: 201, connection: 'connected',
    log: 'ok', framesFirst: 0, framesLast: 0,
    videoBytes: 0, codec: '', frameSize: '',
  });
  assert.equal(noMedia.pass, false);
  assert.equal(noMedia.failedStage, 'frames');

  const noOffer = computeVerdict({
    profile: 'x', offerStatus: null, connection: 'error',
    log: 'EXCEPTION: Failed to fetch', framesFirst: 0, framesLast: 0,
    videoBytes: 0, codec: '', frameSize: '',
  });
  assert.equal(noOffer.pass, false);
  assert.equal(noOffer.failedStage, 'offer');
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `node --test tests/browser/verdict.test.mjs`
Expected: FAIL — `Cannot find module … verdict.mjs` (module does not exist yet).

- [ ] **Step 3: Implement `verdict.mjs`**

Create `tests/browser/verdict.mjs`:

```js
// Pure, browser-free logic for the WHEP browser test: parse the diagnostic
// player's DOM/log strings and decide pass/fail. Kept separate from the
// Puppeteer I/O so it is unit-testable without a browser or a live pipeline.

export function parseOfferStatus(log) {
  const m = String(log).match(/→\s+(\d{3})\b/);
  return m ? Number(m[1]) : null;
}

export function videoRejected(log) {
  return /ANSWER REJECTED VIDEO/.test(String(log));
}

export function parseVideoBytes(bytesText) {
  const m = String(bytesText).match(/(\d+)/);
  return m ? Number(m[1]) : 0;
}

export function framesVerdict(first, last) {
  const f = Number(first), l = Number(last);
  return { first: f, last: l, ok: Number.isFinite(f) && Number.isFinite(l) && l > 0 && l > f };
}

// The gate order also defines which failing stage is reported first.
const GATES = ['offer', 'connection', 'videoRejected', 'frames', 'videoBytes'];

export function computeVerdict(r) {
  const frames = framesVerdict(r.framesFirst, r.framesLast);
  const stages = {
    offer:         { ok: r.offerStatus !== null && r.offerStatus < 400, status: r.offerStatus },
    connection:    { ok: r.connection === 'connected', value: r.connection },
    videoRejected: { ok: !videoRejected(r.log) },
    frames:        { ...frames, increasing: frames.last > frames.first },
    videoBytes:    { ok: Number(r.videoBytes) > 0, value: Number(r.videoBytes) || 0 },
    codec:         { value: r.codec || '' },
    frameSize:     { value: r.frameSize || '' },
  };
  const failedStage = GATES.find((g) => !stages[g].ok) || null;
  return { pass: failedStage === null, profile: r.profile, failedStage, stages };
}

export function formatReport(v, log) {
  const mark = (ok) => (ok ? '✓' : '✗');
  const s = v.stages;
  const lines = [
    `WHEP browser test — profile=${v.profile} — ${v.pass ? 'PASS' : 'FAIL'}` +
      (v.failedStage ? ` (failed: ${v.failedStage})` : ''),
    `  ${mark(s.offer.ok)} offer            HTTP ${s.offer.status ?? '—'}`,
    `  ${mark(s.connection.ok)} connection       ${s.connection.value || '—'}`,
    `  ${mark(s.videoRejected.ok)} video accepted   ${s.videoRejected.ok ? 'yes' : 'NO (m=video port 0)'}`,
    `  ${mark(s.frames.ok)} frames decoded   ${s.frames.first} → ${s.frames.last}` +
      (s.frames.ok ? ' (climbing)' : ''),
    `  ${mark(s.videoBytes.ok)} video bytes      ${s.videoBytes.value}`,
    `    codec          ${s.codec.value || '—'}`,
    `    frame size     ${s.frameSize.value || '—'}`,
  ];
  return lines.join('\n');
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `node --test tests/browser/verdict.test.mjs`
Expected: PASS — all 5 tests green (`# pass 5`).

- [ ] **Step 5: Commit**

```bash
git add tests/browser/verdict.mjs tests/browser/verdict.test.mjs
git commit -m "test(browser): add pure WHEP verdict logic with unit tests"
```

---

## Task 4: Chrome driver

The Puppeteer script that launches real Chrome, clicks Connect on the served player, samples the stages across the media-wait window, and emits report + JSON + exit code. Integration is proven in Task 6; here it must syntax-check and its dependency install.

**Files:**
- Create: `tests/browser/drive-chrome.mjs`
- Create: `tests/browser/package.json`
- Create (generated): `tests/browser/package-lock.json`

**Interfaces:**
- Consumes: `verdict.mjs` (`parseOfferStatus`, `parseVideoBytes`, `computeVerdict`, `formatReport`); the served player's DOM ids; `puppeteer-core`.
- Produces (for Task 5's `run.sh`): CLI `node drive-chrome.mjs --url <player> --endpoint <whep> --profile <p> --timeout <sec> [--headed]`; exit `0` pass / `1` fail; JSON at `target/codec-test/whep-auto-<profile>.json`.

- [ ] **Step 1: Create `package.json`**

Create `tests/browser/package.json`:

```json
{
  "name": "srt-whep-browser-test",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "description": "Automated browser-driven WHEP connection test (Chrome via Puppeteer).",
  "scripts": {
    "test": "node --test"
  },
  "dependencies": {
    "puppeteer-core": "^23.0.0"
  }
}
```

- [ ] **Step 2: Install the dependency (system Chrome — no bundled download)**

Run:
```bash
( cd tests/browser && npm install )
```
Expected: creates `tests/browser/node_modules/` (gitignored) and `tests/browser/package-lock.json`; no Chromium download (`puppeteer-core` bundles no browser).

- [ ] **Step 3: Implement `drive-chrome.mjs`**

Create `tests/browser/drive-chrome.mjs`:

```js
#!/usr/bin/env node
// Drive the real system Google Chrome through the WHEP handshake on the
// diagnostic player, sample the connection stages, and emit report + JSON +
// exit code. Owns the browser only; all pass/fail logic lives in verdict.mjs.
import puppeteer from 'puppeteer-core';
import { writeFileSync, mkdirSync } from 'node:fs';
import { dirname } from 'node:path';
import {
  parseOfferStatus, parseVideoBytes, computeVerdict, formatReport,
} from './verdict.mjs';

function parseArgs(argv) {
  const a = {};
  for (let i = 0; i < argv.length; i++) {
    const k = argv[i];
    if (!k.startsWith('--')) continue;
    const name = k.slice(2);
    if (argv[i + 1] && !argv[i + 1].startsWith('--')) { a[name] = argv[++i]; }
    else { a[name] = true; }
  }
  return a;
}

const args = parseArgs(process.argv.slice(2));
const PLAYER_URL   = args.url      || 'http://localhost:8080/';
const ENDPOINT     = args.endpoint || 'http://localhost:8000/channel';
const PROFILE      = args.profile  || 'unknown';
const MEDIA_WAIT   = Number(args.timeout || 5) * 1000;
const CONNECT_WAIT = Number(args['connect-timeout'] || 10) * 1000;
const HEADED       = args.headed === true;
const JSON_OUT     = args.json || `target/codec-test/whep-auto-${PROFILE}.json`;
const CHROME_PATH  = args['chrome-path'] || process.env.CHROME_PATH ||
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';

const text = (page, id) =>
  page.$eval('#' + id, (el) => el.textContent.trim()).catch(() => '');

const browser = await puppeteer.launch({
  executablePath: CHROME_PATH,
  headless: HEADED ? false : 'new',
  args: ['--autoplay-policy=no-user-gesture-required'],
});

try {
  const page = await browser.newPage();
  await page.goto(PLAYER_URL, { waitUntil: 'load' });
  await page.$eval('#url', (el, v) => { el.value = v; }, ENDPOINT);
  await page.click('#play');

  // Wait for the connection badge to settle to a terminal-ish state (or the
  // connect timeout); a swallowed timeout just means we read whatever we reached.
  await page.waitForFunction(() => {
    const s = document.getElementById('pc').textContent.trim();
    return ['connected', 'failed', 'error'].includes(s) || s.startsWith('server');
  }, { timeout: CONNECT_WAIT }).catch(() => {});

  const connection  = await text(page, 'pc');
  const framesFirst = Number(await text(page, 'frames')) || 0;
  await new Promise((r) => setTimeout(r, MEDIA_WAIT)); // media-wait window
  const framesLast  = Number(await text(page, 'frames')) || 0;

  const log       = await page.$eval('#log',    (el) => el.textContent).catch(() => '');
  const bytesText = await text(page, 'bytes');
  const codec     = await text(page, 'codec');
  const frameSize = await text(page, 'size');
  const offerM    = await page.$eval('#offer',  (el) => el.textContent).catch(() => '');
  const answerM   = await page.$eval('#answer', (el) => el.textContent).catch(() => '');

  const verdict = computeVerdict({
    profile: PROFILE,
    offerStatus: parseOfferStatus(log),
    connection, log, framesFirst, framesLast,
    videoBytes: parseVideoBytes(bytesText),
    codec, frameSize,
  });

  const result = {
    ...verdict, endpoint: ENDPOINT,
    offerVideoMLine: offerM, answerVideoMLine: answerM,
    log: String(log).split('\n'),
  };
  mkdirSync(dirname(JSON_OUT), { recursive: true });
  writeFileSync(JSON_OUT, JSON.stringify(result, null, 2));

  console.log(formatReport(verdict, log));
  console.log(`\nJSON: ${JSON_OUT}`);
  process.exitCode = verdict.pass ? 0 : 1;
} finally {
  await browser.close();
}
```

- [ ] **Step 4: Syntax-check the driver**

Run: `node --check tests/browser/drive-chrome.mjs && echo "driver syntax OK"`
Expected: `driver syntax OK`

- [ ] **Step 5: Re-run the unit tests (guard against an import break)**

Run: `( cd tests/browser && npm test )`
Expected: PASS — `# pass 5`.

- [ ] **Step 6: Commit**

```bash
git add tests/browser/package.json tests/browser/package-lock.json tests/browser/drive-chrome.mjs
git commit -m "feat(browser): add Puppeteer driver that checks WHEP stages in real Chrome"
```

---

## Task 5: One-shot orchestrator + README

The bash entry point that sequences bring-up → serve player → drive Chrome → teardown, with a `trap` guaranteeing cleanup. Plus the README a fresh user needs.

**Files:**
- Create: `tests/browser/run.sh`
- Create: `tests/browser/README.md`

**Interfaces:**
- Consumes: `lib/bringup.sh` (Task 1), `player/index.html` (Task 1), `drive-chrome.mjs` (Task 4), `python3 -m http.server`, `npm`.
- Produces: CLI `tests/browser/run.sh [--profile <p>] [--timeout <sec>] [--headed] [--endpoint <url>] [--skip-bringup] [--player-port <n>]`; exit code == the driver's verdict.

- [ ] **Step 1: Implement `run.sh`**

Create `tests/browser/run.sh`:

```bash
#!/usr/bin/env bash
# One-shot automated browser WHEP test:
#   bring up SRT source + srt-whep -> serve the diagnostic player -> drive the
#   real system Chrome through the WHEP handshake -> check the stages -> teardown.
# Exit code is the driver's verdict (0 pass / 1 fail). Teardown always runs.
#
# Usage: run.sh [--profile <p>] [--timeout <sec>] [--headed]
#               [--endpoint <url>] [--skip-bringup] [--player-port <n>]
set -euo pipefail
DIR="$(cd "$(dirname "$0")" && pwd)"

PROFILE=constrained-baseline
TIMEOUT=5
PLAYER_PORT=8080
ENDPOINT=""
SKIP_BRINGUP=0
HEADED=""
while [ $# -gt 0 ]; do
  case "$1" in
    --profile)      PROFILE="$2"; shift 2 ;;
    --timeout)      TIMEOUT="$2"; shift 2 ;;
    --player-port)  PLAYER_PORT="$2"; shift 2 ;;
    --endpoint)     ENDPOINT="$2"; shift 2 ;;
    --skip-bringup) SKIP_BRINGUP=1; shift ;;
    --headed)       HEADED="--headed"; shift ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done
[ -n "$ENDPOINT" ] || ENDPOINT="http://localhost:8000/channel"

HTTP_PID=""
teardown() {
  [ -n "$HTTP_PID" ] && kill "$HTTP_PID" 2>/dev/null || true
  pkill -9 -f 'target/release/srt-whep'  2>/dev/null || true
  pkill -9 -f 'gst-launch-1.0.*srtsink'  2>/dev/null || true
  pkill -9 -f 'ffmpeg.*mpegts'           2>/dev/null || true
}
trap teardown EXIT

# 1. dependency
if [ ! -d "$DIR/node_modules" ]; then
  echo ">>> installing puppeteer-core…"
  ( cd "$DIR" && npm install --silent )
fi

# 2. bring up source + srt-whep (retries the tsdemux race)
if [ "$SKIP_BRINGUP" -eq 0 ]; then
  echo ">>> bringing up srt-whep + x264 source (profile=$PROFILE)…"
  "$DIR/lib/bringup.sh" "$PROFILE"
fi

# 3. serve the player
echo ">>> serving player on 127.0.0.1:$PLAYER_PORT…"
( cd "$DIR/player" && exec python3 -m http.server "$PLAYER_PORT" --bind 127.0.0.1 ) >/dev/null 2>&1 &
HTTP_PID=$!
for _ in $(seq 1 10); do
  curl -s -o /dev/null "http://127.0.0.1:$PLAYER_PORT/" && break
  sleep 0.3
done

# 4. drive Chrome and check the stages (its exit code is this script's verdict)
echo ">>> driving Chrome…"
node "$DIR/drive-chrome.mjs" \
  --url "http://localhost:$PLAYER_PORT/" \
  --endpoint "$ENDPOINT" \
  --profile "$PROFILE" \
  --timeout "$TIMEOUT" \
  $HEADED
```

- [ ] **Step 2: Make it executable and syntax-check it**

Run:
```bash
chmod +x tests/browser/run.sh && bash -n tests/browser/run.sh && echo "run.sh syntax OK"
```
Expected: `run.sh syntax OK`

- [ ] **Step 3: Write the README**

Create `tests/browser/README.md`:

````markdown
# Automated browser-driven WHEP test

Brings up srt-whep + an SRT source, drives the **real system Google Chrome**
through the full WHEP handshake, checks the connection stages, and exits `0`
(all gates pass) / `1` (any fail). Emits a per-stage report and a JSON file.

This is the automated counterpart to the manual `run-srt-whep-codec-test`
skill — same diagnostic player, same bring-up scripts, no clicking.

## Prerequisites (macOS)

- Release binary built: `cargo build --release` (see repo README for the
  GStreamer framework setup).
- GStreamer framework with `rswebrtc` (`gst-inspect-1.0 whipclientsink` works).
- Google Chrome installed at `/Applications/Google Chrome.app` (or set
  `CHROME_PATH`). **Real Chrome, not Chromium** — Chromium may lack the H.264
  decoder and would show 0 frames on a good stream.
- Node 20+ and `npm` on PATH. `ffmpeg` only for VideoToolbox profiles.

## Run

```sh
tests/browser/run.sh                                  # constrained-baseline, 5s media wait
tests/browser/run.sh --profile main --timeout 8       # another profile, longer wait
tests/browser/run.sh --headed                         # watch the Chrome window
tests/browser/run.sh --skip-bringup \
  --endpoint http://localhost:8000/channel            # drive against an already-running server
```

First run does `npm install` (installs `puppeteer-core`; no browser download).

## Reading the output

A per-stage table prints to stdout; full detail (incl. offer/answer video
m-lines and the player log) lands in `target/codec-test/whep-auto-<profile>.json`.

| Stage | Pass condition |
|---|---|
| offer | server returned an SDP offer (not 503/error) |
| connection | `connectionState === connected` |
| video accepted | no `m=video port 0` rejection in the answer |
| **frames decoded** | **strictly increasing and > 0** across the media wait |
| video bytes | `> 0` |

The frames gate is the crux: `connection connected` but `frames 0 → 0` is the
classic "connected but zero media" failure (e.g. a GStreamer/plugin version
skew), and this test fails it.

## Teardown

`run.sh` tears everything down on exit (even on Ctrl-C). If a run is killed
uncleanly:

```sh
pkill -9 -f 'target/release/srt-whep'; pkill -9 -f 'gst-launch-1.0.*srtsink'
pkill -9 -f 'ffmpeg.*mpegts'; pkill -9 -f 'http.server'
```
````

- [ ] **Step 4: Commit**

```bash
git add tests/browser/run.sh tests/browser/README.md
git commit -m "feat(browser): add one-shot WHEP test orchestrator + README"
```

---

## Task 6: End-to-end verification (positive + negative)

Prove the tool actually discriminates on real hardware: a good stream passes, and a no-server case fails with the right stage. This is the spec's "verify the tool itself" gate.

**Files:**
- (No new files; may modify `tests/browser/drive-chrome.mjs` only if a launch/read fix is needed.)

**Interfaces:**
- Consumes: everything from Tasks 1–5; the built release binary.

- [ ] **Step 1: Ensure the release binary exists**

Run:
```bash
test -x target/release/srt-whep || ( . tests/browser/lib/env.sh && cargo build --release )
test -x target/release/srt-whep && echo "binary OK"
```
Expected: `binary OK`

- [ ] **Step 2: Positive run — good x264 stream must PASS**

Run:
```bash
tests/browser/run.sh --profile constrained-baseline; echo "EXIT=$?"
```
Expected: report shows `PASS`, `connection connected`, `frames decoded N → M (climbing)` with `M > N > 0`, a `video/H264` codec with a `profile-level-id`, and `EXIT=0`. JSON written to `target/codec-test/whep-auto-constrained-baseline.json` with `"pass": true`.

If frames stay `0` headless (decode not running without a display), re-run with `--headed` to confirm the pipeline is fine, then add `'--use-gl=swiftshader'` to the `args` array in `drive-chrome.mjs` and re-test headless. Commit that fix if needed.

- [ ] **Step 3: Inspect the JSON artifact**

Run:
```bash
cat target/codec-test/whep-auto-constrained-baseline.json
```
Expected: valid JSON, `pass: true`, `stages.frames.increasing: true`, `stages.frames.last > stages.frames.first`.

- [ ] **Step 4: Negative run — no server must FAIL with stage `offer`**

With nothing running on `:8000` (the positive run's teardown already killed it), drive against a dead endpoint:
```bash
tests/browser/run.sh --skip-bringup --endpoint http://127.0.0.1:8000/channel; echo "EXIT=$?"
```
Expected: report shows `FAIL (failed: offer)` (the POST is refused → no offer), and `EXIT=1`. Confirms the tool distinguishes failure from success.

- [ ] **Step 5: Confirm no orphaned processes remain**

Run:
```bash
pgrep -fl 'target/release/srt-whep|gst-launch-1.0.*srtsink|http.server' || echo "clean — no orphans"
```
Expected: `clean — no orphans` (teardown worked).

- [ ] **Step 6: Commit any fixes made during verification**

```bash
git add -A tests/browser/
git commit -m "test(browser): verify positive + negative WHEP runs end-to-end" || echo "no fixes needed to commit"
```

(If Step 2/4 required no code change, there may be nothing to commit — that is fine.)

---

## Self-Review

**Spec coverage:**
- Full one-shot orchestration → Task 5 (`run.sh`). ✓
- Real Chrome (not Chromium), H.264 rationale → Global Constraints + Task 4 `executablePath` + README. ✓
- Reuse existing player, DOM-scrape the stages → Task 4 (`text(page, id)` reads) + Task 1 (player moved intact, unchanged). ✓
- Both outputs (report + JSON) + exit 0/1 → Task 4 (`formatReport`, `writeFileSync`, `process.exitCode`). ✓
- Frames strictly-increasing media gate, 5s default → Task 3 (`framesVerdict`), Task 4 (`MEDIA_WAIT`), Global Constraints. ✓
- Promote scripts to repo, repoint skill → Tasks 1 & 2. ✓
- Reliability: tsdemux retry, x264 default, http same-origin, always-teardown → Task 1 (bringup reused), Task 5 (`trap`, x264 default, python http.server). ✓
- Verify positive + negative → Task 6. ✓
- Local-only, CI seam left (headless + exit code + JSON, no workflow added) → satisfied by Tasks 4–5; no CI task, as decided. ✓

**Placeholder scan:** No TBD/TODO; every code step has complete code; every command has an expected result. The only conditional is Task 6 Step 2's documented headless-decode fallback (a real, specific remedy, not a placeholder).

**Type consistency:** `parseOfferStatus`, `parseVideoBytes`, `computeVerdict`, `formatReport`, `framesVerdict` — names and signatures match between `verdict.mjs` (Task 3), its tests (Task 3), and `drive-chrome.mjs` imports (Task 4). DOM ids used in `drive-chrome.mjs` (`url`, `play`, `pc`, `frames`, `bytes`, `codec`, `size`, `log`, `offer`, `answer`) all exist in the promoted `player/index.html`. CLI flags produced by `drive-chrome.mjs` match those passed by `run.sh` (`--url`, `--endpoint`, `--profile`, `--timeout`, `--headed`).
