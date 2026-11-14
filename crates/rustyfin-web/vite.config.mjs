import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, copyFileSync, mkdirSync, readdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

const sha8 = (buf) => createHash('sha256').update(buf).digest('hex').substring(0, 8);

// Post-build step: hash-named global scripts, rewritten HTML, cache version
function cacheVersionPlugin() {
  return {
    name: 'vite-cache-version',
    closeBundle() {
      const distDir = join(__dirname, 'dist');
      const assetsDir = join(distDir, 'assets');
      mkdirSync(assetsDir, { recursive: true });

      // Global (non-module) scripts: copy as <stem>-<hash8>.js
      const scripts = {
        'api.js': join(__dirname, 'src', 'api.js'),
        'app.js': join(__dirname, 'src', 'app.js'),
        'hls.min.js': join(__dirname, 'node_modules', 'hls.js', 'dist', 'hls.min.js'),
      };
      const hashed = {};
      for (const [name, src] of Object.entries(scripts)) {
        const buf = readFileSync(src);
        const out = `${name.replace(/\.js$/, '')}-${sha8(buf)}.js`;
        writeFileSync(join(assetsDir, out), buf);
        hashed[name] = out;
      }

      // logo.png stays unhashed
      copyFileSync(
        join(__dirname, 'src', 'assets', 'logo.png'),
        join(assetsDir, 'logo.png')
      );

      // Rewrite index.html to the hashed script names
      const htmlPath = join(distDir, 'index.html');
      let html = readFileSync(htmlPath, 'utf8')
        .replace('src="./api.js"', `src="/web/assets/${hashed['api.js']}"`)
        .replace('src="./app.js"', `src="/web/assets/${hashed['app.js']}"`)
        .replace('src="/web/assets/hls.min.js"', `src="/web/assets/${hashed['hls.min.js']}"`);

      // Cache version: hash over the final asset names, so it changes whenever any asset does
      const names = readdirSync(assetsDir).sort().join('\n');
      const cacheVersion = sha8(names);
      console.log(`[vite:cache-version] Cache version: ${cacheVersion}`);

      writeFileSync(
        join(distDir, 'sw.js'),
        readFileSync(join(__dirname, 'src', 'sw.js'), 'utf8').replace(/__CACHE_VERSION__/g, cacheVersion)
      );
      html = html.replace(/__CACHE_VERSION__/g, cacheVersion);
      writeFileSync(htmlPath, html);
    },
  };
}

export default defineConfig({
  plugins: [tailwindcss(), cacheVersionPlugin()],
  root: './src',
  base: '/web/',
  build: {
    outDir: '../dist',
    emptyOutDir: true,
    assetsDir: 'assets',
    rollupOptions: {
      input: join(__dirname, 'src', 'index.html'),
      output: {
        entryFileNames: 'assets/[name]-[hash].js',
        chunkFileNames: 'assets/[name]-[hash].js',
        assetFileNames: 'assets/[name]-[hash][extname]',
      },
    },
  },
  server: {
    port: 5173,
    fs: {
      allow: [__dirname],
    },
  },
});
