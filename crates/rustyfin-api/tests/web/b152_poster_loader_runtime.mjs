// B152: the poster loader at run time. Runs app.js's posterLoader with fake cards (rects you
// move by hand), a fake IntersectionObserver and frames you run by hand.
// Usage: node b152_poster_loader_runtime.mjs PATH/TO/app.js (api_tests.rs b152_poster_loader_runtime).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
const line = start => {
    const found = source.split('\n').find(l => l.startsWith(start));
    assert.ok(found, `app.js has ${start}`);
    return found.replace(/^const /, 'var ');
};
const block = (head, end) => {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return source.slice(start, source.indexOf(end, start) + end.length).replace(/^const /, 'var ');
};
const code = [
    line('const POSTER_MAX_LOADING ='),
    'var gridDiagOnPoster = null;',
    line('const POSTER_SEEN_MAX ='),
    line('const POSTER_GUESSES_MAX ='),
    line('const POSTER_CACHED_MS ='),
    block('function posterKey(', '\n}\n'),
    line('const POSTER_WIDTHS ='),
    block('function posterWidth(', '\n}\n'),
    block('function posterWindow(', '\n}\n'),
    block('function posterPlan(', '\n}\n'),
    block('const posterLoader = {', '\n};\n'),
].join('\n');

const VH = 800;
const VW = 1280;

// A card at `top` with a picture to load; move it with card.top = ...
function makeCard(id, top, width = 160) {
    const listeners = {};
    const img = {
        dataset: { imgId: `item${id}`, imgType: 'Primary', imgTag: 'tag' },
        attrs: {},
        style: {},
        fetchPriority: 'auto',
        getAttribute(name) { return this.attrs[name] ?? null; },
        removeAttribute(name) { delete this.attrs[name]; },
        // A picture the test marks `cached` is complete as soon as its src is set, as the browser's
        // memory cache does
        complete: false,
        cached: false,
        set src(value) { this.attrs.src = value; this.complete = this.cached; },
        get src() { return this.attrs.src ?? ''; },
        addEventListener(type, fn) { (listeners[type] ??= new Set()).add(fn); },
        removeEventListener(type, fn) { listeners[type]?.delete(fn); },
        listenerCount() { return Object.values(listeners).reduce((n, s) => n + s.size, 0); },
        fire(type) { for (const fn of [...(listeners[type] || [])]) fn({ type }); },
        matches: () => true,
        closest: () => card,
    };
    const card = {
        top,
        isConnected: true,
        img,
        getBoundingClientRect() { return { top: this.top, bottom: this.top + 240, left: 10, right: 10 + width }; },
    };
    return card;
}

function setup(cards) {
    const frames = [];
    const context = vm.createContext({
        window: { scrollY: 0 },
        innerHeight: VH,
        innerWidth: VW,
        devicePixelRatio: 2,
        performance: { now: () => 0 },
        requestAnimationFrame: fn => frames.push(fn),
        IntersectionObserver: class { observe() {} unobserve() {} },
        API: { token: 'session1', imageUrl: (id, type, opts) => `/Items/${id}/Images/${type}?maxWidth=${opts.maxWidth}&tag=${opts.tag}` },
    });
    vm.runInContext(code, context);
    const loader = context.posterLoader;
    const frame = () => { while (frames.length) frames.shift()(); loader.run(); };
    // A page of cards is added at once, as the renderers do; nothing starts before the next frame
    loader.watch([{ matches: () => false, querySelectorAll: () => cards.map(c => c.img) }]);
    assert.equal(loader.loading.size, 0, 'watch reads no layout and starts nothing itself');
    assert.equal(frames.length, 1, 'one frame is asked for the whole page');
    frame();
    return { loader, frame, context };
}

const started = cards => cards.filter(c => c.img.getAttribute('src')).map(c => cards.indexOf(c));

