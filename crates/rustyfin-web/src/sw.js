// Rustyfin Service Worker: cache-first for content-hashed assets and for tagged images.
// Everything else (index.html, sw.js, logo.png, API, untagged images) goes to the network.
const CACHE_NAME = 'rustyfin-v__CACHE_VERSION__';
const HASHED_ASSET = /^\/web\/assets\/.+-[A-Za-z0-9_-]{8}\.[a-z0-9]+$/;
// Tagged images the server marks immutable name one version of a picture, so they never change.
// Their cache outlives app updates and belongs to one signed-in user: the page names the user
// (an 'image-owner' message), and a different user or a sign-out empties it. It holds at most
// IMAGE_CACHE_MAX pictures; the first ones stored are dropped first.
const IMAGE_CACHE = 'rustyfin-images-v1';
const IMAGE_PATH = /^\/Items\/[^/]+\/Images\/[^/]+(\/\d+)?$/;
const IMAGE_CACHE_MAX = 3000;
// The cache's owner is kept in the cache itself, so a worker restarted after idling knows it
const OWNER_KEY = '/__rustyfin-image-owner';
const TRIM_EVERY = 50;
let imageOwner; // undefined until read from the cache; null when nobody is signed in
let imagePuts = 0;

self.addEventListener('install', () => {
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((key) => key !== CACHE_NAME && key !== IMAGE_CACHE)
            .map((key) => caches.delete(key))
        )
      )
      .then(() => caches.open(IMAGE_CACHE))
      .then(trimImages)
  );
  self.clients.claim();
});

self.addEventListener('message', (event) => {
  const data = event.data || {};
  if (data.type === 'image-owner') event.waitUntil(setImageOwner(data.userId || null));
});

// The user the image cache belongs to, read once per worker lifetime
async function currentOwner() {
  if (imageOwner === undefined) {
    const marker = await (await caches.open(IMAGE_CACHE)).match(OWNER_KEY);
    imageOwner = marker ? await marker.text() : null;
  }
  return imageOwner;
}

// A new owner (or none) empties the cache, so no picture outlives the session it was shown in
async function setImageOwner(userId) {
  if ((await currentOwner()) === userId) return;
  imageOwner = userId;
  await caches.delete(IMAGE_CACHE);
  if (userId) await (await caches.open(IMAGE_CACHE)).put(OWNER_KEY, new Response(userId));
}

// The cache key of a tagged image request, or null. The key leaves out api_key, so a new token
// for the same user reuses the pictures already on the device.
function imageKey(url) {
  if (url.origin !== self.location.origin || !IMAGE_PATH.test(url.pathname)) return null;
  if (!url.searchParams.get('tag')) return null;
  const key = new URL(url);
  key.searchParams.delete('api_key');
  key.searchParams.sort();
  return key.href;
}

// Only a whole, same-origin answer the server marked immutable (its tag is the current one)
function storable(response) {
  return (
    response.status === 200 &&
    response.type === 'basic' &&
    /\bimmutable\b/.test(response.headers.get('Cache-Control') || '')
  );
}

// Drops the first-stored pictures beyond IMAGE_CACHE_MAX (the owner marker is kept)
async function trimImages(cache) {
  const keys = (await cache.keys()).filter((req) => new URL(req.url).pathname !== OWNER_KEY);
  for (const key of keys.slice(0, Math.max(0, keys.length - IMAGE_CACHE_MAX))) await cache.delete(key);
}

// Serves a tagged image from the cache, else from the network, storing it while it is shown. The
// request keeps its signal, so a picture the page stops loading is aborted here too. Nothing is
// served from or stored in the cache while nobody is signed in, or when the owner changed meanwhile.
async function cachedImage(event, key) {
  const owner = await currentOwner();
  const cache = owner ? await caches.open(IMAGE_CACHE) : null;
  const cached = cache && (await cache.match(key));
  if (cached) return cached;
  const response = await fetch(event.request);
  if (cache && storable(response)) {
    const copy = response.clone();
    event.waitUntil(
      (async () => {
        if ((await currentOwner()) !== owner) return;
        await cache.put(key, copy);
        // The first store of each worker lifetime trims too, as an idle worker loses its count
        if (++imagePuts % TRIM_EVERY === 1) await trimImages(cache);
      })()
    );
  }
  return response;
}

self.addEventListener('fetch', (event) => {
  const { request } = event;
  if (request.method !== 'GET') return;
  const url = new URL(request.url);
  const key = imageKey(url);
  if (key) {
    event.respondWith(cachedImage(event, key));
    return;
  }
  if (!HASHED_ASSET.test(url.pathname)) return;

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
