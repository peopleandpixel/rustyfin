// B152: the service worker's image cache. Runs src/sw.js in a vm sandbox against a fake Cache
// Storage and fetch, and drives it with install/activate/message/fetch events.
// Usage: node b152_service_worker.mjs PATH/TO/app.js (run by api_tests.rs b152_service_worker).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(path.join(path.dirname(process.argv[2]), 'sw.js'), 'utf8');
const ORIGIN = 'http://server.test';

class FakeCache {
    constructor() { this.entries = new Map(); }
    static key(req) { return typeof req === 'string' ? new URL(req, ORIGIN).href : req.url; }
    async match(req) { const r = this.entries.get(FakeCache.key(req)); return r ? r.clone() : undefined; }
    async put(req, res) { const k = FakeCache.key(req); this.entries.delete(k); this.entries.set(k, res); }
    async delete(req) { return this.entries.delete(FakeCache.key(req)); }
    async keys() { return [...this.entries.keys()].map(url => ({ url })); }
}
class FakeCaches {
    constructor() { this.byName = new Map(); }
    async open(name) { if (!this.byName.has(name)) this.byName.set(name, new FakeCache()); return this.byName.get(name); }
    async delete(name) { return this.byName.delete(name); }
    async keys() { return [...this.byName.keys()]; }
    images() { return this.byName.get('rustyfin-images-v1'); }
}

// A response as the network gives it; type 'basic' unless said otherwise
function answer(body, { status = 200, cacheControl = 'public, max-age=31536000, immutable', type = 'basic' } = {}) {
    const res = new Response(status === 304 ? null : body, { status, headers: { 'Cache-Control': cacheControl } });
    Object.defineProperty(res, 'type', { value: type });
    const clone = res.clone.bind(res);
    res.clone = () => { const c = clone(); Object.defineProperty(c, 'type', { value: type }); return c; };
    return res;
}

// A worker lifetime on `caches`, whose network answers with net(url)
function worker(caches, net) {
    const listeners = {};
    const fetched = [];
    const self = {
        location: { origin: ORIGIN },
        addEventListener: (type, fn) => { listeners[type] = fn; },
        skipWaiting() {},
        clients: { claim() {} },
    };
    vm.runInContext(source, vm.createContext({
        self, caches, URL, Response, console,
        fetch: async req => { fetched.push(req.url); return net(req.url); },
    }));
    async function dispatch(type, init) {
        const pending = [];
        let responded;
        const event = { ...init, waitUntil: p => pending.push(p), respondWith: p => { responded = p; } };
        listeners[type](event);
        const response = responded && (await responded);
        // waitUntil may be called after awaits, so settle until nothing new is pending
        for (let n = 0; n < pending.length; n++) await pending[n];
        return response;
    }
    return {
        fetched,
        activate: () => dispatch('activate', {}),
        owner: userId => dispatch('message', { data: { type: 'image-owner', userId } }),
        get: url => dispatch('fetch', { request: { method: 'GET', url: new URL(url, ORIGIN).href } }),
    };
}

const poster = (id, tag = 'abc', token = 't1') => `/Items/${id}/Images/Primary?maxWidth=240&tag=${tag}&api_key=${token}`;
// The worker's key for poster(id): no api_key, parameters sorted
const keyOf = id => {
    const url = new URL(poster(id), ORIGIN);
    url.searchParams.delete('api_key');
    url.searchParams.sort();
    return url.href;
};

test('a tagged immutable picture is stored once per user and served without the network', async () => {
    const caches = new FakeCaches();
    const sw = worker(caches, () => answer('jpeg'));
    await sw.owner('u1');
    assert.equal(await (await sw.get(poster(1))).text(), 'jpeg');
    // Another token for the same user is the same key
    assert.equal(await (await sw.get(poster(1, 'abc', 't2'))).text(), 'jpeg');
    assert.equal(sw.fetched.length, 1);
    const keys = [...caches.images().entries.keys()];
    assert.ok(keys.every(k => !k.includes('api_key')), keys.join());
});

