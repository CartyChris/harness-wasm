#!/usr/bin/env node
// Pack the ship artifact: dist/harness.html — one file, WASM inlined, no
// sibling asset fetch (DESIGN.md §2 "Pack").
//
// Pipeline: cargo build (release, wasm32) → wasm-bindgen --target no-modules
// → wasm-opt -Oz → gzip+base64 → dist/harness.html.
//
// Flags:
//   --raw        embed raw base64 instead of gzip (older WebViews without
//                DecompressionStream; bigger file)
//   --skip-build reuse the existing build/bindgen output in target/pack

import { execFileSync } from 'node:child_process';
import { gzipSync } from 'node:zlib';
import { mkdirSync, readFileSync, rmSync, writeFileSync, existsSync, renameSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const SCRIPTS = dirname(fileURLToPath(import.meta.url));
const ROOT = dirname(SCRIPTS);
const CLIENT = join(ROOT, 'crates', 'client');
const PACK_DIR = join(CLIENT, 'target', 'pack');
const DIST = join(ROOT, 'dist');
const SIZE_LIMIT = 2.5 * 1024 * 1024; // §2 size gate

const args = process.argv.slice(2);
const RAW = args.includes('--raw');
const SKIP_BUILD = args.includes('--skip-build');

function resolveBinary(name) {
  // Prefer $CARGO_HOME/bin (rustup toolchains), then well-known locations,
  // then $PATH. The system rustc can be older than Leptos' MSRV.
  const candidates = [
    process.env.CARGO && name === 'cargo' ? process.env.CARGO : null,
    join(process.env.HOME || '', '.cargo', 'bin', name),
    join('/usr', 'local', 'bin', name),
    name,
  ].filter(Boolean);
  for (const candidate of candidates) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }
  return name;
}

function run(command, argv, cwd) {
  const bin = resolveBinary(command);
  console.log('$ ' + bin + ' ' + argv.join(' '));
  execFileSync(bin, argv, { cwd, stdio: 'inherit' });
}

// 1. Build the client (wasm32, release, size profile).
if (!SKIP_BUILD) {
  run('cargo', ['build', '--release', '--target', 'wasm32-unknown-unknown', '--lib'], CLIENT);
  rmSync(PACK_DIR, { recursive: true, force: true });
  mkdirSync(PACK_DIR, { recursive: true });
  const rawWasm = join(CLIENT, 'target', 'wasm32-unknown-unknown', 'release', 'harness_client.wasm');
  // 2. wasm-bindgen with no-modules: classic script, boots under file:// and
  //    Origin: null (Textastic WKWebView).
  run('wasm-bindgen', ['--target', 'no-modules', '--out-dir', 'target/pack', rawWasm], CLIENT);
  // 3. wasm-opt -Oz on the bindgen output (bulk memory is used by the glue).
  run('wasm-opt', ['-Oz', '--enable-bulk-memory-opt',
    join('target', 'pack', 'harness_client_bg.wasm'),
    '-o', join('target', 'pack', 'harness_client_bg.opt.wasm')], CLIENT);
  renameSync(join(PACK_DIR, 'harness_client_bg.opt.wasm'), join(PACK_DIR, 'harness_client_bg.wasm'));
}

const gluePath = join(PACK_DIR, 'harness_client.js');
const wasmPath = join(PACK_DIR, 'harness_client_bg.wasm');
for (const required of [gluePath, wasmPath]) {
  if (!existsSync(required)) {
    console.error('missing ' + required + ' — run without --skip-build');
    process.exit(1);
  }
}
const glue = readFileSync(gluePath, 'utf8');
const wasm = readFileSync(wasmPath);

if (glue.includes('</script')) {
  console.error('glue contains </script — cannot inline');
  process.exit(1);
}

// 4. Embed the WASM.
const payload = RAW ? wasm : gzipSync(wasm, { level: 9 });
const base64 = payload.toString('base64');
const encoding = RAW ? 'raw' : 'gzip';
console.log(
  `wasm ${wasm.length} bytes → ${encoding}+base64 ${base64.length} chars`
);

// 5. Loader + CSS + shell.
const loader = readFileSync(join(SCRIPTS, 'loader.js'), 'utf8')
  .replace('__WASM_ENC__', encoding)
  .replace('__WASM_B64__', base64)
  .replace('__WASM_LEN__', String(wasm.length));
const css = readFileSync(join(CLIENT, 'style.css'), 'utf8');

const html = [
  '<!DOCTYPE html>',
  '<html lang="en">',
  '<head>',
  '<meta charset="utf-8">',
  '<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">',
  '<meta name="theme-color" content="#0b0d10">',
  '<title>Harness</title>',
  '<style>',
  css,
  '</style>',
  '</head>',
  '<body>',
  // Visible shell before WASM boots; failed instantiate is never a blank page.
  '<div id="shell">',
  '  <div class="boot-topbar">',
  '    <span class="boot-mark" aria-hidden="true">H</span>',
  '    <span class="boot-title">Harness</span>',
  '  </div>',
  '  <div class="boot-body">',
  '    <div class="boot-glyph" aria-hidden="true">H</div>',
  '    <div class="boot-loading">Loading <span class="boot-dots"><span></span><span></span><span></span></span></div>',
  '  </div>',
  '  <noscript><p>Harness needs JavaScript enabled — the whole app is WebAssembly.</p></noscript>',
  '</div>',
  // Outside #shell: the app removes #shell on boot, but the init-error
  // region must survive to report failures after that point too.
  '<div id="init-error" role="alert"></div>',
  '<div id="app"></div>',
  '<script>',
  glue,
  '</script>',
  '<script>',
  loader,
  '</script>',
  '</body>',
  '</html>',
  '',
].join('\n');

mkdirSync(DIST, { recursive: true });
const outPath = join(DIST, 'harness.html');
writeFileSync(outPath, html);

const size = Buffer.byteLength(html);
const mb = (size / (1024 * 1024)).toFixed(2);
console.log(`wrote ${outPath} (${size} bytes, ${mb} MB)`);
if (size > SIZE_LIMIT) {
  console.error(`SIZE GATE FAILED: ${mb} MB > 2.5 MB (DESIGN.md §2)`);
  process.exit(1);
}
