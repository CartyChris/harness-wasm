// Pack checks (DESIGN.md §10, §13): size gate, node --check on every script
// block, single-file/no-CDN/no-sibling-fetch, iPhone Safari CSS gates.
// Run via `scripts/check-ship.sh` (or `node --test tests/pack.test.mjs`).

import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync, writeFileSync, mkdtempSync } from 'node:fs';
import { execFileSync } from 'node:child_process';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = dirname(dirname(fileURLToPath(import.meta.url)));
const DIST = join(ROOT, 'dist', 'harness.html');
const SIZE_LIMIT = 2.5 * 1024 * 1024;

const html = readFileSync(DIST, 'utf8');

function scriptBlocks(source) {
  const blocks = [];
  const re = /<script>([\s\S]*?)<\/script>/g;
  let match;
  while ((match = re.exec(source)) !== null) {
    blocks.push(match[1]);
  }
  return blocks;
}

test('dist/harness.html exists and is under the 2.5 MB size gate', () => {
  const size = Buffer.byteLength(html);
  assert.ok(size > 0, 'dist file is not empty');
  assert.ok(size <= SIZE_LIMIT, `size ${size} > ${SIZE_LIMIT}`);
});

test('every <script> block passes node --check', () => {
  const blocks = scriptBlocks(html);
  assert.ok(blocks.length >= 2, 'expected glue + loader script blocks');
  const dir = mkdtempSync(join(tmpdir(), 'harness-ship-'));
  blocks.forEach((block, index) => {
    const file = join(dir, `block-${index}.js`);
    writeFileSync(file, block);
    execFileSync(process.execPath, ['--check', file]);
  });
});

test('WASM is inlined — the loader never fetches a sibling asset', () => {
  const blocks = scriptBlocks(html);
  // The loader is the last block (the glue may contain dormant default-fetch
  // code paths that we never trigger by passing bytes explicitly).
  const loader = blocks[blocks.length - 1];
  assert.ok(!loader.includes('fetch('), 'loader must not fetch');
  assert.ok(!/\.wasm['"]/.test(loader), 'loader must not reference .wasm files');
  assert.ok(/'[A-Za-z0-9+/=]{1000,}'/.test(loader), 'no embedded base64 payload found');
  // The embedded payload declares its encoding.
  assert.match(loader, /var ENCODING = '(gzip|raw)';/, 'loader encoding marker missing');
});

test('single file: no external references on first paint', () => {
  assert.ok(!html.includes('<link'), 'no <link> elements allowed');
  assert.ok(!/<script[^>]+src=/.test(html), 'no external script src');
  assert.ok(!/<img[^>]+src=/.test(html), 'no external images');
  assert.ok(!html.includes('@import'), 'no CSS @import');
  assert.ok(!/https?:\/\/[^"'\\\s)]+\.(woff2?|ttf|otf)/.test(html), 'no CDN font URLs');
});

test('visible shell before boot, init-error region, noscript', () => {
  assert.ok(html.includes('id="shell"'), 'boot shell present');
  assert.ok(html.includes('boot-loading'), 'visible loading indicator');
  assert.ok(html.includes('id="init-error"'), 'init-error region present');
  assert.ok(html.includes('<noscript>'), 'noscript fallback present');
  assert.ok(html.indexOf('id="shell"') < html.indexOf('<script>'), 'shell renders before scripts');
});

// iPhone Safari gates (DESIGN.md §7) that are statically checkable.
test('CSS gates: 16px inputs, 44px targets, dvh+vh, safe-area, viewport var', () => {
  assert.match(html, /font-size:\s*16px/, 'inputs are 16px (no iOS zoom)');
  assert.match(html, /min-height:\s*44px/, 'tap targets at least 44px');
  assert.match(html, /height:\s*100vh/, '100vh fallback present');
  assert.match(html, /height:\s*100dvh/, '100dvh present');
  assert.match(html, /--vvh/, 'visualViewport-driven height var present');
  assert.match(html, /safe-area-inset-top/, 'safe-area top');
  assert.match(html, /safe-area-inset-bottom/, 'safe-area bottom');
  assert.match(html, /safe-area-inset-left/, 'safe-area left');
  assert.match(html, /safe-area-inset-right/, 'safe-area right');
  assert.match(html, /@media \(hover: hover\)/, 'hover styles are additive only');
});
