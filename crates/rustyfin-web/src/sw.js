// Rustyfin Service Worker — Cache-First für statische Assets
// Vite generates: index.css (bundled styles), hls.min.js, logo.png
// api.js & app.js are loaded with cache-busting query params, not pre-cached
// index.html is NOT cached — always fetched from server to get latest script references
const CACHE_NAME = 'rustyfin-v__CACHE_VERSION__';
const ASSETS = [
  '/web/assets/index.css',
  '/web/assets/hls.min.js',
  '/web/assets/logo.png'
];

self.addEventListener('install', (event) => {
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) => cache.addAll(ASSETS))
  );
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches.keys().then((keys) =>
      Promise.all(
        keys.filter((key) => key !== CACHE_NAME).map((key) => caches.delete(key))
      )
    )
  );
  self.clients.claim();
});

self.addEventListener('fetch', (event) => {
  const { request } = event;

  // Cache-First für statische Assets (aber nicht für index.html, api.js/app.js mit Cache-Busting)
  if (request.url.startsWith('/web/') && !request.url.includes('/api/') && !request.url.includes('?v=') && !request.url.endsWith('/index.html')) {
    event.respondWith(
      caches.match(request).then((cached) => {
        const fetchPromise = fetch(request).then((response) => {
          if (response.ok) {
            const clone = response.clone();
            caches.open(CACHE_NAME).then((cache) => cache.put(request, clone));
          }
          return response;
        });
        return cached || fetchPromise;
      })
    );
    return;
  }

  // Network-First für API-Calls (mit Cache-Fallback)
  if (request.url.includes('/api/')) {
    event.respondWith(
      fetch(request).then((response) => {
        if (response.ok) {
          const clone = response.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(request, clone));
        }
        return response;
      }).catch(() => caches.match(request))
    );
    return;
  }

  // Standard: Network only
  event.respondWith(fetch(request));
});
