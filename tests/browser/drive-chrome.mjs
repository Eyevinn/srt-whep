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
