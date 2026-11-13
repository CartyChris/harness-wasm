// Opt-in LIVE turn through the real dev-server proxy (key stays server-side).
//   cargo run -p harness-proxy --bin dev-server   # 127.0.0.1:8788, key from env
//   SHOTS=<dir> node tests/e2e/live.mjs
// The page is served by a tiny static server; the proxy is cross-origin
// (CORS), exactly like the phone talking to the deployed proxy.
import { createServer } from 'node:http';
import { createRequire } from 'node:module';
import { existsSync, readdirSync, readFileSync, mkdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const ROOT = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const PROXY = process.env.PROXY || 'http://127.0.0.1:8788';
const SHOTS = process.env.SHOTS || join(ROOT, 'target', 'shots');
mkdirSync(SHOTS, { recursive: true });
function findPlaywright() {
  if (process.env.PW_CORE) return process.env.PW_CORE;
  const store = '/usr/local/lib/pnpm/5/.pnpm';
  if (existsSync(store)) {
    const hit = readdirSync(store).filter(function (d) { return d.indexOf('playwright-core@') === 0; }).sort()[0];
    if (hit) return join(store, hit, 'node_modules', 'playwright-core');
  }
  return 'playwright-core';
}
const { chromium } = require(findPlaywright());
const html = readFileSync(join(ROOT, 'dist', 'harness.html'));
const server = createServer(function (_req, res) { res.writeHead(200, { 'content-type': 'text/html' }); res.end(html); });
await new Promise(function (r) { server.listen(Number(process.env.LIVE_PORT || 8793), '127.0.0.1', r); });

const browser = await chromium.launch({ executablePath: process.env.CHROME || '/usr/bin/google-chrome', headless: true });
const context = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true });
await context.addInitScript(function (proxy) {
  if (window.top !== window) return;
  if (!localStorage.getItem('harness.settings')) localStorage.setItem('harness.settings', JSON.stringify({ base_url: proxy, token: '' }));
}, PROXY);
const page = await context.newPage();
const errors = [];
page.on('pageerror', function (e) { errors.push(String(e)); });
const chat = [];
page.on('response', async function (r) {
  if (r.url().indexOf('/api/chat') >= 0) chat.push({ status: r.status(), body: await r.text().catch(function () { return ''; }) });
});
page.on('request', function (r) {
  if (r.url().indexOf('/api/chat') >= 0) chat.push({ request: JSON.parse(r.postData() || '{}') });
});

await page.goto('http://127.0.0.1:' + (process.env.LIVE_PORT || 8793) + '/');
await page.waitForSelector('.composer-input');
await page.fill('.composer-input', 'Create notes.md containing one line: "live from grok". Then reply with one short sentence.');
await page.click('.send-btn');
await page.waitForSelector('.live', { timeout: 30000 });
await page.waitForTimeout(300);
await page.screenshot({ path: join(SHOTS, '18-live-streaming.png') });
for (let i = 0; i < 6; i++) {
  const got = await Promise.race([
    page.waitForSelector('.approve-btn.yes', { timeout: 60000 }).then(function () { return 'ask'; }),
    page.waitForSelector('.turn-meta, .turn-status.is-failed', { timeout: 60000 }).then(function () { return 'end'; }),
  ]);
  if (got === 'end') break;
  await page.screenshot({ path: join(SHOTS, '19-live-approval.png') });
  await page.click('.approve-btn.yes');
  await page.waitForTimeout(200);
}
await page.waitForTimeout(500);
await page.evaluate(function () { var t = document.getElementById('thread'); t.scrollTop = t.scrollHeight; });
await page.screenshot({ path: join(SHOTS, '20-live-completed.png') });

const reqs = chat.filter(function (c) { return c.request; });
const resps = chat.filter(function (c) { return 'status' in c; });
const chunks = resps.map(function (r) { return (r.body.match(/^data: /gm) || []).length; });
const transcript = await page.textContent('#thread');
const summary = {
  model_requests: reqs.length,
  models: [...new Set(reqs.map(function (r) { return r.request.model; }))],
  statuses: resps.map(function (r) { return r.status; }),
  sse_data_lines_per_request: chunks,
  tool_cards: await page.locator('.timeline .act').count(),
  done_cards: await page.locator('.act[data-status="ok"]').count(),
  turn_meta: (await page.textContent('.turn-meta').catch(function () { return null; })),
  failed: (await page.locator('.turn-status.is-failed').count()) > 0,
  page_errors: errors,
};
writeFileSync(join(SHOTS, 'live-ui-transcript.txt'), transcript + '\n\n' + resps.map(function (r) { return r.body; }).join('\n----\n'));
console.log(JSON.stringify(summary, null, 2));
await browser.close();
server.close();
