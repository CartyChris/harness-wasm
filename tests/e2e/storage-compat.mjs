// Storage compatibility across the REAL builds, in one browser profile (so
// IndexedDB persists between builds, as it would on the phone):
//   A  Stage 2 writes a chat            → Stage 3.1 boots it (migration)
//   B  Stage 3.1 adds a tool turn       → Stage 2 still reads every chat (rollback)
//   C  Stage 2 (rolled back) adds a Q+A → Stage 3.1 keeps its tool cards AND absorbs the new Q+A
//   D  Stage 3 build (events in localStorage) → Stage 3.1 moves them into IndexedDB intact
// Builds come from git: abe4c8b (Stage 2), 40e1883 (Stage 3), dist/ (this tree).
import { startMock } from './mock-proxy.mjs';
import { createRequire } from 'node:module';
import { execFileSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const ROOT = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const { chromium } = require(process.env.PW_CORE || '/usr/local/lib/pnpm/5/.pnpm/playwright-core@1.59.1/node_modules/playwright-core');
for (const [rev, out] of [['abe4c8b', '/tmp/stage2.html'], ['40e1883', '/tmp/stage3.html']]) {
  execFileSync('sh', ['-c', 'git -C "' + ROOT + '" show ' + rev + ':dist/harness.html > ' + out]);
}
const S2 = '/tmp/stage2.html', S3 = '/tmp/stage3.html', S31 = join(ROOT, 'dist', 'harness.html');
const PORT = 8805;
const browser = await chromium.launch({ executablePath: process.env.CHROME || '/usr/bin/google-chrome', headless: true });
let failures = 0;
function proof(name, ok, detail) { console.log((ok ? 'PASS ' : 'FAIL ') + name + (detail ? ' — ' + detail : '')); if (!ok) failures++; }

async function profile() {
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 } });
  await ctx.addInitScript(function () {
    if (window.top !== window) return;
    if (!localStorage.getItem('harness.settings')) localStorage.setItem('harness.settings', JSON.stringify({ base_url: location.origin, token: 'demo-token' }));
  });
  const page = await ctx.newPage();
  const errs = [];
  page.on('pageerror', function (e) { errs.push(String(e)); });
  return { ctx, page, errs };
}
// Serve one build on the shared origin and load it.
let server = null;
async function load(p, html) {
  if (server) await new Promise(function (r) { server.close(r); server.closeAllConnections && server.closeAllConnections(); });
  const m = await startMock(PORT, html);
  server = m.server;
  await p.page.goto('http://127.0.0.1:' + PORT + '/');
  await p.page.waitForSelector('textarea');
  await p.page.waitForTimeout(600);
}
async function send(p, text) {
  await p.page.fill('textarea', text);
  const before = await p.page.locator('.msg-assistant').count();
  await p.page.click('button[aria-label="Send"]');
  await p.page.waitForFunction(function (n) { return document.querySelectorAll('.msg-assistant').length > n && !document.querySelector('button[aria-label="Stop"]'); }, before, { timeout: 20000 });
  await p.page.waitForTimeout(400);
}
const lsSessions = function (p) { return p.page.evaluate(function () { return JSON.parse(localStorage.getItem('harness.sessions') || '[]'); }); };
const idbSessions = function (p) {
  return p.page.evaluate(async function () {
    const db = await new Promise(function (res, rej) { const r = indexedDB.open('harness'); r.onsuccess = function () { res(r.result); }; r.onerror = rej; });
    if (!db.objectStoreNames.contains('kv')) return [];
    return await new Promise(function (res) { const out = []; const c = db.transaction('kv').objectStore('kv').openCursor(); c.onsuccess = function () { const cur = c.result; if (!cur) return res(out); if (String(cur.key).indexOf('s:') === 0) out.push(JSON.parse(cur.value)); cur.continue(); }; });
  });
};

// ---------------------------------------------------------------- A + B + C
const p = await profile();
await load(p, S2);
await send(p, 'Show me what markdown looks like');
const s2 = await lsSessions(p);
await load(p, S31);
const usersA = await p.page.$$eval('.msg-user', function (els) { return els.map(function (e) { return e.textContent.trim(); }); });
const idbA = await idbSessions(p);
proof('A  MIGRATION Stage 2 → 3.1: chat renders and lives in IndexedDB as events',
  s2.length === 1 && usersA.join() === 'Show me what markdown looks like' && idbA.length === 1 && idbA[0].events.map(function (e) { return e.kind; }).join() === 'user/message,turn/start,assistant/message,turn/end',
  'idb kinds: ' + (idbA[0] ? idbA[0].events.map(function (e) { return e.kind; }).join(' ') : 'none'));

