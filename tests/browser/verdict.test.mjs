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
