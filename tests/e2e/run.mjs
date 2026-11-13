// End-to-end UI checks + 390×844 screenshots for the folded-repo features.
// Drives the packed dist/harness.html in headless Chrome against the loopback
// mock proxy (tests/e2e/mock-proxy.mjs). Exits non-zero on any failed check.
//
//   PW_CORE=<path to playwright-core> SHOTS=<dir> node tests/e2e/run.mjs
//
// Every check is named after the repo pattern it proves.

import { startMock } from './mock-proxy.mjs';
import { createRequire } from 'node:module';
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
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
await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: 'http://127.0.0.1:' + PORT });
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
const chips = await page.evaluate(function () {
  return { ctx: document.querySelector('button[aria-label="Context"]').textContent, log: document.querySelector('button[aria-label="Event log"]').textContent, ctxTitle: document.querySelector('button[aria-label="Context"]').title };
});
check('P2.6 chips carry a visible meaning (tok / events) + stable aria-label', /\d+(\.\dk)? tok$/.test(chips.ctx.trim()) && /^\d+ events?$/.test(chips.log.trim()) && /tokens/.test(chips.ctxTitle), JSON.stringify(chips));
const hint = await page.evaluate(function () {
  return { coarse: matchMedia('(pointer: coarse)').matches, kbd: getComputedStyle(document.querySelector('.kbd-hint')).display, foot: document.querySelector('.composer-foot').innerText };
});
check('P2.5 ⌘⏎ hint hidden on touch (pointer: coarse), mode line kept', hint.coarse && hint.kbd === 'none' && hint.foot.indexOf('⌘') < 0 && /approval/.test(hint.foot), JSON.stringify(hint));
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
const pre = await page.evaluate(function () {
  var d = document.querySelector('.approve .diff');
  return d ? { head: d.querySelector('.diff-head').innerText, body: d.querySelector('.diff-body').innerText, rows: d.querySelectorAll('.dl').length, more: !!d.querySelector('.show-all'), undo: !!d.querySelector('.undo-btn'), applied: document.querySelectorAll('.diff:not(.is-proposed)').length } : null;
});
check('P1.1 write_file diff visible BEFORE Allow (proposed, collapsed, no Undo)',
  !!pre && /will create/i.test(pre.head) && pre.body.indexOf('<!DOCTYPE html>') >= 0 && pre.body.indexOf('<title>Tip</title>') >= 0 && pre.more && !pre.undo && pre.applied === 0, pre && (pre.head.replace(/\s+/g, ' ') + ' · rows ' + pre.rows));
await page.click('.approve .show-all');
const allRows = await page.locator('.approve .diff .dl').count();
check('P1.1 "Show all" expands the proposed diff to the whole new file', allRows >= 23, allRows + ' rows');
await page.evaluate(function () { var c = document.querySelector('.approve'); c.scrollIntoView({ block: 'end' }); });
await shot('03b-approval-with-diff');

await page.click('.approve-btn.yes');
await page.waitForSelector('.diff', { timeout: 10000 });
await page.waitForSelector('.act[data-status="ask"]', { timeout: 10000 });
const editPre = await page.evaluate(function () {
  var d = document.querySelector('.approve .diff');
  return d ? { head: d.querySelector('.diff-head').innerText, add: [].map.call(d.querySelectorAll('.dl.add'), function (r) { return r.innerText; }).join('\n'), del: [].map.call(d.querySelectorAll('.dl.del'), function (r) { return r.innerText; }).join('\n') } : null;
});
check('P1.1 edit_file SEARCH/REPLACE shown as a line diff BEFORE Allow', !!editPre && /will edit/i.test(editPre.head) && editPre.del.indexOf('value="18"') >= 0 && editPre.add.indexOf('value="20"') >= 0, editPre && editPre.head.replace(/\s+/g, ' '));
await page.evaluate(function () { var c = document.querySelector('.approve'); c.scrollIntoView({ block: 'end' }); });
await shot('03c-approval-edit-diff');
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
const zero = await page.evaluate(function () {
  var created = [].find.call(document.querySelectorAll('.diff'), function (d) { return /^created$/i.test(d.querySelector('.diff-verb').innerText); });
  var del = created.querySelector('.diff-stat .del');
  return { text: del.innerText, zero: del.classList.contains('is-zero'), color: getComputedStyle(del).color, danger: getComputedStyle(document.documentElement).getPropertyValue('--danger').trim() };
});
check('P3.9 zero count "−0" renders muted, not red', zero.zero && /0/.test(zero.text) && zero.color !== 'rgb(255, 107, 107)' && zero.color.indexOf('255, 1') < 0, JSON.stringify(zero));
await page.click('.copy-btn');
await page.waitForSelector('.toast');
const clip1 = await page.evaluate(function () { return navigator.clipboard.readText(); });
check('P2.4 Copy on the reply footer copies the reply text (no tool JSON)', clip1.indexOf("I'll build it as one self-contained file") === 0 && clip1.indexOf('Done. **index.html**') > 0 && clip1.indexOf('"content"') < 0, clip1.slice(0, 60));
check('P2.4 "Copied" toast shown', (await page.textContent('.toast')).trim() === 'Copied');
await shot('04b-copy-toast');
await page.waitForSelector('.toast', { state: 'detached', timeout: 4000 });
check('P2.4 toast clears itself', (await page.locator('.toast').count()) === 0);

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
check('P2.7 access token masked by default (type=password, 16px)', (await page.getAttribute('#access-token', 'type')) === 'password' &&
  (await page.evaluate(function () { return getComputedStyle(document.getElementById('access-token')).fontSize; })) === '16px');
