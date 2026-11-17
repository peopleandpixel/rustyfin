// Phones: the navigation drawer (modules/drawer.js) and the desktop sidebar's breakpoint.
// Below Tailwind's lg (64rem) the desktop sidebar must stay hidden even after sign-in shows it,
// and the hamburger opens a slide-in drawer (a class, not an inline transform, since Tailwind v4's
// translate utilities set the `translate` property). The drawer closes on a tap on its overlay,
// on any item in it, on Escape and on navigation, as Jellyfin 12.4's drawer does on phones.
// Usage: node r3mobile_drawer.mjs (run by api_tests.rs r3mobile_drawer).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = p => fs.readFileSync(new URL(p, src), 'utf8');
const html = read('index.html');
const css = read('input.css').replace(/\/\*[\s\S]*?\*\//g, '');
const router = read('modules/router.js');

// --- A minimal page: elements by id with classes, attributes and click listeners ---
function element(id, classes = []) {
    const cls = new Set(classes);
    const attrs = {};
    const listeners = {};
    return {
        id,
        innerHTML: '',
        classList: {
            add: (...n) => n.forEach(c => cls.add(c)),
            remove: (...n) => n.forEach(c => cls.delete(c)),
            contains: n => cls.has(n),
            toggle: (n, on) => ((on ?? !cls.has(n)) ? cls.add(n) : cls.delete(n)),
        },
        setAttribute: (k, v) => { attrs[k] = String(v); },
        getAttribute: k => (k in attrs ? attrs[k] : null),
        addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
        fire: (type, ev) => (listeners[type] || []).forEach(fn => fn(ev)),
    };
}
const els = {
    sidebar: element('sidebar', ['jf-drawer', 'jf-sidebar-desktop']),
    'sidebar-mobile': element('sidebar-mobile', ['jf-drawer', 'jf-drawer-mobile', 'hidden']),
    'sidebar-mobile-content': element('sidebar-mobile-content'),
    'sidebar-overlay': element('sidebar-overlay', ['hidden']),
    'sidebar-btn': element('sidebar-btn'),
};
els.sidebar.innerHTML = '<nav><a class="jf-drawer-item">Home</a></nav>';
const docListeners = {};
globalThis.document = {
    getElementById: id => els[id] || null,
    addEventListener: (type, fn) => (docListeners[type] ||= []).push(fn),
};
// A fake clock: frames and timers run only when the test advances time
let now = 0;
let timers = [];
globalThis.setTimeout = (fn, ms = 0) => { timers.push({ at: now + ms, fn }); return timers.length; };
globalThis.clearTimeout = () => {};
globalThis.requestAnimationFrame = fn => globalThis.setTimeout(fn, 0);
function advance(ms) {
    now += ms;
    for (;;) {
        const due = timers.filter(t => t.at <= now).sort((a, b) => a.at - b.at);
        if (!due.length) break;
        timers = timers.filter(t => !due.includes(t));
        due.forEach(t => t.fn());
    }
}

const drawer = await import(new URL('modules/drawer.js', src));
const m = els['sidebar-mobile'], o = els['sidebar-overlay'], btn = els['sidebar-btn'];
const isOpen = () => m.classList.contains('is-open');

test('the desktop sidebar is shown by CSS at lg only, not by the hidden class', () => {
    const tag = html.match(/<aside id="sidebar" class="([^"]*)"/);
    assert.ok(tag, '#sidebar exists');
    const classes = tag[1].split(/\s+/);
    assert.ok(classes.includes('jf-sidebar-desktop'), 'jf-sidebar-desktop class');
    assert.ok(!classes.includes('hidden') && !classes.includes('lg:block'), 'no hidden / lg:block, which show() would undo');
    assert.match(css, /\.jf-sidebar-desktop\s*\{[^}]*display:\s*none/);
    assert.match(css, /@media\s*\(width\s*>=\s*64rem\)\s*\{\s*\.jf-sidebar-desktop\s*\{[^}]*display:\s*block/);
});

test('the mobile drawer slides by a class, not a Tailwind translate utility', () => {
    const tag = html.match(/<aside id="sidebar-mobile" class="([^"]*)"/);
    assert.ok(tag, '#sidebar-mobile exists');
    assert.ok(tag[1].split(/\s+/).includes('jf-drawer-mobile'));
    assert.doesNotMatch(tag[1], /translate-x|transition-transform/);
    assert.match(css, /\.jf-drawer-mobile\s*\{[^}]*transform:\s*translateX\(-100%\)/);
    assert.match(css, /\.jf-drawer-mobile\.is-open\s*\{[^}]*transform:\s*none/);
    assert.match(css, /\.jf-drawer-mobile\s*\{[^}]*safe-area-inset-bottom/);
});

