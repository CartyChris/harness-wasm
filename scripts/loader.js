'use strict';
// Harness single-file loader (packed into dist/harness.html by
// scripts/pack-single-html.mjs). Classic script, no template literals, no
// sibling .wasm/.js fetch: the WASM bytes are embedded as base64 (gzip when
// DecompressionStream is available). A failed instantiate shows an error in
// the boot shell instead of a blank page.
(function () {
  var ENCODING = '__WASM_ENC__';
  var BASE64 = '__WASM_B64__';
  var EXPECTED_LENGTH = __WASM_LEN__;

  function showInitError(detail) {
    var loading = document.querySelector('.boot-loading');
    if (loading && loading.parentNode) {
      loading.parentNode.removeChild(loading);
    }
    var glyph = document.querySelector('.boot-glyph');
    if (glyph) {
      glyph.style.display = 'none';
    }
    var region = document.getElementById('init-error');
    if (!region) {
      return;
    }
    var title = document.createElement('div');
    title.className = 'boot-error-title';
    title.textContent = 'Harness could not start.';
    var body = document.createElement('div');
    body.textContent = detail;
    region.appendChild(title);
    region.appendChild(body);
    region.classList.add('is-visible');
  }

  function decodeBase64(text) {
    if (typeof atob !== 'function') {
      throw new Error('This browser has no atob; it cannot boot Harness.');
    }
    var binary = atob(text);
    var bytes = new Uint8Array(binary.length);
    for (var i = 0; i < binary.length; i++) {
      bytes[i] = binary.charCodeAt(i);
    }
    return bytes;
  }

  function gunzip(bytes) {
    if (typeof DecompressionStream !== 'function') {
      throw new Error(
        'This build is gzip-packed and needs DecompressionStream (iOS 16.4+). ' +
          'Repack with scripts/pack-single-html.mjs --raw for older devices.'
      );
    }
    var stream = new Blob([bytes]).stream();
    var piped = stream.pipeThrough(new DecompressionStream('gzip'));
    return new Response(piped).arrayBuffer().then(function (buffer) {
      return new Uint8Array(buffer);
    });
  }

  function instantiate(bytes) {
    if (typeof wasm_bindgen !== 'function') {
      throw new Error('The wasm-bindgen glue did not load.');
    }
    if (EXPECTED_LENGTH > 0 && bytes.length !== EXPECTED_LENGTH) {
      throw new Error(
        'Embedded WASM is damaged: expected ' + EXPECTED_LENGTH +
          ' bytes but decoded ' + bytes.length + '.'
      );
    }
    return wasm_bindgen({ module_or_path: bytes }).then(function () {
      if (typeof wasm_bindgen.run_app !== 'function') {
        throw new Error('The WASM module has no run_app export.');
      }
      wasm_bindgen.run_app();
    });
  }

  function boot() {
    var promise;
    try {
      var bytes = decodeBase64(BASE64);
      promise = ENCODING === 'gzip' ? gunzip(bytes) : Promise.resolve(bytes);
    } catch (error) {
      showInitError(error && error.message ? error.message : String(error));
      return;
    }
    promise.then(instantiate).then(null, function (error) {
      showInitError(error && error.message ? error.message : String(error));
    });
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', boot);
  } else {
    boot();
  }
})();
