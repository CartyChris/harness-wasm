# Harness

A mobile-first agent console shipped as **one HTML file**. The client is Rust
(Leptos CSR) compiled to `wasm32-unknown-unknown`; OpenRouter is reached only
through a Rust backend proxy, so the API key never enters the client, the
repo, or logs.

## Layout

```
api/chat.rs         POST /api/chat   — SSE proxy (Vercel Rust runtime, Fluid)
api/models.rs       GET  /api/models — model list relay
api/health.rs       GET  /api/health — liveness, no secrets
crates/proxy        shared proxy handlers + loopback dev server
crates/client       Leptos CSR app (its own workspace; wasm only)
crates/core         pure logic: model policy, SSE parsing, types
scripts/pack-single-html.mjs  → dist/harness.html (ship artifact)
scripts/check-ship.sh         size + node --check + CSS gates + secret grep
scripts/secret-grep.sh        must be clean (0 hits) before anything ships
tests/              proxy integration tests + pack checks
dist/harness.html   the packed single-file client (built, never hand-edited)
```

## Toolchain

- rustc ≥ 1.88 (Leptos 0.8 MSRV), `wasm32-unknown-unknown` target
- `wasm-bindgen-cli` matching the client's `wasm-bindgen` version (0.2.130)
- `wasm-opt` (binaryen) for the `-Oz` pass
- Node ≥ 20 for the packer and pack checks
- Trunk is dev-only (`trunk serve` in `crates/client`); the ship path is the
  packer, never Trunk.

## Env

Copy `.env.example` to `.env` (local) or set the same keys in Vercel project
env. Values are secrets; `.env` is gitignored.

| Key | Required | Where |
|---|---|---|
| `OPENROUTER_API_KEY` | yes | server env only — never in the client |
| `HARNESS_ACCESS_TOKEN` | yes for any non-loopback deploy | bearer gate |

A deploy without `HARNESS_ACCESS_TOKEN` fails closed: every `/api/chat` and
`/api/models` request returns 401. The loopback dev server may run without it
only because it binds `127.0.0.1`.

## Run (local dev)

```sh
# proxy on loopback (binds 127.0.0.1:8788 only)
cp .env.example .env   # fill values
cargo run -p harness-proxy --bin dev-server

# in the app: open Settings (model chip), set Proxy URL
# http://127.0.0.1:8788 and the access token, save.
```

`HARNESS_DEV_PORT` overrides the port.

## Pack (the ship path)

```sh
node scripts/pack-single-html.mjs            # → dist/harness.html (~0.3 MB)
node scripts/pack-single-html.mjs --raw      # no DecompressionStream needed
```

The packer builds the release wasm, runs `wasm-bindgen --target no-modules`
plus `wasm-opt -Oz`, then inlines gzip+base64 WASM, the glue (classic script),
and the CSS into one HTML file. No sibling `.wasm`/`.js` fetch: it boots from
a static host, `file://`, and Textastic preview. Gate: ≤ 2.5 MB.

## Test / verify

```sh
scripts/check-ship.sh   # cargo test --workspace + pack checks + secret grep
```

Covers (DESIGN.md §10/§13): exact-origin host scope, 401 bearer gate (and
fail-closed when unset), redaction of key-shaped values, 413 body cap,
incremental SSE (frame-timing based), pack size, `node --check` on every
embedded script block, single-file/no-CDN, and the iPhone CSS gates.

## Deploy (Vercel)

Official Rust runtime (`vercel_runtime` 2.x with the axum feature) on Fluid.
Each `api/*.rs` is a `[[bin]]` in the root `Cargo.toml`; `vercel.json` sets
`maxDuration` (chat: 300s — the honest Hobby-plan cap; a turn that would
exceed it ends as an interrupted turn, never a silent truncation).

```sh
vercel deploy
```

Serving: `dist/harness.html` is a static file — host it anywhere (Vercel
static, or any static host) and point its Connect card at the proxy URL. With
Vercel serving both, leave the proxy URL blank (same origin).

## Models

The model is locked, never auto-picked from a live list. Fresh profiles
default to `x-ai/grok-4.7`. Exactly the old stock default
`openai/gpt-4o-mini` migrates once to `x-ai/grok-4.7`; any model you chose
yourself is kept verbatim. A failed/stale list never overwrites the lock.

## Security notes

- Upstream origin is exactly `https://openrouter.ai` (no client URL input, no
  subdomains, no http, no ports).
- Request bodies over 1 MiB → 413 before any upstream call; bodies containing
  OpenRouter key shapes → 400.
- Upstream errors map to short public strings; keys/tokens are redacted from
  logs and never echoed.
- Model output renders as escaped text only; artifact HTML runs in a
  sandboxed iframe (`sandbox="allow-scripts"`, no `allow-same-origin`).
- CORS is `*` with `Authorization`/`Content-Type` allowed (bearer-token auth,
  no cookies) so `Origin: null` (Textastic/`file://`) works.

## Scope

Stage 3 (Nov 2026): agent harness. Event log, tool loop with approval modes and per-tool rules, phone-local workspace with diffs and undo, pause/resume and reload recovery, context and event-log sheets. Tests: `cargo test --workspace`, `node tests/e2e/run.mjs` (mock proxy, 390x844 shots), `node tests/e2e/storage-compat.mjs` (real Stage 2 ⇄ Stage 3 storage), `node tests/e2e/live.mjs` (opt-in live turn through the dev-server). See /workspace/wo/harness-wasm/STAGE3-RETURN.md.


This is the Stage 2 first pass: shell, sessions, streaming thread with
Cancel/Retry, Connect card, locked model + migration, safe rendering, basic
artifact sheet, and the full proxy. Tool loops, richer artifacts, and the
Claude.ai-bar polish come next (Stage 3, King).
