// Loopback mock of the Harness proxy for UI tests (tests/e2e/run.mjs).
// Speaks the same wire format as the real proxy (/api/chat SSE in the
// OpenRouter shape, /api/models, /api/health) with scripted, slowed-down
// scenarios so mid-stream states can be screenshotted. No network, no keys.
//
// Scenarios (picked from the last user message / tool results in the body):
//   "tip calculator"  → step 1: text + write_file(index.html)
//                       step 2: edit_file(index.html)   step 3: summary
//   "slow story"      → ~12 s of slow tokens (pause / cancel / reload tests)
//   resume note       → continuation text
//   "bulk files"      → 5 steps, each write_file of ~190 KB (storage quota test)
//   "delete index"    → delete_file(index.html) (delete preview on the card)
//   anything else     → a Markdown-rich answer (list, table, code)
// Auth: like the real proxy, /api/chat answers 401 unless the bearer is
// "demo-token" (the e2e profile's token).

import { createServer } from 'node:http';
import { readFileSync } from 'node:fs';

const RESUME_NOTE = 'Continue your previous reply exactly where it stopped.';

export const TIP_HTML = [
  '<!DOCTYPE html>',
  '<html lang="en">',
  '<head>',
  '<meta name="viewport" content="width=device-width, initial-scale=1">',
  '<title>Tip</title>',
  '<style>',
  'body { font: 17px -apple-system, sans-serif; margin: 0; padding: 24px; background: #faf7f2; color: #222; }',
  'label { display: block; margin: 14px 0 6px; font-weight: 600; }',
  'input { font-size: 16px; width: 100%; padding: 10px; border: 1px solid #ccc; border-radius: 10px; }',
  '.out { margin-top: 20px; font-size: 28px; font-weight: 700; }',
  '</style>',
  '</head>',
  '<body>',
  '<h1>Tip calculator</h1>',
  '<label>Bill <input id="bill" type="number" value="48"></label>',
  '<label>Tip % <input id="pct" type="number" value="18"></label>',
  '<div class="out" id="out"></div>',
  '<script>',
  'function calc() { var b = +bill.value, p = +pct.value; out.textContent = "Tip $" + (b * p / 100).toFixed(2); }',
  'bill.oninput = pct.oninput = calc; calc();',
  '</script>',
  '</body>',
  '</html>',
  '',
].join('\n');

function chunk(delta, finish) {
  return 'data: ' + JSON.stringify({ choices: [{ delta: delta, finish_reason: finish || null }] }) + '\n\n';
}

function words(text) {
  return text.match(/\S+\s*|\s+/g) || [];
}

function toolCallFrames(id, name, argsObj, step) {
  const args = JSON.stringify(argsObj);
  const size = step || 60;
  const frames = [chunk({ tool_calls: [{ index: 0, id: id, type: 'function', function: { name: name, arguments: '' } }] })];
  for (let i = 0; i < args.length; i += size) {
    frames.push(chunk({ tool_calls: [{ index: 0, function: { arguments: args.slice(i, i + size) } }] }));
  }
  frames.push(chunk({}, 'tool_calls'));
  return frames;
}

const MARKDOWN_ANSWER =
  'Here is a quick tour of what renders safely:\n\n' +
  '## Highlights\n\n' +
  '- **Bold**, *italic*, and `inline code`\n' +
  '- Links like [the docs](https://example.com) open only on tap\n' +
  '- Raw HTML such as <img src=x onerror=alert(1)> stays plain text\n\n' +
  '| Mode | Reads | Writes |\n|---|---|---|\n| Read-only | run | blocked |\n| Ask | run | ask first |\n| Auto | run | run |\n\n' +
  '```rust\nfn main() {\n    println!("hello from the harness");\n}\n```\n\n' +
  'Anything else you want to try?';

const STORY =
  'Once upon a time, in a city of glass towers and slow trains, a small robot named Pim kept a garden on the roof of a library. ' +
  'Every morning Pim counted the tomatoes, watered the basil, and read one page of a book left open on the windowsill. ' +
  'The page changed every night, though no one ever saw who turned it. Pim began to leave notes in the margins, small questions written in careful block letters, ' +
  'and one rainy Tuesday an answer appeared in a looping, unfamiliar hand.';