test('untagged, short-lived, failed and opaque answers are not stored', async () => {
    const caches = new FakeCaches();
    const answers = {
        '/Items/1/Images/Primary': () => answer('untagged'),
        '/Items/2/Images/Primary': () => answer('stale tag', { cacheControl: 'public, max-age=300' }),
        '/Items/3/Images/Primary': () => answer('gone', { status: 404 }),
        '/Items/4/Images/Primary': () => answer('opaque', { type: 'opaque' }),
    };
    const sw = worker(caches, url => answers[new URL(url).pathname]());
    await sw.owner('u1');
    await sw.get('/Items/1/Images/Primary?maxWidth=240&api_key=t');
    for (const id of [2, 3, 4]) await sw.get(poster(id));
    assert.deepEqual([...caches.images().entries.keys()].map(k => new URL(k).pathname), ['/__rustyfin-image-owner']);
    // The untagged request is not the worker's at all; the others go to the network every time
    for (const id of [2, 3, 4]) await sw.get(poster(id));
    assert.equal(sw.fetched.length, 6);
});

test('sign-out and another user empty the cache; nothing is kept while signed out', async () => {
    const caches = new FakeCaches();
    const sw = worker(caches, () => answer('jpeg'));
    await sw.owner('u1');
    await sw.get(poster(1));
    await sw.owner('u1'); // the same user again keeps it
    assert.equal(caches.images().entries.size, 2);
    await sw.owner('u2');
    assert.equal(caches.images().entries.size, 1, 'only the new owner marker');
    await sw.get(poster(1));
    assert.equal(sw.fetched.length, 2, 'u2 does not get u1\'s copy');
    await sw.owner(null);
    assert.equal(caches.images(), undefined);
    await sw.get(poster(1));
    await sw.get(poster(1));
    assert.equal(sw.fetched.length, 4, 'signed out: always the network');
    assert.equal(caches.images()?.entries.size ?? 0, 0);
});

test('the page names the owner on sign-in and sign-out, and signed out empties the cache itself', async () => {
    const api = fs.readFileSync(path.join(path.dirname(process.argv[2]), 'api.js'), 'utf8');
    const method = name => {
        const start = api.indexOf(`\n    ${name}`);
        assert.ok(start >= 0, `api.js has ${name}`);
        return api.slice(start, api.indexOf('\n    },\n', start));
    };
    assert.match(method('logout() {'), /this\.announceImageOwner\(\)/);
    assert.match(method('_storeSession('), /this\.announceImageOwner\(\)/);
    const body = method('announceImageOwner() {');
    const posted = [];
    const deleted = [];
    const context = vm.createContext({
        navigator: { serviceWorker: { controller: { postMessage: m => posted.push(m) } } },
        globalThis: { caches: { delete: async n => { deleted.push(n); } } },
    });
    const announce = vm.runInContext(`(function ${body.trim().replace(/^announceImageOwner/, '')}\n})`, context);
    announce.call({ token: 'tok', userId: 'u1' });
    announce.call({ token: null, userId: 'u1' });
    assert.deepEqual(posted.map(m => ({ ...m })), [{ type: 'image-owner', userId: 'u1' }, { type: 'image-owner', userId: null }]);
    assert.deepEqual(deleted, ['rustyfin-images-v1']);
});

test('a restarted worker knows the owner from the cache and still trims to the cap', async () => {
    const caches = new FakeCaches();
    const first = worker(caches, () => answer('jpeg'));
    await first.owner('u1');
    const cache = caches.images();
    for (let i = 0; i < 3010; i++) await cache.put(keyOf(`old${i}`), answer('x'));

    // A new lifetime: the cached picture is served, and its first store trims
    const second = worker(caches, () => answer('jpeg'));
    assert.equal(await (await second.get(poster('old3009'))).text(), 'x');
    assert.equal(second.fetched.length, 0);
    await second.get(poster('new'));
    const keys = [...cache.entries.keys()];
    assert.equal(keys.length, 3001, 'the cap plus the owner marker');
    assert.ok(keys.some(k => k.endsWith('/__rustyfin-image-owner')));
    assert.ok(!keys.some(k => k.includes('/Items/old0/')), 'the first stored go first');
    assert.ok(keys.some(k => k.includes('/Items/new/')));

    // Activating a new version trims too
    for (let i = 0; i < 20; i++) await cache.put(keyOf(`more${i}`), answer('x'));
    await worker(caches, () => answer('jpeg')).activate();
    assert.equal(cache.entries.size, 3001);
});
