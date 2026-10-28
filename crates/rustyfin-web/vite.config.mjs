import { defineConfig } from 'vite';
import tailwindcss from '@tailwindcss/vite';
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, copyFileSync, mkdirSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Compute content-based cache version hash
function computeCacheVersion() {
  const files = [
    join(__dirname, 'src', 'app.js'),
    join(__dirname, 'src', 'api.js'),
    join(__dirname, 'src', 'input.css'),
    join(__dirname, 'src', 'index.html'),
    join(__dirname, 'src', 'assets', 'logo.png'),
    join(__dirname, 'node_modules', 'hls.js', 'dist', 'hls.min.js'),
  ];
  const hash = createHash('md5');
  for (const f of files) {
    try {
      hash.update(readFileSync(f));
    } catch {
      // skip missing files
    }
  }
  return hash.digest('hex').substring(0, 8);
}

// Custom plugin: copy static assets and replace __CACHE_VERSION__ placeholders
function cacheVersionPlugin() {
  const cacheVersion = computeCacheVersion();
  console.log(`[vite:cache-version] Cache version: ${cacheVersion}`);

  return {
    name: 'vite-cache-version',
    closeBundle() {
      const distDir = join(__dirname, 'dist');
      const assetsDir = join(distDir, 'assets');
      mkdirSync(assetsDir, { recursive: true });

      // Copy hls.min.js locally
      copyFileSync(
        join(__dirname, 'node_modules', 'hls.js', 'dist', 'hls.min.js'),
        join(assetsDir, 'hls.min.js')
      );

      // Copy logo.png
      copyFileSync(
        join(__dirname, 'src', 'assets', 'logo.png'),
        join(assetsDir, 'logo.png')
      );

      // Copy global scripts (non-module, loaded via plain <script> tags)
      copyFileSync(
        join(__dirname, 'src', 'api.js'),
        join(assetsDir, 'api.js')
      );
      copyFileSync(
        join(__dirname, 'src', 'app.js'),
        join(assetsDir, 'app.js')
      );

      // Process sw.js: replace __CACHE_VERSION__
      const swSrc = join(__dirname, 'src', 'sw.js');
      const swContent = readFileSync(swSrc, 'utf8').replace(/__CACHE_VERSION__/g, cacheVersion);
      writeFileSync(join(distDir, 'sw.js'), swContent);

      // Process index.html: replace __CACHE_VERSION__ and fix non-module script paths
      const htmlPath = join(distDir, 'index.html');
      let htmlContent = readFileSync(htmlPath, 'utf8');
      htmlContent = htmlContent
        .replace(/__CACHE_VERSION__/g, cacheVersion)
        // Vite keeps relative paths for non-module scripts; fix them to absolute
        .replace(/src="\.\/api\.js"/g, 'src="/web/assets/api.js"')
        .replace(/src="\.\/app\.js"/g, 'src="/web/assets/app.js"');
      writeFileSync(htmlPath, htmlContent);

      console.log(`[vite:cache-version] Done — version ${cacheVersion}`);
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
        entryFileNames: 'assets/[name].js',
        chunkFileNames: 'assets/[name].js',
        assetFileNames: 'assets/[name][extname]',
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