// 3.1 tool turn (Auto mode so no approvals)
await p.page.click('button[aria-label="Approval mode"]');
await p.page.click('.menu-item >> text=Auto');
await p.page.fill('.composer-input', 'Build a single-file HTML tip calculator in the workspace (index.html), then summarize it.');
await p.page.click('.send-btn');
await p.page.waitForSelector('.turn-meta >> nth=1', { timeout: 20000 });
await p.page.waitForTimeout(500);
const cardsB = await p.page.locator('.timeline .act').count();
const ls31 = await lsSessions(p);
proof('B0 3.1 keeps only the text mirror in localStorage (no events there)', ls31.every(function (s) { return s.events.length === 0 && s.messages.length >= 2; }), JSON.stringify(ls31.map(function (s) { return [s.messages.length, s.events.length]; })));

await load(p, S2);
const body2 = await p.page.textContent('body');
const s2b = await lsSessions(p);
proof('B  ROLLBACK 3.1 → Stage 2: Stage 2 reads every chat, no wipe',
  body2.indexOf('could not be read') < 0 && p.errs.length === 0 && JSON.stringify(s2b).indexOf('tip calculator') >= 0 && JSON.stringify(s2b).indexOf('quick tour') >= 0,
  s2b.map(function (s) { return s.title + ':' + s.messages.length; }).join(' | '));
// Stage 2 continues the chat after the rollback. (Stage 2's own uid bug —
// ids restart at -1 per page load — makes its reply land on an older message
// with the same id, so wait for the request, not for a new bubble.)
await p.page.fill('textarea', 'One more markdown tour please');
await p.page.click('button[aria-label="Send"]');
await p.page.waitForFunction(function () { return !document.querySelector('button[aria-label="Stop"]'); }, null, { timeout: 20000 });
await p.page.waitForTimeout(1500);
const s2c = await lsSessions(p);

await load(p, S31);
const cardsC = await p.page.locator('.timeline .act').count();
const usersC = await p.page.$$eval('.msg-user', function (els) { return els.map(function (e) { return e.textContent.trim(); }); });
proof('C  ROUND TRIP 3.1 → 2 → 3.1: tool cards kept (event log) and the Stage 2 Q+A absorbed',
  cardsB === 2 && cardsC === 2 && usersC.indexOf('One more markdown tour please') >= 0 && usersC.length === 3,
  'cards ' + cardsB + '→' + cardsC + ' · users: ' + usersC.join(' | ') + ' · s2 msgs ' + s2c.map(function (s) { return s.messages.length; }).join(','));
await p.ctx.close();

// ---------------------------------------------------------------------- D
const q = await profile();
await load(q, S3);
await q.page.click('button[aria-label="Approval mode"]');
await q.page.click('.menu-item >> text=Auto');
await q.page.fill('.composer-input', 'Build a single-file HTML tip calculator in the workspace (index.html), then summarize it.');
await q.page.click('.send-btn');
await q.page.waitForSelector('.turn-meta', { timeout: 20000 });
await q.page.waitForTimeout(500);
const s3 = await lsSessions(q);
await load(q, S31);
const cardsD = await q.page.locator('.timeline .act').count();
const idbD = await idbSessions(q);
const lsD = await lsSessions(q);
proof('D  Stage 3 (events in localStorage) → 3.1: logs moved to IndexedDB intact, localStorage slimmed',
  s3[0].events.length > 5 && idbD.length === 1 && JSON.stringify(idbD[0].events) === JSON.stringify(s3[0].events) && cardsD === 2 && lsD[0].events.length === 0,
  s3[0].events.length + ' events · cards ' + cardsD + ' · ls bytes ' + JSON.stringify(s3).length + ' → ' + JSON.stringify(lsD).length);
proof('no page errors', p.errs.length === 0 && q.errs.length === 0, p.errs.concat(q.errs).join(' | '));
await q.ctx.close();

await browser.close();
server.close();
console.log(failures === 0 ? 'storage-compat: PASS' : 'storage-compat: ' + failures + ' FAILED');
process.exit(failures === 0 ? 0 : 1);
