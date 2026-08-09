// Minifies the classic (non-module) <script> files with esbuild — the same
// engine Vite uses for `build.minify: 'esbuild'`.
//
// Vite cannot bundle classic scripts (they share top-level globals like `API`
// and `loadFiles` across files), so it leaves their <script> tags untouched in
// index.html. This step rewrites each file in frontend/js/ into
// ../static/js/, preserving the original names so the tags keep working.
//
// `esbuild.transform` compiles each file in isolation, so top-level
// const/function declarations stay top-level (i.e. globals) — no module
// wrapper, no `export {}`.
import { build as esbuildBuild, transform } from 'esbuild';
import { copyFile, readdir, readFile, mkdir, writeFile } from 'node:fs/promises';
import { join, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const srcDir = join(root, 'js');
const outDir = join(root, '..', 'static', 'js');

await mkdir(outDir, { recursive: true });

const files = (await readdir(srcDir)).filter((f) => f.endsWith('.js'));
if (files.length === 0) {
  throw new Error(`no .js files found under ${srcDir}`);
}

for (const f of files) {
  const code = await readFile(join(srcDir, f), 'utf8');
  const { code: min } = await transform(code, {
    loader: 'js',
    minify: true,
    // Match the browsers Vite targets; keeps optional chaining etc. working.
    target: 'es2018',
  });
  await writeFile(join(outDir, f), min);
  console.log(`minified js/${f}: ${code.length} -> ${min.length} bytes`);
}

// Build the vendored libfw SDK (a `window.LibfwClient` classic-script/IIFE
// bundle) directly from the installed npm package with esbuild, then copy the
// WASM engine beside it. The npm package ships ESM sources + a raw WASM
// binary but no prebuilt classic-script bundle, so we build it here instead
// of maintaining a hand-committed vendor copy (which drifts from the version
// actually installed). The bundle is written to BOTH ../static/vendor
// (embedded by release builds via include_str!/include_bytes!) and ./vendor
// (served from disk by debug builds), so both modes always run a bundle that
// matches the installed libfw-client.
const sdkRoot = join(root, 'node_modules', 'libfw-client');
const vendorOut = join(root, '..', 'static', 'vendor');
const devVendorOut = join(root, 'vendor');
await mkdir(vendorOut, { recursive: true });
await mkdir(devVendorOut, { recursive: true });

await esbuildBuild({
  entryPoints: [join(sdkRoot, 'index.js')],
  bundle: true,
  format: 'iife',
  globalName: 'LibfwClient',
  platform: 'browser',
  minify: true,
  // The wasm-bindgen glue only falls back to `import.meta.url` in ESM; in
  // classic-script mode the SDK resolves the sibling `.wasm` from
  // `document.currentScript.src`, so `import.meta` here is dead code.
  define: { 'import.meta.url': '{}' },
  outfile: join(vendorOut, 'libfw-client.js'),
  logLevel: 'info',
});
await copyFile(join(vendorOut, 'libfw-client.js'), join(devVendorOut, 'libfw-client.js'));
await copyFile(join(sdkRoot, 'pkg', 'libfw_client_bg.wasm'), join(vendorOut, 'libfw_client_bg.wasm'));
await copyFile(join(sdkRoot, 'pkg', 'libfw_client_bg.wasm'), join(devVendorOut, 'libfw_client_bg.wasm'));
console.log('built vendor/libfw-client.js + libfw_client_bg.wasm from node_modules/libfw-client');
