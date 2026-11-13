// End-to-end UI checks + 390×844 screenshots for the folded-repo features.
// Drives the packed dist/harness.html in headless Chrome against the loopback
// mock proxy (tests/e2e/mock-proxy.mjs). Exits non-zero on any failed check.
//
//   PW_CORE=<path to playwright-core> SHOTS=<dir> node tests/e2e/run.mjs
//
// Every check is named after the repo pattern it proves.

import { startMock } from './mock-proxy.mjs';
import { createRequire } from 'node:module';
import { existsSync, mkdirSync, readdirSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const require = createRequire(import.meta.url);
const ROOT = dirname(dirname(dirname(fileURLToPath(import.meta.url))));
const HTML = join(ROOT, 'dist', 'harness.html');
const SHOTS = process.env.SHOTS || join(ROOT, 'target', 'shots');
const PORT = Number(process.env.E2E_PORT || 8799);
if (!process.argv.includes('--probe')) mkdirSync(SHOTS, { recursive: true });

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
const CHROME = process.env.CHROME || '/usr/bin/google-chrome';
// --probe: exit 0 iff playwright-core + Chromium are usable (check-ship gate).
if (process.argv.includes('--probe')) process.exit(existsSync(CHROME) ? 0 : 1);

let failures = 0;
function check(name, ok, detail) {
  console.log((ok ? 'PASS ' : 'FAIL ') + name + (detail ? ' — ' + detail : ''));
  if (!ok) failures++;
}

const { server, requests } = await startMock(PORT, HTML);
const browser = await chromium.launch({ executablePath: CHROME, headless: true });
const context = await browser.newContext({
  viewport: { width: 390, height: 844 },
  deviceScaleFactor: 2,
  isMobile: true,
  hasTouch: true,
});
await context.addInitScript(function () {
  // Top document only: inside the sandboxed artifact iframe storage access
  // throws by design (no allow-same-origin).
  if (window.top !== window) return;
  if (!localStorage.getItem('harness.settings')) {
    localStorage.setItem('harness.settings', JSON.stringify({ base_url: location.origin, token: 'demo-token' }));
  }
});
const page = await context.newPage();
const pageErrors = [];
page.on('pageerror', function (e) { pageErrors.push(e.message); });

async function shot(name) {
  await page.waitForTimeout(260); // let sheet/menu entry animations settle
  const layout = await page.evaluate(function () {
    const vw = window.innerWidth;
    const doc = document.documentElement.scrollWidth;
    const appEl = document.querySelector('.app');
    const thread = document.querySelector('.thread');
    const sheetBody = document.querySelector('.sheet-body');
    // Visible buttons under 44px (the §7 tap-target gate).
    const small = [];
    document.querySelectorAll('button').forEach(function (b) {
      const r = b.getBoundingClientRect();
      const cs = getComputedStyle(b);
      if (r.width === 0 || r.height === 0 || cs.visibility === 'hidden') return;
      if (b.classList.contains('overlay-backdrop') || b.classList.contains('menu-scrim')) return;
      if (r.height < 43.5 || r.width < 43.5) small.push((b.className || b.tagName) + ' ' + Math.round(r.width) + 'x' + Math.round(r.height));
    });
    const smallInputs = [];
    document.querySelectorAll('input, textarea, select').forEach(function (el) {
      if (parseFloat(getComputedStyle(el).fontSize) < 16) smallInputs.push(el.className);
    });
    return {
      vw: vw,
      doc: doc,
      app: appEl ? appEl.scrollWidth : 0,
      thread: thread ? thread.scrollWidth : 0,
      sheet: sheetBody ? sheetBody.scrollWidth - sheetBody.clientWidth : 0,
      small: small,
      smallInputs: smallInputs,
    };
  });
  const overflow = layout.doc > layout.vw || layout.app > layout.vw || layout.thread > layout.vw || layout.sheet > 1;
  check('layout[' + name + '] no horizontal overflow at 390', !overflow, JSON.stringify({ doc: layout.doc, app: layout.app, thread: layout.thread, sheet: layout.sheet }));
  check('layout[' + name + '] tap targets >= 44px', layout.small.length === 0, layout.small.join(', '));
  check('layout[' + name + '] inputs >= 16px', layout.smallInputs.length === 0, layout.smallInputs.join(', '));
  await page.screenshot({ path: join(SHOTS, name + '.png') });
}

const lastBody = function () { return requests.length ? requests[requests.length - 1].body : null; };
const userTexts = function (body) {
  return (body.messages || []).filter(function (m) { return m.role === 'user'; }).map(function (m) { return m.content; });
};

// ---------------------------------------------------------------- 1. boot
await page.goto('http://127.0.0.1:' + PORT + '/');
await page.waitForSelector('.composer-input');
check('boot: fresh profile model is x-ai/grok-4.7', (await page.textContent('.model-name')).trim() === 'grok-4.7');
check('boot: empty state with suggestions', (await page.locator('.suggest').count()) === 4);
await shot('01-empty-state');

// ------------------------------------------ 2. codex tool loop + opencode ask
await page.click('.suggest >> nth=0');
await page.waitForSelector('.live.has-text');
await page.waitForTimeout(450);
await shot('02-streaming-mid-turn');
await page.waitForSelector('.approve', { timeout: 15000 });
const first = requests[0].body;
check('codex: request carries the tools array', Array.isArray(first.tools) && first.tools.length === 6, first.tools ? first.tools.map(function (t) { return t.function.name; }).join(',') : 'none');
check('codex: model locked to x-ai/grok-4.7 in the request', first.model === 'x-ai/grok-4.7');
check('aider: system message carries the workspace map', first.messages[0].role === 'system' && /Workspace/.test(first.messages[0].content));
check('secrets: no key-shaped value in the request', JSON.stringify(first).indexOf('sk-or-') < 0);
check('opencode: write_file waits for approval in Ask mode', (await page.textContent('.act[data-status="ask"] .act-badge')).indexOf('Needs approval') >= 0);
await shot('03-approval-card-opencode');

await page.click('.approve-btn.yes');
await page.waitForSelector('.diff', { timeout: 10000 });
await page.waitForSelector('.act[data-status="ask"]', { timeout: 10000 });
await page.click('.act[data-status="ask"] .approve-btn.always');
await page.waitForSelector('.turn-meta', { timeout: 15000 });
const toolCards = await page.locator('.timeline .act').count();
check('codex: 3-step turn = text + 2 tool cards + final text', toolCards === 2 && (await page.locator('.msg-assistant .md').count()) >= 2, toolCards + ' cards');
check('codex: loop sent tool results back (3 model requests)', requests.length === 3, requests.length + ' requests');
const third = lastBody();
const roles = third.messages.map(function (m) { return m.role; }).join(',');
check('deepseek: derived history = system,user,assistant,tool,assistant,tool', roles === 'system,user,assistant,tool,assistant,tool', roles);
check('openhands: both cards observed (Done badges)', (await page.locator('.act[data-status="ok"]').count()) === 2);
const rules = await page.evaluate(function () { return localStorage.getItem('harness.rules'); });
check('opencode: "Always allow" persisted a per-tool rule', /"edit_file":"allow"/.test(rules || ''), rules);
await page.evaluate(function () { document.getElementById('thread').scrollTop = 0; });
await page.waitForTimeout(100);
await page.evaluate(function () { var t = document.getElementById('thread'); t.scrollTop = t.scrollHeight; });
await shot('04-completed-turn-tools-diff');

// Expand the first card: Action → Observation (OpenHands).
await page.click('.timeline .act >> nth=0 >> .act-head');
await page.waitForSelector('.act.is-open .act-body');
const actionText = await page.textContent('.act.is-open .act-body');
check('openhands: expanded card shows Action and Observation', actionText.indexOf('Action') >= 0 && actionText.indexOf('Observation') >= 0 && actionText.indexOf('created index.html') >= 0);
await page.locator('.act.is-open').scrollIntoViewIfNeeded();
await shot('05-timeline-action-observation-openhands');
await page.click('.act.is-open .act-head');

// ------------------------------------------------------- 3. aider undo
const undoButtons = await page.locator('.undo-btn').count();
check('aider: only the latest edit of a file is undoable', undoButtons === 1, undoButtons + ' undo buttons');
await page.click('.undo-btn');
await page.waitForSelector('.note');
check('aider: undo appended a visible note', (await page.textContent('.note')).indexOf('Undid edit to index.html') >= 0);
check('aider: the earlier edit became undoable again', (await page.locator('.undo-btn').count()) === 1);
await page.evaluate(function () { var t = document.getElementById('thread'); t.scrollTop = t.scrollHeight; });
await shot('06-undo-edit-aider');

// ----------------------------------------------- 4. aider context sheet
await page.click('button[aria-label="Context"]');
await page.waitForSelector('.ctx-sheet');
const mapText = await page.textContent('.ctx-pre');
check('aider: repo map lists index.html with its <title> symbol', mapText.indexOf('index.html') === 0 && mapText.indexOf('<title>Tip</title>') >= 0, mapText.slice(0, 80));
await page.click('.ctx-file .mini-btn');
await page.waitForSelector('iframe.artifact-frame');
await page.click('.artifact-sheet .mini-btn >> text=Source');
const restored = await page.textContent('.artifact-pre');
check('aider: undo restored the pre-edit file (tip 18%, not 20%)', restored.indexOf('value="18"') >= 0 && restored.indexOf('value="20"') < 0);
await page.click('.artifact-sheet button[aria-label="Close"]');
await page.click('button[aria-label="Context"]');
await page.waitForSelector('.ctx-sheet');
await shot('07-context-repo-map-aider');
await page.click('.ctx-sheet button[aria-label="Close"]');
check('overlay: context sheet unmounted on close', (await page.locator('.overlay').count()) === 0);

// ------------------------------------------------ 5. deepseek event log
await page.click('button[aria-label="Event log"]');
await page.waitForSelector('.log-sheet .ev');
const kinds = await page.locator('.ev-kind').allTextContents();
check('deepseek: log shows durable events incl. tool/decision, tool/result, edit/undo',
  ['user/message', 'turn/start', 'assistant/message', 'tool/decision', 'tool/result', 'turn/end', 'edit/undo'].every(function (k) { return kinds.indexOf(k) >= 0; }), kinds.join(' '));
const seqs = await page.locator('.ev-seq').allTextContents();
check('deepseek: sequence numbers are contiguous (append-only)', seqs.length === Number(seqs[0].slice(1)), seqs[0] + ' of ' + seqs.length);
await shot('08-event-log-deepseek');
await page.click('.log-sheet button[aria-label="Close"]');

// --------------------------------------- 6. codex approval-mode menu
await page.click('button[aria-label="Approval mode"]');
await page.waitForSelector('.menu');
await shot('09-approval-mode-menu-codex');
await page.click('.menu-item >> text=Read-only');
check('codex: mode chip shows Read-only', (await page.textContent('.mode-chip')).indexOf('Read-only') >= 0);
check('overlay: menu unmounted after choosing', (await page.locator('.menu').count()) === 0);

// ------------------------------- 7. settings / model sheet + permissions
await page.click('.model-chip');
await page.waitForSelector('.sheet .field-input');
await shot('10-settings-model-sheet');
const effective = await page.textContent('.perm-row[data-tool="write_file"] .perm-eff');
check('codex+opencode: read-only sandbox denies write_file', effective.indexOf('Deny') >= 0, effective);
await page.click('.mode-seg .seg-btn >> text=Ask');
await page.locator('.perm-list').scrollIntoViewIfNeeded();
await page.evaluate(function () { var b = document.querySelector('.sheet-body'); b.scrollTop = b.scrollHeight; });
await shot('11-tool-permissions-opencode');
const editEff = await page.textContent('.perm-row[data-tool="edit_file"] .perm-eff');
check('opencode: per-tool rule overrides mode (edit_file → Allow (rule))', editEff.indexOf('Allow (rule)') >= 0, editEff);
await page.click('.sheet button[aria-label="Close"]');
check('overlay: settings unmounted and hit-testing restored', (await page.locator('.overlay').count()) === 0 &&
  (await page.evaluate(function () { var r = document.querySelector('.composer-input').getBoundingClientRect(); var el = document.elementFromPoint(r.left + 20, r.top + 10); return !!el && el.classList.contains('composer-input'); })));

// ----------------------------------------------- 8. artifact preview
await page.click('.diff-foot .mini-btn >> text=Preview');
await page.waitForSelector('iframe.artifact-frame');
const sandbox = await page.getAttribute('iframe.artifact-frame', 'sandbox');
check('safe: artifact HTML only in sandboxed iframe (no allow-same-origin)', sandbox === 'allow-scripts', sandbox);
await page.waitForTimeout(400);
await shot('12-artifact-preview');
await page.click('.artifact-sheet button[aria-label="Close"]');

// -------------------------------------------- 9. goose pause / resume
await page.click('button[aria-label="New chat"]');
await page.fill('.composer-input', 'Tell me a slow story');
await page.click('.send-btn');
await page.waitForSelector('.pause-chip');
await page.waitForTimeout(2200);
await page.click('.pause-chip');
await page.waitForSelector('.turn-status.is-paused');
const pausedText = await page.textContent('.msg-assistant .md');
check('goose: pause keeps the partial text and the turn open', pausedText.length > 20 && (await page.locator('.resume-chip').count()) === 1, pausedText.length + ' chars');
await shot('13-paused-turn-goose');
const before = requests.length;
await page.click('.resume-btn');
await page.waitForSelector('.turn-meta', { timeout: 20000 });
const resumeBody = requests[before].body;
const ut = userTexts(resumeBody);
check('goose: resume sends partial + resume note, user message not duplicated',
  ut.filter(function (t) { return t === 'Tell me a slow story'; }).length === 1 && /^Continue your previous reply/.test(ut[ut.length - 1]), ut.join(' | ').slice(0, 120));
const finalText = await page.textContent('.msg-assistant .md');
check('goose: resumed text flows on in the same reply', finalText.indexOf(pausedText.trim().slice(0, 30)) === 0 && finalText.indexOf('The end.') > 0);
await shot('14-resumed-complete-goose');

// ----------------------------------------- 10. cancel + retry (stage 2)
await page.click('button[aria-label="New chat"]');
await page.fill('.composer-input', 'Another slow story please');
await page.click('.send-btn');
await page.waitForSelector('.live.has-text');
await page.waitForTimeout(900);
await page.click('.send-btn.is-stop');
await page.waitForSelector('.retry-btn');
check('cancel: partial kept, Retry offered', (await page.textContent('.turn-status')).indexOf('partial reply kept') >= 0);
const beforeRetry = requests.length;
await page.click('.retry-btn');
await page.waitForSelector('.live.has-text');
const retryUsers = userTexts(requests[beforeRetry].body);
check('retry: user event not duplicated, cancelled attempt not in history',
  retryUsers.length === 1 && requests[beforeRetry].body.messages.filter(function (m) { return m.role === 'assistant'; }).length === 0, retryUsers.join('|'));
await page.click('.send-btn.is-stop');
await page.waitForSelector('.retry-btn');

// ---------------------------------------- 11. goose reload recovery
await page.click('button[aria-label="New chat"]');
await page.fill('.composer-input', 'One more slow story');
await page.click('.send-btn');
await page.waitForSelector('.live.has-text');
await page.waitForTimeout(2600);
await page.reload();
await page.waitForSelector('.turn-status.is-paused', { timeout: 10000 });
const recovered = await page.textContent('.msg-assistant .md');
const reason = await page.textContent('.turn-status.is-paused');
check('goose: reload mid-stream keeps the partial as a paused turn', recovered.length > 10 && reason.indexOf('page reloaded') >= 0 && (await page.locator('.resume-chip').count()) === 1, recovered.length + ' chars · ' + reason);
await shot('15-reload-recovered-goose');

// Hard kill (no pagehide ran): only the checkpoint survives. Boot must
// settle the open turn as Paused with the checkpointed text.
await page.evaluate(function () {
  var sessions = JSON.parse(localStorage.getItem('harness.sessions') || '[]');
  sessions.unshift({ id: 's-crash', title: 'Crash test', updated_at: Date.now(), events: [
    { seq: 1, at: Date.now(), kind: 'user/message', id: 'u-crash', text: 'Write a haiku about rain' },
    { seq: 2, at: Date.now(), kind: 'turn/start', turn: 't-crash', model: 'x-ai/grok-4.7', mode: 'ask' },
  ] });
  localStorage.setItem('harness.sessions', JSON.stringify(sessions));
  localStorage.setItem('harness.live', JSON.stringify({ session: 's-crash', turn: 't-crash', step: 1, text: 'Soft rain on the roof,' }));
});
await page.reload();
await page.waitForSelector('.turn-status.is-paused', { timeout: 10000 });
check('goose: hard-kill recovery restores checkpointed text as Paused',
  (await page.textContent('.msg-assistant .md')).indexOf('Soft rain on the roof,') >= 0 && (await page.textContent('.banner-note')).indexOf('reload') >= 0);
await page.click('.banner-note .mini-btn');

// ---------------------------------------------- 12. markdown answer
await page.click('button[aria-label="New chat"]');
await page.fill('.composer-input', 'Show me what markdown looks like');
await page.click('.send-btn');
await page.waitForSelector('.turn-meta', { timeout: 15000 });
check('safe: raw <img onerror> rendered as text, no img element', (await page.locator('.md img').count()) === 0 && (await page.textContent('.md')).indexOf('<img src=x onerror=alert(1)>') >= 0);
check('markdown: table + code card + list rendered', (await page.locator('.md table').count()) === 1 && (await page.locator('.codecard').count()) === 1 && (await page.locator('.md ul li').count()) === 3);
await shot('16-markdown-answer');

// ------------------------------------- 13. keyboard-open composer
await page.fill('.composer-input', 'Typing with the keyboard up — the composer stays pinned above it.');
await page.focus('.composer-input');
await page.setViewportSize({ width: 390, height: 508 }); // 336px keyboard
await page.waitForTimeout(350);
const kb = await page.evaluate(function () {
  var r = document.querySelector('.composer-card').getBoundingClientRect();
  return { bottom: r.bottom, vvh: getComputedStyle(document.documentElement).getPropertyValue('--vvh'), h: window.visualViewport.height };
});
check('viewport: composer above the keyboard (visualViewport-tracked)', kb.bottom <= kb.h && kb.vvh.trim() === kb.h + 'px', JSON.stringify(kb));
await page.screenshot({ path: join(SHOTS, '17-keyboard-open-raw.png') });
await page.setViewportSize({ width: 390, height: 844 });

check('no page errors', pageErrors.length === 0, pageErrors.join(' | '));

await browser.close();
server.close();

// Compose the keyboard shot to full 390×844 with a labelled keyboard area.
try {
  execFileSync('python3', [join(ROOT, 'tests', 'e2e', 'keyboard_composite.py'), join(SHOTS, '17-keyboard-open-raw.png'), join(SHOTS, '17-keyboard-open-composer.png')]);
} catch (e) {
  console.log('note: keyboard composite skipped (' + e.message.split('\n')[0] + ')');
}

console.log(failures === 0 ? 'e2e: PASS' : 'e2e: ' + failures + ' FAILED');
process.exit(failures === 0 ? 0 : 1);