await page.click('.pw-toggle');
const shown = await page.getAttribute('#access-token', 'type');
const toggleLabel = await page.getAttribute('.pw-toggle', 'aria-label');
await page.click('.pw-toggle');
check('P2.7 Show/Hide toggle reveals and re-masks', shown === 'text' && toggleLabel === 'Hide access token' && (await page.getAttribute('#access-token', 'type')) === 'password');
await shot('10-settings-model-sheet');
// P1.3 export: proxy URL + token stripped
await page.locator('.export-btn').scrollIntoViewIfNeeded();
const [download] = await Promise.all([page.waitForEvent('download'), page.click('.export-btn')]);
const exportPath = join(SHOTS, 'export-sample.json');
await download.saveAs(exportPath);
const exported = readFileSync(exportPath, 'utf8');
const exportedJson = JSON.parse(exported);
check('P1.3 export has sessions + events, no access token, no proxy URL',
  exportedJson.format === 'harness-chats' && exportedJson.sessions.length >= 1 && exportedJson.sessions[0].events.length > 3 &&
  exported.indexOf('demo-token') < 0 && exported.indexOf('127.0.0.1:' + PORT) < 0 && exported.indexOf('"token"') < 0 && exported.indexOf('base_url') < 0,
  exportedJson.sessions.length + ' sessions, ' + exported.length + ' bytes');
await page.waitForSelector('.data-status');
// P1.3 import of a poisoned file is refused, nothing written
const sessionsBefore = await page.evaluate(function () { return JSON.parse(localStorage.getItem('harness.sessions')).length; });
const poisonPath = join(SHOTS, 'poisoned-import.json');
const poisoned = JSON.parse(exported);
poisoned.sessions[0].id = 's-poison';
poisoned.sessions[0].title = 'Poisoned';
poisoned.sessions[0].events[0].text = 'my key ' + ['sk', '-or-', 'v1-', '0123456789abcdef0123456789'].join('');
writeFileSync(poisonPath, JSON.stringify(poisoned));
await page.setInputFiles('.import-btn input[type=file]', poisonPath);
await page.waitForSelector('.data-status.is-err');
const refusal = await page.textContent('.data-status');
const sessionsAfter = await page.evaluate(function () { return JSON.parse(localStorage.getItem('harness.sessions')).length; });
check('P1.3 key-shaped import refused with a plain message, nothing saved', /^Import refused: it contains an OpenRouter-style key/.test(refusal) && sessionsAfter === sessionsBefore &&
  (await page.evaluate(function () { return localStorage.getItem('harness.sessions').indexOf('Poisoned') < 0; })), refusal);
await page.evaluate(function () { var b = document.querySelector('.sheet-body'); b.scrollTop = b.scrollHeight; });
await shot('10b-export-import-refused');
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
await page.click('.full-btn');
await page.waitForTimeout(150);
const fullBox = await page.evaluate(function () { var r = document.querySelector('.artifact-sheet').getBoundingClientRect(); return { top: r.top, h: r.height, label: document.querySelector('.full-btn').getAttribute('aria-label') }; });
check('P3.10 artifact opens full screen (whole viewport) and toggles back', fullBox.top === 0 && fullBox.h === 844 && fullBox.label === 'Exit full screen', JSON.stringify(fullBox));
await shot('12b-artifact-fullscreen');
await page.click('.full-btn');
await page.click('.artifact-sheet button[aria-label="Close"]');

