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
import { transform } from 'esbuild';
import { readdir, readFile, mkdir, writeFile } from 'node:fs/promises';
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