test('a picture loaded before starts at once when its card comes back, outside the queue', () => {
    // Five loads fill the queue; cards 0 and 1 finish first
    const cards = [0, 100, 200, 300, 400].map((top, i) => makeCard(i, top));
    const { loader, frame, context } = setup(cards);
    cards[0].img.fire('load');
    cards[1].img.fire('load');
    for (const top of [500, 600]) {
        const filler = makeCard(10 + top, top);
        loader.watch([{ matches: () => false, querySelectorAll: () => [filler.img] }]);
    }
    frame();
    assert.equal(loader.loading.size, 5, 'the queue is full');
    const watch = list => { loader.watch([{ matches: () => false, querySelectorAll: () => list.map(c => c.img) }]); frame(); };
    // Recycled cells: item 0 is in the browser's memory cache, item 1 is not (evicted), item 0 again
    // in a card twice as wide after a resize, and a picture never loaded
    const hit = makeCard(0, 50);
    hit.img.cached = true;
    const guess = makeCard(1, 60);
    const wider = makeCard(0, 70, 320);
    const other = makeCard(7, 80);
    watch([hit, guess, wider, other]);
    assert.equal(hit.img.src, cards[0].img.src, 'the remembered URL, with the queue full');
    hit.img.fire('load');
    assert.ok(!loader.loading.has(hit), 'done at once');
    assert.equal(hit.img.style.transition, 'none', 'a picture loaded at once shows without the fade-in');
    assert.equal(guess.img.src, cards[1].img.src, 'a guess starts too');
    assert.ok(loader.loading.has(guess), 'but counts as a load');
    assert.equal(wider.img.src, '', 'a smaller picture is not reused for a wider card');
    assert.equal(other.img.src, '', 'a new picture waits for the plan');
    // A guess scrolled far away is stopped like any load
    guess.top = 9000;
    frame();
    assert.equal(guess.img.src, '', 'stopped');
    // Another session's URLs are not reused
    context.API.token = 'session2';
    const later = makeCard(0, 0);
    later.img.cached = true;
    watch([later]);
    assert.notEqual(later.img.style.transition, 'none');
});

test('on-screen cards start first at high priority, at most 5, sized for the card', () => {
    const cards = [1200, 0, 300, 2000, 600, 900, 1500, 9000].map((top, i) => makeCard(i, top));
    const { loader } = setup(cards);
    assert.deepEqual(started(cards).sort(), [1, 2, 4, 5, 0].sort());
    assert.equal(loader.loading.size, 5);
    for (const i of [1, 2, 4]) assert.equal(cards[i].img.fetchPriority, 'high', `card ${i}`);
    for (const i of [5, 0]) assert.equal(cards[i].img.fetchPriority, 'low', `card ${i}`);
    assert.equal(cards[1].img.src, '/Items/item1/Images/Primary?maxWidth=320&tag=tag'); // 160 px at 2x
});

test('a load scrolled far past stops, drops its listeners, and starts again when it comes back', () => {
    const cards = [0, 300, 900, 1200, 1500, 1800].map((top, i) => makeCard(i, top));
    const { loader, frame } = setup(cards);
    assert.deepEqual(started(cards), [0, 1, 2, 3, 4]);
    // Scroll down 3000 px: cards 0-4 are far above the window, card 5 is not
    cards.forEach(c => { c.top -= 3000; });
    frame();
    assert.deepEqual(started(cards), []);
    assert.equal(loader.loading.size, 0);
    for (const c of cards.slice(0, 5)) assert.equal(c.img.listenerCount(), 0, 'stop removes the listeners');
    // Back up: they start again, each with one listener pair
    cards.forEach(c => { c.top += 3000; });
    frame();
    assert.deepEqual(started(cards), [0, 1, 2, 3, 4]);
    for (const c of cards.slice(0, 5)) assert.equal(c.img.listenerCount(), 2);
    // A load finishing frees its slot for the next card and forgets it
    cards[0].img.fire('load');
    frame();
    assert.equal(cards[0].img.listenerCount(), 0);
    assert.ok(!loader.near.has(cards[0]) && !loader.watched.has(cards[0]));
    assert.deepEqual(started(cards), [0, 1, 2, 3, 4, 5]);
    assert.equal(cards[0].img.getAttribute('src'), '/Items/item0/Images/Primary?maxWidth=320&tag=tag');
});

test('a picture started ahead of the screen is raised to high priority once on screen', () => {
    const cards = [makeCard(0, 1000)];
    const { frame } = setup(cards);
    assert.equal(cards[0].img.fetchPriority, 'low');
    cards[0].top = 200;
    frame();
    assert.equal(cards[0].img.fetchPriority, 'high');
});

test('cards removed from the page are stopped and forgotten', () => {
    const cards = [0, 300].map((top, i) => makeCard(i, top));
    const { loader, frame } = setup(cards);
    cards[0].isConnected = false;
    frame();
    assert.equal(cards[0].img.getAttribute('src'), null);
    assert.ok(!loader.loading.has(cards[0]) && !loader.watched.has(cards[0]));
    // A sweep (route change) drops detached cards that were not near too
    const far = makeCard(2, 20000);
    loader.watch([far.img]);
    frame();
    assert.ok(loader.watched.has(far) && far.img.getAttribute('src') === null, 'far: watched, not started');
    far.isConnected = false;
    loader.sweep();
    assert.ok(!loader.watched.has(far));
    assert.equal(cards[1].img.getAttribute('src') !== null, true);
});