function scenario(body) {
  const msgs = Array.isArray(body.messages) ? body.messages : [];
  const users = msgs.filter(function (m) { return m.role === 'user'; });
  const lastUser = users.length ? String(users[users.length - 1].content || '') : '';
  const toolMsgs = msgs.filter(function (m) { return m.role === 'tool'; });
  // Tool results since the last user message (this turn only).
  let lastUserIdx = -1;
  msgs.forEach(function (m, i) { if (m.role === 'user') lastUserIdx = i; });
  const turnTools = msgs.slice(lastUserIdx + 1).filter(function (m) { return m.role === 'tool'; });
  const lastMsg = msgs[msgs.length - 1] || {};
  const lower = lastUser.toLowerCase();
  if (lastMsg.role === 'user' && lastUser.indexOf(RESUME_NOTE) === 0) {
    return { delay: 60, frames: words(' Pim read it twice, folded the note into a paper boat, and sailed it down the rain gutter to whoever was listening. The end.').map(function (w) { return chunk({ content: w }); }) };
  }
  if (lower.indexOf('tip calculator') >= 0) {
    if (toolMsgs.length === 0) {
      const intro = words("I'll build it as one self-contained file so you can preview it right here.").map(function (w) { return chunk({ content: w }); });
      return { delay: 70, frames: intro.concat(toolCallFrames('call_write_1', 'write_file', { path: 'index.html', content: TIP_HTML })) };
    }
    if (toolMsgs.length === 1) {
      return { delay: 40, frames: toolCallFrames('call_edit_2', 'edit_file', { path: 'index.html', search: 'value="18"', replace: 'value="20"' }) };
    }
    const summary = 'Done. **index.html** is a single-file tip calculator:\n\n- Bill and tip % inputs (16px, no iOS zoom)\n- Live total as you type\n- Default tip bumped to 20%\n\nTap **Preview** on the diff card to try it.';
    return { delay: 30, frames: words(summary).map(function (w) { return chunk({ content: w }); }) };
  }
  if (lower.indexOf('bulk files') >= 0) {
    if (toolMsgs.length < 5) {
      const n = toolMsgs.length + 1;
      const line = 'const row' + n + ' = "' + 'x'.repeat(80) + '";\n';
      const content = line.repeat(Math.floor(190000 / line.length));
      return { delay: 0, frames: toolCallFrames('call_bulk_' + n + '_' + Date.now(), 'write_file', { path: 'data/part' + n + '.js', content: content }, 20000) };
    }
    return { delay: 10, frames: words('Wrote five ~190 KB files.').map(function (w) { return chunk({ content: w }); }) };
  }
  if (lower.indexOf('delete index') >= 0 && turnTools.length === 0) {
    return { delay: 30, frames: toolCallFrames('call_del_1', 'delete_file', { path: 'index.html' }) };
  }
  if (lower.indexOf('delete index') >= 0) {
    return { delay: 20, frames: words('Okay — I left index.html in place.').map(function (w) { return chunk({ content: w }); }) };
  }
  if (lower.indexOf('slow story') >= 0) {
    return { delay: 140, frames: words(STORY).map(function (w) { return chunk({ content: w }); }) };
  }
  return { delay: 25, frames: words(MARKDOWN_ANSWER).map(function (w) { return chunk({ content: w }); }) };
}

export function startMock(port, htmlPath) {
  const requests = [];
  const server = createServer(function (req, res) {
    const cors = {
      'Access-Control-Allow-Origin': '*',
      'Access-Control-Allow-Headers': 'Authorization, Content-Type',
      'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
    };
    if (req.method === 'OPTIONS') { res.writeHead(204, cors); res.end(); return; }
    if (req.url === '/' || req.url.indexOf('/harness.html') === 0) {
      res.writeHead(200, { 'Content-Type': 'text/html; charset=utf-8' });
      res.end(readFileSync(htmlPath));
      return;
    }
    if (req.url === '/api/health') {
      res.writeHead(200, Object.assign({ 'Content-Type': 'application/json' }, cors));
      res.end('{"ok":true,"service":"harness"}');
      return;
    }
    if (req.url === '/api/models') {
      res.writeHead(200, Object.assign({ 'Content-Type': 'application/json' }, cors));
      res.end(JSON.stringify({ data: [{ id: 'x-ai/grok-4.7' }, { id: 'anthropic/claude-sonnet-4.5' }, { id: 'openai/gpt-5' }] }));
      return;
    }
    if (req.url === '/api/chat' && req.method === 'POST') {
      let raw = '';
      req.on('data', function (d) { raw += d; });
      req.on('end', function () {
        let body = {};
        try { body = JSON.parse(raw); } catch (e) { body = {}; }
        requests.push({ body: body, auth: req.headers.authorization || '' });
        if ((req.headers.authorization || '') !== 'Bearer demo-token') {
          res.writeHead(401, Object.assign({ 'Content-Type': 'application/json' }, cors));
          res.end('{"error":"missing or wrong access token"}');
          return;
        }
        const plan = scenario(body);
        res.writeHead(200, Object.assign({ 'Content-Type': 'text/event-stream; charset=utf-8', 'Cache-Control': 'no-cache' }, cors));
        let i = 0;
        let closed = false;
        res.on('close', function () { closed = true; });
        (function tick() {
          if (closed) return;
          if (i >= plan.frames.length) { res.end('data: [DONE]\n\n'); return; }
          res.write(plan.frames[i++]);
          setTimeout(tick, plan.delay);
        })();
      });
      return;
    }
    res.writeHead(404, cors);
    res.end('{"error":"not found"}');
  });
  return new Promise(function (resolve) {
    server.listen(port, '127.0.0.1', function () { resolve({ server: server, requests: requests }); });
  });
}
