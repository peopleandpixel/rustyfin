// In-app navigation draws the page once. navigate() sets location.hash, and the hashchange that follows
// must not draw the same page again. Back/Forward and a typed hash still draw it (once), and so do the
// first load and a re-click of the current page.
// Runs modules/router.js's source in a vm sandbox: its imports become counting stubs, and location.hash
// fires hashchange asynchronously as a browser does.
// Usage: node r3mobile_navigate_once.mjs (run by api_tests.rs r3mobile_navigate_once).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const router = fs.readFileSync(new URL('modules/router.js', src), 'utf8')
    .replace(/^import[\s\S]*?from\s+'[^']+';$/gm, '')
    .replace(/^export\s*\{[^}]*\};?$/gm, '');

const renders = [];
const listeners = {};
let hash = '';
const fireHashchange = () => setTimeout(() => (listeners.hashchange || []).forEach(fn => fn({ type: 'hashchange' })), 0);
const location = {
    get hash() { return hash ? '#' + hash : ''; },
    set hash(v) {
        const next = String(v).replace(/^#/, '');
        if (next === hash) return;
        hash = next;
        fireHashchange();
    },
};
// The user goes Back or types a hash: the URL changes, then hashchange fires
const browse = to => { hash = to; fireHashchange(); };
const settle = () => new Promise(r => setTimeout(r, 5));

const counting = name => (...args) => { renders.push([name, ...args]); };
const pageRenderers = {
    renderHome: counting('home'), renderFavorites: counting('favorites'), renderLogin: counting('login'),
    renderLibrary: counting('library'), renderDetail: counting('detail'), renderSearch: counting('search'),
    renderPerson: counting('person'), renderSettings: counting('settings'), renderAdmin: counting('admin'),
};
const noop = () => {};
const el = () => ({ classList: { add: noop, remove: noop, toggle: noop, contains: () => false }, addEventListener: noop });
const known = {
    ...pageRenderers,
    window: null,
    location,
    history: { replaceState: (_s, _t, url) => { hash = String(url).replace(/^#/, ''); } },
    document: { querySelectorAll: () => [], addEventListener: noop, getElementById: () => null },
    sessionStorage: { setItem: noop, getItem: () => null },
    $: () => el(),
    API: { isLoggedIn: () => false },
    console,
    setTimeout,
    URLSearchParams,
};
const win = {
    location,
    scrollTo: noop,
    addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
};
known.window = win;
known.addEventListener = win.addEventListener;
// Anything else the module uses (its other imports and globals) is a do-nothing function
const scope = new Proxy(known, {
    // `undefined` and the sandbox's own `__out` stay outside the proxy
    has: (_t, k) => typeof k === 'string' && k !== 'undefined' && !k.startsWith('__'),
    get: (t, k) => (k in t ? t[k] : (k === Symbol.unscopables ? undefined : noop)),
    set: (t, k, v) => { t[k] = v; return true; },
});
const __out = {};
const ctx = vm.createContext({ scope, __out });
vm.runInContext(`with (scope) { ${router}\n __out.navigate = navigate; __out.handleHash = handleHash; }`, ctx);
const { navigate, handleHash } = __out;

test('a navigation from the app draws the page once', async () => {
    handleHash(); // first load, no hash: home
    await settle();
    assert.deepEqual(renders.map(r => r[0]), ['home']);
    renders.length = 0;
    navigate('library', 'lib1');
    await settle();
    assert.deepEqual(renders, [['library', 'lib1']]);
    renders.length = 0;
    navigate('detail', 'item1');
    navigate('detail', 'item2'); // two in a row before either hashchange arrives
    await settle();
    assert.deepEqual(renders, [['detail', 'item1'], ['detail', 'item2']]);
});

test('re-clicking the current page draws it again, once', async () => {
    renders.length = 0;
    navigate('detail', 'item2');
    await settle();
    assert.deepEqual(renders, [['detail', 'item2']]);
});

test('Back, Forward and a typed hash draw the page once', async () => {
    renders.length = 0;
    browse('library/lib1'); // Back
    await settle();
    assert.deepEqual(renders, [['library', 'lib1']]);
    renders.length = 0;
    browse('detail/item2'); // Forward to the page navigate() had set before
    await settle();
    assert.deepEqual(renders, [['detail', 'item2']]);
    renders.length = 0;
    browse('/search/abc'); // typed in another form: made canonical in place, drawn once
    await settle();
    assert.deepEqual(renders, [['search', 'abc']]);
    assert.equal(location.hash, '#search/abc');
});
