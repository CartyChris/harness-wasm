import { startMock } from './mock-proxy.mjs';
import { createRequire } from 'node:module';
const require = createRequire(import.meta.url);
// Storage compatibility in the REAL builds, both directions:
//   Stage 2 build writes a chat -> Stage 3 build boots it (migration)
//   Stage 3 build adds a tool turn -> Stage 2 build still reads everything (rollback)
//   git show abe4c8b:dist/harness.html > /tmp/stage2.html && node tests/e2e/storage-compat.mjs
import { execFileSync } from 'node:child_process';
import { existsSync } from 'node:fs';
if (!existsSync('/tmp/stage2.html')) execFileSync('sh', ['-c', 'git show abe4c8b:dist/harness.html > /tmp/stage2.html']);
const { chromium } = require('/usr/local/lib/pnpm/5/.pnpm/playwright-core@1.59.1/node_modules/playwright-core');
const browser = await chromium.launch({ executablePath: '/usr/bin/google-chrome', headless: true });
async function run(html, seed) {
  const { server } = await startMock(8805, html);
  const ctx = await browser.newContext({ viewport: { width: 390, height: 844 } });
  await ctx.addInitScript(function (seed) {
    if (window.top !== window) return;
    if (seed && !localStorage.getItem('harness.sessions')) for (const k in seed) localStorage.setItem(k, seed[k]);
    if (!localStorage.getItem('harness.settings')) localStorage.setItem('harness.settings', JSON.stringify({ base_url: location.origin, token: 'demo-token' }));
  }, seed);
  const page = await ctx.newPage();
  const errs = []; page.on('pageerror', e => errs.push(String(e)));
  await page.goto('http://127.0.0.1:8805/');
  return { page, ctx, server, errs };
}
// Stage 2 build: make a real chat.
let s = await run('/tmp/stage2.html', null);
const input = await s.page.$('textarea');
await input.fill('Show me what markdown looks like');
await s.page.keyboard.press('Control+Enter').catch(() => {});
await s.page.waitForTimeout(300);
const send = await s.page.$('button[aria-label="Send"]'); if (send) await send.click().catch(() => {});
await s.page.waitForTimeout(3000);
const dump = await s.page.evaluate(() => { const o = {}; for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); o[k] = localStorage.getItem(k); } return o; });
const s2 = JSON.parse(dump['harness.sessions'] || '[]');
console.log('stage2 sessions', s2.length, 'messages', s2[0] && s2[0].messages && s2[0].messages.map(m => m.role + ':' + String(m.content).slice(0, 30)), 'has events key', s2[0] && 'events' in s2[0], 'errors', s.errs);
await s.ctx.close(); s.server.close();
// Stage 3 build on the same origin, seeded with that exact storage.
s = await run('/workspace/harness-wasm/dist/harness.html', dump);
await s.page.waitForSelector('.composer-input');
await s.page.waitForTimeout(500);
const users = await s.page.$$eval('.msg-user', els => els.map(e => e.textContent.trim()));
const asst = await s.page.$$eval('.msg-assistant .md', els => els.map(e => e.textContent.trim().slice(0, 40)));
const s3 = JSON.parse(await s.page.evaluate(() => localStorage.getItem('harness.sessions')));
console.log('stage3 renders user', users, 'assistant', asst, 'title', await s.page.textContent('.topbar-title, .title').catch(() => '?'));
console.log('stage3 stored events', s3[0].events && s3[0].events.map(e => e.kind), 'errors', s.errs);
const ok = users.length === 1 && users[0].indexOf('markdown') >= 0 && asst.length === 1 && asst[0].length > 10 && s.errs.length === 0;
console.log(ok ? 'MIGRATION PROOF: PASS' : 'MIGRATION PROOF: FAIL');
// Stage 3 tool turn (tip calculator, Auto-allow both writes).
await s.page.click('.suggest >> nth=0').catch(async () => {
  await s.page.fill('.composer-input', 'Build a single-file HTML tip calculator in the workspace (index.html), then summarize it.');
  await s.page.click('.send-btn');
});
for (let i = 0; i < 4; i++) {
  const got = await Promise.race([
    s.page.waitForSelector('.approve-btn.yes', { timeout: 15000 }).then(() => 'ask'),
    s.page.waitForSelector('.turn-meta >> nth=1', { timeout: 15000 }).then(() => 'end'),
  ]).catch(() => 'end');
  if (got === 'end') break;
  await s.page.click('.approve-btn.yes');
  await s.page.waitForTimeout(150);
}
await s.page.waitForTimeout(400);
const dump3 = await s.page.evaluate(() => { const o = {}; for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); o[k] = localStorage.getItem(k); } return o; });
await s.ctx.close(); s.server.close();
const r = await run('/tmp/stage2.html', dump3);
await r.page.waitForTimeout(1500);
const sessions2 = await r.page.evaluate(() => JSON.parse(localStorage.getItem('harness.sessions') || '[]'));
const body2 = await r.page.textContent('body');
const lost = body2.indexOf('could not be read') >= 0;
const okBack = !lost && sessions2.length >= 1 && r.errs.length === 0 &&
  JSON.stringify(sessions2).indexOf('tip calculator') >= 0 && JSON.stringify(sessions2).indexOf('quick tour') >= 0;
console.log('rollback: stage2 sees sessions', sessions2.length, 'titles', sessions2.map(x => x.title), 'storage-wipe note', lost, 'errors', r.errs);
console.log(okBack ? 'ROLLBACK PROOF: PASS' : 'ROLLBACK PROOF: FAIL');
await browser.close(); r.server.close(); process.exit(ok && okBack ? 0 : 1);
