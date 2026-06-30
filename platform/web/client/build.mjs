/**
 * esbuild build script for the rustyboy web-client TypeScript frontend.
 *
 * Three entry points, three different output formats:
 *   app.ts            → static/app.js            (ESM; wasm import kept as external URL)
 *   menu.ts           → static/menu.js            (IIFE; sets window.MenuRenderer)
 *   audio-processor.ts→ static/audio-processor.js (IIFE; AudioWorklet classic script)
 *
 * Run:  node build.mjs
 */

import * as esbuild from 'esbuild';
import { fileURLToPath } from 'url';
import path from 'path';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const srcDir = path.join(__dirname, 'src-ts');
const outDir = path.join(__dirname, 'static');

// Version tag used in the wasm-pack output filename.
// Must match the ?v= query string on the <script> import in index.html and in app.ts.
const WASM_VERSION     = 'gemini-title-bg-crisp-csp-20260627';
const WASM_RUNTIME_URL = `/static/rustyboy_web_client.js?v=${WASM_VERSION}`;

/**
 * esbuild plugin: rewrites the `rustyboy-wasm` module alias to the runtime URL
 * and marks it external so the bundler leaves the import statement intact.
 * TypeScript type-checks against ./static/rustyboy_web_client.d.ts via tsconfig paths.
 */
const wasmClientPlugin = {
  name: 'rustyboy-wasm-client',
  setup(build) {
    build.onResolve({ filter: /^rustyboy-wasm$/ }, () => ({
      path: WASM_RUNTIME_URL,
      external: true,
    }));
  },
};

const sharedOptions = {
  bundle: true,
  target: ['es2020'],
  sourcemap: false,
  logLevel: 'info',
};

await Promise.all([
  // ── app.ts → ESM ──────────────────────────────────────────────────────────
  // Loaded by index.html as <script type="module">. The wasm import becomes a
  // bare ES module import that the browser resolves at runtime.
  esbuild.build({
    ...sharedOptions,
    entryPoints: [path.join(srcDir, 'app.ts')],
    format: 'esm',
    outfile: path.join(outDir, 'app.js'),
    plugins: [wasmClientPlugin],
  }),

  // ── menu.ts → IIFE ────────────────────────────────────────────────────────
  // Loaded by index.html as a plain <script>. The IIFE sets window.MenuRenderer
  // as a side-effect so app.js can access it after the script tag runs.
  esbuild.build({
    ...sharedOptions,
    entryPoints: [path.join(srcDir, 'menu.ts')],
    format: 'iife',
    outfile: path.join(outDir, 'menu.js'),
  }),

  // ── audio-processor.ts → IIFE ─────────────────────────────────────────────
  // Loaded by AudioContext.audioWorklet.addModule('/static/audio-processor.js').
  // Must be a self-contained classic script; the IIFE ensures registerProcessor
  // is called as a side-effect without leaking class names to global scope.
  esbuild.build({
    ...sharedOptions,
    entryPoints: [path.join(srcDir, 'audio-processor.ts')],
    format: 'iife',
    outfile: path.join(outDir, 'audio-processor.js'),
  }),
]);