test('router uses the drawer module and closes it on navigation', () => {
    assert.match(router, /from '\.\/drawer\.js'/);
    assert.doesNotMatch(router, /function toggleSidebar/);
    const nav = router.slice(router.indexOf('function navigate('));
    assert.match(nav.slice(0, nav.indexOf('\n}\n')), /closeSidebar\(\)/);
});

test('the header brand word and version badge hide on phones only', () => {
    // input.css has an unlayered `.hidden { display: none !important }`, so `hidden sm:inline` would hide
    // them at every width; a max-sm variant hides them below 640 px only
    const badge = html.match(/<span id="version-badge" class="([^"]*)"/)[1].split(/\s+/);
    const brand = html.match(/<span class="([^"]*)">Rustyfin<\/span>/)[1].split(/\s+/);
    for (const classes of [badge, brand]) {
        assert.ok(classes.includes('max-sm:hidden'), classes.join(' '));
        assert.ok(!classes.includes('hidden'), 'no plain hidden class');
    }
});

test('viewport covers the notch area', () => {
    assert.match(html, /<meta name="viewport" content="[^"]*viewport-fit=cover/);
});

test('the hamburger opens the drawer with the sidebar links, and toggles it shut', () => {
    assert.equal(drawer.sidebarOpen(), false);
    drawer.toggleSidebar();
    assert.ok(!m.classList.contains('hidden') && !o.classList.contains('hidden'));
    advance(0);
    assert.ok(isOpen());
    assert.equal(drawer.sidebarOpen(), true);
    assert.equal(btn.getAttribute('aria-expanded'), 'true');
    assert.match(els['sidebar-mobile-content'].innerHTML, /jf-drawer-item/);
    drawer.toggleSidebar();
    assert.ok(!isOpen() && o.classList.contains('hidden'));
    assert.equal(btn.getAttribute('aria-expanded'), 'false');
    advance(200);
    assert.ok(m.classList.contains('hidden'), 'hidden once the slide-out ends');
});

test('reopening during the slide-out keeps the drawer shown', () => {
    drawer.openSidebar(); advance(0);
    drawer.closeSidebar(); advance(50);
    drawer.openSidebar(); advance(300);
    assert.ok(isOpen() && !m.classList.contains('hidden'));
    drawer.closeSidebar(); advance(200);
});

test('Escape closes it; other keys do not', () => {
    drawer.openSidebar(); advance(0);
    (docListeners.keydown || []).forEach(fn => fn({ key: 'a' }));
    assert.ok(isOpen());
    (docListeners.keydown || []).forEach(fn => fn({ key: 'Escape' }));
    assert.ok(!isOpen());
    advance(200);
});

test('a tap on an item in the drawer closes it; a tap on empty space does not', () => {
    drawer.openSidebar(); advance(0);
    const target = sel => ({ closest: s => (s.split(',').some(x => sel.includes(x.trim())) ? {} : null) });
    m.fire('click', { target: target([]) });
    assert.ok(isOpen(), 'empty space');
    m.fire('click', { target: target(['.jf-drawer-item']) });
    assert.ok(!isOpen(), 'drawer item');
    advance(200);
});

test('closing a closed drawer changes nothing', () => {
    drawer.closeSidebar();
    assert.ok(m.classList.contains('hidden') && o.classList.contains('hidden'));
});
