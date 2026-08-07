import { defineConfig } from 'vite';
import { resolve } from 'path';
import { fileURLToPath } from 'url';
import ViteMinifyPlugin from 'vite-plugin-html-minifier-terser';

const __dirname = fileURLToPath(new URL('.', import.meta.url));

// Mirrors the zline_sso frontend build:
//   - The loose frontend/ directory stays the single source of truth and is
//     served from disk in debug builds (see src/statics.rs).
//   - `vite build` minifies everything into ../static (HTML via
//     vite-plugin-html-minifier-terser, JS/CSS via esbuild), which release
//     builds embed into the binary with include_str!.
// `config.js` is intentionally NOT bundled: it is served dynamically by the
// backend (api::config_js) and is referenced as a plain classic script.
export default defineConfig({
  base: './',
  plugins: [
    ViteMinifyPlugin({
      removeComments: true,
      collapseWhitespace: true,
      minifyJS: true,
      minifyCSS: true,
    }),
  ],
  build: {
    outDir: '../static',
    emptyOutDir: true,
    minify: 'esbuild',
    rollupOptions: {
      input: {
        main: resolve(__dirname, 'index.html'),
      },
      output: {
        // Keep a stable, predictable path for the bundled CSS so src/statics.rs
        // can include_str!("../static/css/main.css") instead of a hashed name.
        assetFileNames: 'css/[name][extname]',
      },
    },
  },
});