// P1.1 delete_file: the card names the file and its line count in red before Allow
await page.fill('.composer-input', 'Please delete index.html');
await page.click('button[aria-label="Send"]');
await page.waitForSelector('.act[data-status="ask"] .approve .diff.is-delete', { timeout: 15000 });
const delCard = await page.evaluate(function () {
  var d = document.querySelector('.act[data-status="ask"] .approve .diff.is-delete');
  var row = d.querySelector('.dl.del.whole');
  return { head: d.textContent, row: row ? row.textContent : '', color: row ? getComputedStyle(row).color : '' };
});
check('P1.1 delete_file preview shows the filename and line count in red before Allow',
  /will delete/i.test(delCard.head) && delCard.head.indexOf('index.html') >= 0 && /Deletes the whole file · 23 lines/.test(delCard.row) && /^rgb\((2[0-5]\d|1[5-9]\d),/.test(delCard.color), JSON.stringify(delCard));
await page.evaluate(function () { var c = document.querySelector('.act[data-status="ask"] .approve'); c.scrollIntoView({ block: 'end' }); });
await shot('03d-approval-delete-preview');
await page.click('.act[data-status="ask"] .approve-btn.no');
await page.waitForFunction(function () { return !document.querySelector('button[aria-label="Stop"]'); }, null, { timeout: 15000 });
check('P1.1 denied delete leaves the file in the workspace', (await page.locator('.diff-foot .mini-btn >> text=Preview').count()) >= 1);

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
check('P3.8 exactly one Resume (composer chip), inline line is status text only', (await page.locator('button:has-text("Resume")').count()) === 1 && (await page.locator('.resume-btn').count()) === 0);
await shot('13-paused-turn-goose');
const before = requests.length;
await page.click('.resume-chip');
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
await shot('20-cancel-stopped-retry');
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
await page.click('.code-copy');
await page.waitForTimeout(150);
const clip2 = await page.evaluate(function () { return navigator.clipboard.readText(); });
check('P2.4 Copy on a code block copies exactly the code', clip2 === 'fn main() {\n    println!("hello from the harness");\n}', JSON.stringify(clip2));
const usersBeforeRegen = await page.locator('.msg-user').count();
const reqBeforeRegen = requests.length;
check('P2.4 Regenerate offered on the latest Done turn', (await page.locator('.regen-btn').count()) === 1);
await page.click('.regen-btn');
await page.waitForSelector('.turn-meta', { timeout: 15000 });
await page.waitForTimeout(200);
const regenBody = requests[reqBeforeRegen].body;
check('P2.4 regenerate: no duplicate user event, old reply not in history, one reply shown',
  (await page.locator('.msg-user').count()) === usersBeforeRegen && userTexts(regenBody).length === 1 &&
  regenBody.messages.filter(function (m) { return m.role === 'assistant'; }).length === 0 && (await page.locator('.msg-assistant').count()) === 1,
  userTexts(regenBody).join('|'));
await page.click('button[aria-label="Event log"]');
await page.waitForSelector('.log-sheet .ev');
const regenKinds = await page.locator('.ev-kind').allTextContents();
check('P2.4 regenerate keeps exactly one user/message event in the log', regenKinds.filter(function (k) { return k === 'user/message'; }).length === 1 && regenKinds.filter(function (k) { return k === 'turn/start'; }).length === 2, regenKinds.join(' '));
await page.click('.log-sheet button[aria-label="Close"]');

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

// ------------------------------------------ 14. sessions drawer
await page.click('button[aria-label="Sessions"]');
await page.waitForSelector('.drawer .session-row');
const drawerRows = await page.locator('.drawer .session-row').count();
check('drawer: lists every chat', drawerRows >= 6, drawerRows + ' rows');
await shot('19-drawer-sessions');
await page.click('.drawer .session-row >> nth=1 >> .session-open');
check('drawer: switching session closes the drawer', (await page.locator('.drawer').count()) === 0);

// ---------------- 15. P1.2 IndexedDB: ~1 MB workspace × 3 sessions, no quota
await page.click('button[aria-label="Approval mode"]');
await page.click('.menu-item >> text=Auto');
for (const tag of ['A', 'B', 'C']) {
  await page.click('button[aria-label="New chat"]');
  await page.fill('.composer-input', 'Write bulk files ' + tag);
  await page.click('.send-btn');
  await page.waitForSelector('.turn-meta', { timeout: 60000 });
}
await page.waitForTimeout(800);
const store = await page.evaluate(async function () {
  const db = await new Promise(function (res, rej) { const r = indexedDB.open('harness'); r.onsuccess = function () { res(r.result); }; r.onerror = rej; });
  const rows = await new Promise(function (res) { const t = db.transaction('kv').objectStore('kv'); const out = []; const c = t.openCursor(); c.onsuccess = function () { const cur = c.result; if (!cur) return res(out); out.push([cur.key, cur.value.length]); cur.continue(); }; });
  let ls = 0;
  for (let i = 0; i < localStorage.length; i++) { const k = localStorage.key(i); ls += k.length + localStorage.getItem(k).length; }
  return { rows: rows, idbKeys: rows.length, idbChars: rows.reduce(function (a, r) { return a + r[1]; }, 0), bulkSessions: rows.filter(function (r) { return r[1] > 900000; }).length, migrated: rows.some(function (r) { return r[0] === 'meta:migrated'; }), lsChars: ls, warn: !!document.querySelector('.banner-warn') };
});
console.log('STORAGE ' + JSON.stringify(store));
check('P1.2 three ~1 MB sessions land in IndexedDB with no quota error', store.bulkSessions === 3 && store.migrated && !store.warn, JSON.stringify(store));
check('P1.2 localStorage keeps only settings + the text mirror (< 100 KB)', store.lsChars < 100000, store.lsChars + ' chars');
await page.reload();
await page.waitForSelector('.composer-input');
await page.click('button[aria-label="Context"]');
await page.waitForSelector('.ctx-sheet');
const ctxFiles = await page.locator('.ctx-file').count();
check('P1.2 after reload the bulk workspace is restored from IndexedDB (5 files)', ctxFiles === 5, ctxFiles + ' files');
await page.click('.ctx-sheet button[aria-label="Close"]');
await page.click('button[aria-label="Approval mode"]');
await page.click('.menu-item >> text=Ask');

// ------------------------------- 16. error states: 401, offline, no proxy
await page.click('button[aria-label="New chat"]');
await page.click('.model-chip');
await page.fill('#access-token', 'wrong-token');
await page.click('.sheet-foot .primary-btn');
await page.fill('.composer-input', 'Hello?');
await page.click('.send-btn');
await page.waitForSelector('.turn-status.is-err', { timeout: 10000 });
const unauth = await page.textContent('.turn-status.is-err');
// 401 opens the Connect sheet on purpose (Stage 2 behaviour).
check('401: wrong token → plain "Connect required" + Retry, Connect sheet opened', /Connect required/.test(unauth) && (await page.locator('.retry-btn').count()) === 1 && (await page.locator('#access-token').count()) === 1, unauth);
await shot('22b-unauthorized-connect-sheet');
await page.click('.sheet button[aria-label="Close"]');
await shot('22-unauthorized-401');
await page.click('.model-chip');
await page.fill('#access-token', 'demo-token');
await page.click('.sheet-foot .primary-btn');

await context.setOffline(true);
await page.click('button[aria-label="New chat"]');
await page.fill('.composer-input', 'Are you there?');
await page.click('.send-btn');
await page.waitForSelector('.turn-status.is-err', { timeout: 10000 });
const offline = await page.textContent('.turn-status.is-err');
check('offline: plain "Proxy unreachable" + Retry', /unreachable|offline/i.test(offline) && (await page.locator('.retry-btn').count()) === 1, offline);
await shot('23-offline');
await context.setOffline(false);

check('no page errors', pageErrors.length === 0, pageErrors.join(' | '));

// Fresh profile: no proxy configured → connect banner; then re-import.
const fresh = await browser.newContext({ viewport: { width: 390, height: 844 }, deviceScaleFactor: 2, isMobile: true, hasTouch: true });
const fp = await fresh.newPage();
const freshErrors = [];
fp.on('pageerror', function (e) { freshErrors.push(e.message); });
await fp.goto('http://127.0.0.1:' + PORT + '/');
await fp.waitForSelector('.composer-input');
check('no-proxy: fresh profile shows the connect banner', (await fp.locator('.banner-connect').count()) === 1);
await fp.screenshot({ path: join(SHOTS, '21-no-proxy.png') });
await fp.click('.banner-connect');
await fp.waitForSelector('.import-btn');
await fp.setInputFiles('.import-btn input[type=file]', exportPath);
await fp.waitForSelector('.data-status');
const importMsg = await fp.textContent('.data-status');
await fp.click('.sheet button[aria-label="Close"]');
await fp.click('button[aria-label="Sessions"]');
await fp.waitForSelector('.drawer .session-row');
await fp.waitForTimeout(400);
const freshTitles = await fp.locator('.drawer .session-title').allTextContents();
const exportedTitles = exportedJson.sessions.map(function (x) { return x.title; });
check('P1.3 re-import into a fresh profile restores the same sessions', /^Imported \d+ chats? \(0 already here\)/.test(importMsg) &&
  exportedTitles.length === freshTitles.length && exportedTitles.every(function (t) { return freshTitles.indexOf(t) >= 0; }), importMsg + ' · ' + freshTitles.join(' | '));
await fp.screenshot({ path: join(SHOTS, '24-imported-fresh-profile.png') });
check('no page errors (fresh profile)', freshErrors.length === 0, freshErrors.join(' | '));
await fresh.close();

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
