// B144: Refresh metadata shows a busy button, says when it is done, and updates the open page;
// a library change updates an open detail page; the action row uses Lucide icons, not emoji.
// Runs app.js's icon, refreshItemMetadata and refreshViewAfterLibraryChange in a vm sandbox.
// Usage: node b144_refresh_metadata.mjs PATH/TO/app.js (run by api_tests.rs b144_refresh_metadata).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');

function slice(head) {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return source.slice(start, source.indexOf('\n}\n', start) + 3);
}

const ICONS = {
    RefreshCw: [['path', { d: 'M3 12a9 9 0 0 1 9-9' }], ['path', { d: 'M21 3v5h-5' }]],
    Heart: [['path', { d: 'M19 14c1.49-1.46 3-3.21 3-5.5' }]],
};

// A sandbox with the app's globals stubbed; calls are logged in order.
function sandbox({ page = 'detail', detailId = 'item-1', refresh } = {}) {
    const log = [];
    const timers = [];
    const context = vm.createContext({
        window: { lucideIcons: ICONS },
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        toast: (msg, type = 'success') => log.push(['toast', msg, type]),
        renderDetail: id => { log.push(['renderDetail', id]); return Promise.resolve(); },
        renderLibrary: id => log.push(['renderLibrary', id]),
        renderHome: () => log.push(['renderHome']),
        setTimeout: (fn, ms) => { timers.push([fn, ms]); return timers.length; },
        API: {
            refreshItem: id => { log.push(['refreshItem', id]); return refresh ? refresh() : Promise.resolve({ status: 'ok' }); },
            invalidateCache: prefix => log.push(['invalidateCache', prefix]),
        },
    });
    vm.runInContext(
        `var currentPage = ${JSON.stringify(page)}; var currentDetailId = ${JSON.stringify(detailId)};
         var currentLibraryId = 'lib-1';
         ${slice('function icon(')}
         ${slice('async function refreshItemMetadata(')}
         ${slice('function refreshViewAfterLibraryChange(')}
         this.icon = icon; this.refreshItemMetadata = refreshItemMetadata;
         this.refreshViewAfterLibraryChange = refreshViewAfterLibraryChange;
         this.go = (page, id) => { currentPage = page; currentDetailId = id; };`,
        context,
    );
    return { context, log, timers };
}

function fakeButton() {
    const attrs = new Map([['aria-label', 'detail.refresh_metadata'], ['title', 'detail.refresh_metadata']]);
    const classes = new Set(['jf-btn-ghost']);
    return {
        disabled: false,
        isConnected: true,
        dataset: { id: 'item-1' },
        attrs,
        classes,
        classList: { add: c => classes.add(c), remove: c => classes.delete(c), contains: c => classes.has(c) },
        setAttribute: (k, v) => attrs.set(k, String(v)),
        getAttribute: k => (attrs.has(k) ? attrs.get(k) : null),
        removeAttribute: k => attrs.delete(k),
    };
}

function deferred() {
    let resolve, reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
}

test('icon builds a currentColor Lucide SVG from the icon node', () => {
    const { context } = sandbox();
    const svg = context.icon('RefreshCw', { size: 18, class: 'jf-busy-spin' });
    assert.match(svg, /^<svg [^>]*xmlns="http:\/\/www\.w3\.org\/2000\/svg"/);
    assert.match(svg, /viewBox="0 0 24 24"/);
    assert.match(svg, /width="18"/);
    assert.match(svg, /height="18"/);
    assert.match(svg, /fill="none"/);
    assert.match(svg, /stroke="currentColor"/);
    assert.match(svg, /stroke-width="2"/);
    assert.match(svg, /stroke-linecap="round"/);
    assert.match(svg, /stroke-linejoin="round"/);
    assert.match(svg, /class="[^"]*jf-busy-spin[^"]*"/);
    assert.match(svg, /<path d="M3 12a9 9 0 0 1 9-9"\s*\/?>/);
    assert.match(svg, /<path d="M21 3v5h-5"\s*\/?>/);
    assert.match(svg, /<\/svg>$/);
});

test('an icon without a label is hidden from screen readers; a label is escaped', () => {
    const { context } = sandbox();
    assert.match(context.icon('Heart'), /aria-hidden="true"/);
    const labelled = context.icon('Heart', { label: 'A "b" <c>' });
    assert.match(labelled, /role="img"/);
    assert.match(labelled, /aria-label="A &quot;b&quot; &lt;c&gt;"/);
    assert.doesNotMatch(labelled, /aria-hidden/);
});

test('an unknown icon name gives an empty string', () => {
    const { context } = sandbox();
    assert.equal(context.icon('NoSuchIcon'), '');
});

test('the button is busy while the refresh runs, and a second click sends nothing', async () => {
    const d = deferred();
    const { context, log } = sandbox({ refresh: () => d.promise });
    const btn = fakeButton();
    const first = context.refreshItemMetadata(btn, 'item-1');
    assert.equal(btn.disabled, true);
    assert.equal(btn.getAttribute('aria-busy'), 'true');
    assert.ok(btn.classes.has('is-busy'));
    assert.equal(btn.getAttribute('aria-label'), 'detail.metadata_refreshing');
    await context.refreshItemMetadata(btn, 'item-1');
    assert.equal(log.filter(e => e[0] === 'refreshItem').length, 1);
    d.resolve({ status: 'ok' });
    await first;
    assert.equal(btn.disabled, false);
    assert.equal(btn.getAttribute('aria-busy'), null);
    assert.ok(!btn.classes.has('is-busy'));
    assert.equal(btn.getAttribute('aria-label'), 'detail.refresh_metadata');
});

