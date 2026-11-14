// Rustyfin Service Worker: cache-first for content-hashed assets only.
// Everything else (index.html, sw.js, logo.png, API) goes to the network.
const CACHE_NAME = 'rustyfin-v__CACHE_VERSION__';
const HASHED_ASSET = /^\/web\/assets\/.+-[A-Za-z0-9_-]{8}\.[a-z0-9]+$/;

self.addEventListener('install', () => {
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
  if (request.method !== 'GET') return;
  if (!HASHED_ASSET.test(new URL(request.url).pathname)) return;

  event.respondWith(
    caches.open(CACHE_NAME).then((cache) =>
      cache.match(request).then((cached) => {
        if (cached) return cached;
        return fetch(request).then((response) => {
          if (response.ok) cache.put(request, response.clone());
          return response;
        });
      })
    )
  );
});
