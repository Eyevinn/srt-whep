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