test('when the refresh ends, the caches are dropped, the open page re-renders and a toast says so', async () => {
    const { context, log } = sandbox();
    await context.refreshItemMetadata(fakeButton(), 'item-1');
    const names = log.map(e => e[0]);
    assert.deepEqual(log.filter(e => e[0] === 'invalidateCache').map(e => e[1]).sort(), ['/Items', '/Shows']);
    assert.ok(names.lastIndexOf('invalidateCache') < names.indexOf('renderDetail'), 'caches dropped before the re-render');
    assert.deepEqual(log.find(e => e[0] === 'renderDetail'), ['renderDetail', 'item-1']);
    assert.deepEqual(log.find(e => e[0] === 'toast'), ['toast', 'detail.metadata_refreshed', 'success']);
    assert.ok(!log.some(e => e[0] === 'toast' && e[1] === 'detail.metadata_refreshing'), 'no "Refreshing…" toast after the fact');
});

test('a page the user has left is not re-rendered, but the toast still shows', async () => {
    const d = deferred();
    const { context, log } = sandbox({ refresh: () => d.promise });
    const p = context.refreshItemMetadata(fakeButton(), 'item-1');
    context.go('detail', 'item-2');
    d.resolve({ status: 'ok' });
    await p;
    assert.ok(!log.some(e => e[0] === 'renderDetail'));
    assert.ok(log.some(e => e[0] === 'invalidateCache' && e[1] === '/Items'));
    assert.deepEqual(log.find(e => e[0] === 'toast'), ['toast', 'detail.metadata_refreshed', 'success']);
});

test('a failed refresh shows an error toast with the message and restores the button', async () => {
    const { context, log } = sandbox({ refresh: () => Promise.reject(new Error('POST /Items/item-1/Refresh: 500')) });
    const btn = fakeButton();
    await context.refreshItemMetadata(btn, 'item-1');
    assert.deepEqual(log.find(e => e[0] === 'toast'),
        ['toast', 'detail.metadata_refresh_failed|{"message":"POST /Items/item-1/Refresh: 500"}', 'error']);
    assert.ok(!log.some(e => e[0] === 'renderDetail'));
    assert.equal(btn.disabled, false);
    assert.equal(btn.getAttribute('aria-busy'), null);
});

test('a refresh that times out says it continues on the server', async () => {
    const err = new Error('The operation timed out.');
    err.name = 'TimeoutError';
    const { context, log } = sandbox({ refresh: () => Promise.reject(err) });
    const btn = fakeButton();
    await context.refreshItemMetadata(btn, 'item-1');
    assert.deepEqual(log.find(e => e[0] === 'toast'), ['toast', 'detail.metadata_refresh_timeout', 'error']);
    assert.equal(btn.disabled, false);
});

test('a library change re-renders an open detail page, library page or home page from fresh data', () => {
    for (const [page, expected] of [['detail', ['renderDetail', 'item-1']], ['library', ['renderLibrary', 'lib-1']], ['home', ['renderHome']]]) {
        const { context, log, timers } = sandbox({ page });
        context.refreshViewAfterLibraryChange();
        assert.deepEqual(log.filter(e => e[0] === 'invalidateCache').map(e => e[1]).sort(), ['/Items', '/Shows'], page);
        assert.equal(timers.length, 1, page);
        timers[0][0]();
        assert.deepEqual(log.at(-1), expected, page);
    }
    const { context, log, timers } = sandbox({ page: 'admin' });
    context.refreshViewAfterLibraryChange();
    assert.equal(timers.length, 0);
    assert.ok(!log.some(e => e[0].startsWith('render')));
});

test("the detail page's action row uses Lucide icons, not emoji, at one height", () => {
    const start = source.indexOf('<!-- Action Bar -->');
    assert.ok(start >= 0, 'renderDetail has the action bar');
    const row = source.slice(start, source.indexOf('<!-- Overview section -->', start));
    for (const emoji of ['🔄', '♥', '❤']) assert.ok(!row.includes(emoji), `no ${emoji} in the action row`);
    assert.match(row, /icon\('RefreshCw'/);
    assert.match(row, /icon\('Heart'/);
    assert.match(row, /data-action="refresh-item"[^>]*aria-label="\$\{escapeHtml\(t\('detail\.refresh_metadata'\)\)\}"|aria-label="\$\{escapeHtml\(t\('detail\.refresh_metadata'\)\)\}"[^>]*data-action="refresh-item"/);
    const buttons = row.match(/<button[^>]*>/g);
    assert.ok(buttons.length >= 4);
    for (const b of buttons) assert.match(b, /\bh-11\b/, `every action button is h-11: ${b}`);
    assert.match(source, /'refresh-item': el => refreshItemMetadata\(el, el\.dataset\.id\)/);
});
