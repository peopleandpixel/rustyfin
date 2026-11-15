// B152 round 3: the virtual library grid. Runs app.js's grid maths, cell recycling and page fill
// logic in a vm sandbox with a small fake DOM.
// Usage: node b152_library_grid.mjs PATH/TO/app.js (run by api_tests.rs b152_library_grid).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
const fn = head => {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return source.slice(start, source.indexOf('\n}\n', start) + 3);
};
const line = head => {
    const found = source.split('\n').find(l => l.startsWith(head));
    assert.ok(found, `app.js has ${head}`);
    return found.replace(/^const /, 'var ');
};

// --- a fake DOM: elements with children, dataset, attributes, focus ---
class El {
    constructor(tag = 'div') {
        this.tagName = tag.toUpperCase();
        this.children = [];
        this.parent = null;
        this.dataset = {};
        this.style = { setProperty(k, v) { this[k] = v; } };
        this.attrs = {};
        this.classes = new Set();
    }
    get isConnected() { return this.parent ? this.parent.isConnected : this.root === true; }
    get firstElementChild() { return this.children[0] ?? null; }
    get childNodes() { return [...this.children]; }
    set innerHTML(html) { // only the library placeholder is parsed
        const cell = new El('div');
        cell.classes.add('jf-card').add('jf-card-pending');
        const poster = new El('div');
        poster.classes.add('jf-poster');
        cell.appendChild(poster);
        this.children = [];
        this.appendChild(cell);
    }
    setAttribute(k, v) { this.attrs[k] = String(v); }
    getAttribute(k) { return this.attrs[k] ?? null; }
    hasAttribute(k) {
        if (k.startsWith('data-')) return k.slice(5).replace(/-(\w)/g, (_, c) => c.toUpperCase()) in this.dataset;
        return k in this.attrs;
    }
    appendChild(n) { return this.insertBefore(n, null); }
    insertBefore(n, ref) {
        n.parent?.children.splice(n.parent.children.indexOf(n), 1);
        const at = ref ? this.children.indexOf(ref) : this.children.length;
        assert.ok(at >= 0, 'insertBefore: ref is a child');
        this.children.splice(at, 0, n);
        n.parent = this;
        return n;
    }
    remove() { if (this.parent) { this.parent.children.splice(this.parent.children.indexOf(this), 1); this.parent = null; } }
    replaceWith(n) { this.parent.insertBefore(n, this); this.remove(); }
    replaceChildren(...nodes) { this.children.forEach(c => { c.parent = null; }); this.children = []; nodes.forEach(n => this.appendChild(n)); }
    contains(x) { return x === this || this.children.some(c => c.contains(x)); }
    focus() { context.document.activeElement = this; }
    closest(sel) {
        assert.equal(sel, '[data-pos]');
        for (let e = this; e; e = e.parent) if ('pos' in e.dataset) return e;
        return null;
    }
    all() { return [this, ...this.children.flatMap(c => c.all())]; }
    querySelector(sel) { return this.querySelectorAll(sel)[0] ?? null; }
    querySelectorAll(sel) {
        const [tag, cls] = sel.split('.');
        return this.all().slice(1).filter(e => (!tag || e.tagName === tag.toUpperCase()) && (!cls || e.classes.has(cls)));
    }
}

// A card for an item: its id, and its picture by tag
function createCardElement(item) {
    const card = new El('div');
    card.classes.add('jf-card');
    card.dataset.id = item.Id;
    const tag = item.ImageTags?.Primary;
    if (tag) {
        const img = new El('img');
        img.classes.add('jf-thumb-img');
        img.dataset.imgTag = tag;
        card.appendChild(img);
    }
    return card;
}

const forgotten = [];
const context = vm.createContext({
    document: { createElement: tag => new El(tag), activeElement: null },
    createCardElement,
    railLetter: item => (item.Name || '#')[0].toUpperCase(),
    posterLoader: { forget: c => forgotten.push(c) },
    AbortController,
    URLSearchParams,
    console,
});
vm.runInContext([
    'var libraryView = null, libraryItems = [], libraryIndex = null, libraryTotal = null, libraryGen = 1;',
    'var libraryInFlight = new Map(), libraryRequested = new Set(), libraryDir = 1, libraryIndexRequest = null;',
    'var calls = { changed: [], loaded: [], scheduled: 0 };',
    'var librarySettle = null, libraryLastScroll = -1e9, libraryLastSpeed = 0, performance = { now: () => 0 };',
    'var libraryIndexHashRetries = 0, libraryIndexAt = null, libraryFlyingNow = false, libraryIndexMissing = null;',
    // Frames are kept until a test runs them (flushFrames)
    'var frames = []; function requestAnimationFrame(f) { frames.push(f); } function flushFrames() { while (frames.length) frames.shift()(); }',
    // Timers are kept, so a test can see what was queued, cleared or run
    'var timers = []; function setTimeout(f, ms) { timers.push({ f, ms, live: true }); return timers.length; }',
    'function clearTimeout(id) { if (timers[id - 1]) timers[id - 1].live = false; }',
    line('const LIBRARY_INDEX_SETTLE_MS ='),
    line('const BLURHASH_DIGITS ='),
    line('const LIBRARY_FLYING_SPEED ='),
    line('const LIBRARY_PLACEHOLDER ='),
    line('const LIBRARY_PAGE_SIZE ='),
    line('const LIBRARY_PAGES_AT_ONCE ='),
    line('const LIBRARY_OVERSCAN ='),
    line('const SKELETON_RGB ='),
    line('const TINT_MIN_LUMA ='),
    line('const lumaOf ='),
    line('const LIBRARY_INDEX_LIMIT ='),
    line('const LIBRARY_INDEX_HASH_RETRY_MS ='),
    line('const LIBRARY_INDEX_HASH_RETRIES ='),
    line('const cardPictureTag ='),
    ...['function libraryLayout(', 'function libraryCellBox(', 'function libraryCellRange(', 'function libraryPagePlan(',
        'function libraryCellElement(', 'function libraryPlace(', 'function libraryRenderRange(', 'function libraryRefreshCells(',
        'function libraryReplaceCell(', 'function libraryFillCell(', 'function liteCardDiffers(', 'function upgradeCard(',
        'async function loadLibraryPage(', 'function libraryUpdate(', 'function libraryFlying(',
        'function libraryScrollSpeed(', 'function blurhashAverage(', 'function stopLibraryLoad(',
        'function libraryRowsAhead(', 'function blurhashTint(', 'function librarySize(', 'function libraryHasHash(',
        'function libraryFlyingLimit('].map(fn),
    // Stand-ins for what the tests drive by hand
    'function scheduleLibraryUpdate() { calls.scheduled++; }',
    'function scheduleRailUpdate() {}',
    'function libraryChanged(total) { calls.changed.push(total); }',
    'function libraryEmptyHtml() { return \'<div class="text-center" tabindex="-1">library.no_filter_results</div>\'; }',
    'function loadLibraryIndex(gen) { calls.index = (calls.index || 0) + 1; }',
    'var libraryIndexRefresh = null, railAnchor = null, libraryQuery = {}, container = null;',
    'var librarySeq = 0, libraryTotalSeq = 0;',
    'function $(id) { return id === "lib-grid-container" ? container : null; }',
    'function libraryGridTop() { return 0; }',
    'function toast() {} function t(k) { return k; }',
    'var pageAnswers = {};',
    'async function fetchLibraryPage(start) { const a = await pageAnswers[start]; if (a instanceof Error) throw a; return a; }',
].join('\n'), context);

const COLS = 7;
// The cells on a screen whose top is `y` px into the grid (800 px tall)
function screenOf(y) {
    const v = context.libraryView;
    return context.libraryCellRange(y - context.libraryGridTop(v), y - context.libraryGridTop(v) + 800, v.pitch, v.cols, context.libraryTotal);
}

function newGrid(total) {
    const grid = new El('div');
    grid.root = true;
    context.libraryView = {
        ...context.libraryLayout(1000, COLS, 16, 300, total),
        grid,
        cells: new Map(),
        resizeObserver: { disconnect() {} },
    };
    context.libraryTotal = total;
    context.libraryItems = [];
    context.libraryIndex = null;
    context.libraryGen = 1;
    context.libraryFlyingNow = false;
    context.document.activeElement = null;
    forgotten.length = 0;
    return grid;
}
const order = grid => grid.children.map(c => Number(c.dataset.pos));
const item = (id, tag = `t${id}`, extra = {}) => ({ Id: `id${id}`, Name: `N${id}`, ImageTags: { Primary: tag }, ...extra });

test('the layout and a cell box follow the columns, gap and cell height', () => {
    const l = context.libraryLayout(1000, 7, 16, 300, 3654);
    assert.equal(l.colWidth, (1000 - 6 * 16) / 7);
    assert.equal(l.pitch, 316);
    assert.equal(l.height, 522 * 316 - 16); // 3,654 items in 522 rows
    assert.deepEqual({ ...context.libraryCellBox(8, l) }, { x: l.colWidth + 16, y: 316 });
    assert.equal(context.libraryLayout(1000, 7, 16, 300, 0).height, 0);
    // Exact at any depth: row 9,999 of 10,000 items in two columns is at 4,999 pitches
    const phone = context.libraryLayout(358, 2, 16, 340, 10000);
    assert.equal(context.libraryCellBox(9999, phone).y, 4999 * 356);
});

test('cells are recycled: kept ones stay the same elements, in index order', () => {
    const grid = newGrid(500);
    context.libraryRenderRange(0, 20);
    assert.deepEqual(order(grid), [...Array(21).keys()]);
    const seven = context.libraryView.cells.get(7);
    assert.equal(seven.style.top, '316px');
    assert.equal(seven.getAttribute('aria-posinset'), '8');
    assert.equal(seven.getAttribute('aria-setsize'), '500');
    assert.ok(seven.hasAttribute('data-i'), 'no item yet: a skeleton');

    context.libraryRenderRange(7, 34);
    assert.deepEqual(order(grid), [...Array(28).keys()].map(i => i + 7));
    assert.equal(context.libraryView.cells.get(7), seven, 'the same element');
    assert.equal(forgotten.length, 7, 'cells 0-6 left the loader');
    // Scrolling back up puts the new cells before the kept ones
    context.libraryRenderRange(0, 10);
    assert.deepEqual(order(grid), [...Array(11).keys()]);
});

test('the focused cell stays in the page, in order, when the range moves away', () => {
    const grid = newGrid(500);
    context.libraryItems = Array.from({ length: 500 }, (_, i) => item(i));
    context.libraryRenderRange(0, 20);
    const focused = context.libraryView.cells.get(12);
    assert.equal(focused.getAttribute('role'), 'listitem', 'the cell is the list item');
    assert.equal(focused.firstElementChild.getAttribute('role'), 'link', 'its card is the link that opens it (F29)');
    assert.equal(focused.firstElementChild.tabIndex, 0, 'cards with an item take focus');
    focused.firstElementChild.focus();
    context.libraryRenderRange(100, 110);
    assert.deepEqual(order(grid), [12, ...[...Array(11).keys()].map(i => i + 100)]);
    assert.equal(context.libraryView.cells.get(12), focused);
    context.libraryRenderRange(0, 5);
    assert.deepEqual(order(grid), [0, 1, 2, 3, 4, 5, 12]);
});

test('a page fills skeletons, keeps lite cards, upgrades in place, and replaces another item (F17)', async () => {
    const grid = newGrid(100);
    context.libraryIndex = Array.from({ length: 100 }, (_, i) => item(i));
    context.libraryIndex[3] = item(999, 'other'); // the index saw another item at 3 (the list moved)
    context.libraryRenderRange(0, 9);
    const cells = i => context.libraryView.cells.get(i);
    const before = [0, 1, 2, 3].map(cells);
    assert.ok(before.every(c => c.firstElementChild.dataset.lite === '1'), 'lite cards from the index');
    const oldImg = before[2].querySelector('img.jf-thumb-img');
    context.pageAnswers[0] = {
        total: 100,
        items: Array.from({ length: 50 }, (_, i) => item(i, `t${i}`, i === 2 ? { UserData: { Played: true } } : {})),
    };
    await context.loadLibraryPage(1, 0);
    assert.equal(cells(0), before[0], 'same item, no watch state: the same card');
    assert.equal(cells(0).firstElementChild.dataset.lite, undefined, 'no longer lite');
    assert.equal(cells(2), before[2], 'played: upgraded in place');
    assert.equal(cells(2).querySelector('img.jf-thumb-img'), oldImg, 'its picture element is kept');
    assert.notEqual(cells(3), before[3], 'another item: a new card');
    assert.equal(cells(3).firstElementChild.dataset.id, 'id3');
    assert.ok(forgotten.includes(before[3].firstElementChild), 'the old card left the loader');
    assert.deepEqual(order(grid), [...Array(10).keys()]);
    assert.ok(context.calls.scheduled > 0, 'the next pages are asked for');
});

test('a page with another total starts over; a stale load changes nothing', async () => {
    newGrid(100);
    context.libraryRenderRange(0, 9);
    context.pageAnswers[50] = { total: 98, items: [] };
    await context.loadLibraryPage(1, 1);
    assert.deepEqual([...context.calls.changed], [98]);
    context.pageAnswers[0] = { total: 100, items: [item(0)] };
    context.libraryGen = 2; // a newer load began
    await context.loadLibraryPage(1, 0);
    assert.ok(context.libraryView.cells.get(0).hasAttribute('data-i'), 'still a skeleton');
});

test('pages far from a new landing are aborted, and the screen\'s pages are asked first', () => {
    newGrid(3654);
    const far = new AbortController();
    context.libraryInFlight = new Map([[60, far]]);
    context.libraryRequested = new Set([0, 60]);
    context.calls.loaded = [];
    // Record the pages asked instead of loading them
    vm.runInContext('loadLibraryPage = (gen, page) => { calls.loaded.push(page); libraryInFlight.set(page, new AbortController()); };', context);
    context.window = { scrollY: 0 };
    context.innerHeight = 800;
    vm.runInContext('var window = globalThis.window, innerHeight = globalThis.innerHeight;', context);
    context.libraryUpdate();
    assert.ok(far.signal.aborted, 'page 60 aborted');
    assert.ok(!context.libraryRequested.has(60));
    assert.deepEqual([...context.calls.loaded], [1, 2], 'page 0 held, then the pages ahead, two at a time');
    // At rest the overscan is LIBRARY_OVERSCAN.aheadMin rows (6 x 316 px) below the screen: rows 0-8
    assert.equal(context.libraryView.cells.size, 63);
});

test('a lone jump after a pause is a landing; consecutive fast events fly (F20)', () => {
    const speed = context.libraryScrollSpeed;
    assert.equal(speed(800, 5000, 800), 0, 'one screen after five idle seconds: a landing');
    assert.equal(speed(20000, 300, 800), 0, 'a scrollbar jump after a pause too');
    assert.equal(speed(220, 16, 800), (220 / 800) * (1000 / 16), 'a smooth fling frame: about 17 screens a second');
    assert.ok(speed(220, 16, 800) > context.LIBRARY_FLYING_SPEED);
    assert.ok(speed(40, 16, 800) < context.LIBRARY_FLYING_SPEED, 'a steady 2,500 px/s scroll does not fly');
});

test('while flying, new cells are tinted skeletons of the screen and its overscan, and become cards when it settles', () => {
    const grid = newGrid(3654);
    const DIGITS = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~';
    const dc = v => Array.from({ length: 4 }, (_, k) => DIGITS[Math.floor(v / 83 ** (3 - k)) % 83]).join('');
    context.libraryIndex = Array.from({ length: 3654 }, (_, i) => ({ ...item(i), ImageBlurHashes: { Primary: { [`t${i}`]: '00' + dc(0x102030) } } }));
    context.libraryInFlight = new Map();
    context.libraryRequested = new Set([0]);
    vm.runInContext('loadLibraryPage = () => {};', context);
    context.window.scrollY = 20000;
    context.libraryUpdate(20); // 20 screens a second
    const cells = [...context.libraryView.cells.values()];
    // The screen (rows 50-52, 20,000-20,800 px), 2 rows behind and, at 20 screens a second, the
    // most rows ahead (10): rows 48-62
    assert.equal(cells.length, 15 * 7);
    assert.ok(cells.every(c => c.dataset.tint === '1' && c.hasAttribute('data-i')), 'tinted skeletons');
    // The average rgb(16, 32, 48) is darker than TINT_MIN_LUMA: lifted toward the skeleton tone
    assert.equal(cells[0].firstElementChild.firstElementChild.style.backgroundColor, 'rgb(57, 65, 77)');
    // It settles: the screen's rows become lite cards at once, the overscan in the next frame;
    // the band is back to its resting 6 rows: rows 48-58
    context.libraryUpdate(0);
    const isCard = c => c.firstElementChild.dataset.lite === '1';
    const screen = screenOf(20000);
    const cellsOf = (a, b) => [...context.libraryView.cells].filter(([i]) => i >= a && i <= b).map(([, c]) => c);
    assert.ok(cellsOf(screen.first, screen.last).every(isCard), 'the screen first');
    assert.ok(!cellsOf(screen.last + 1, Infinity).some(isCard), 'the overscan waits a frame');
    context.flushFrames();
    const settled = [...context.libraryView.cells.values()];
    assert.ok(settled.every(isCard), 'cards now');
    assert.equal(settled.length, 11 * 7);
    assert.deepEqual(order(grid), [...order(grid)].sort((a, b) => a - b));
});

test('leaving the library makes its running load stale: a late first page or page builds nothing (F24)', async () => {
    vm.runInContext(fn('async function loadLibraryPage('), context); // the real one (an earlier test stubbed it)
    newGrid(100);
    const pending = new AbortController();
    context.libraryInFlight = new Map([[1, pending]]);
    context.libraryGen = 5;
    const gen = context.libraryGen; // the load that is running (renderLibrary holds this)
    context.stopLibraryLoad();
    assert.notEqual(context.libraryGen, gen, 'the generation moved on');
    assert.ok(pending.signal.aborted);
    assert.equal(context.libraryView, null);
    assert.equal(context.libraryTotal, null);
    // The old load's page comes in late: nothing is kept or drawn
    context.pageAnswers[0] = { total: 100, items: [item(0)] };
    await context.loadLibraryPage(gen, 0);
    assert.equal(context.libraryItems.length, 0);
    assert.equal(context.libraryView, null);
});

test('a page with an aborted request leaves a newer request of that page alone (F26)', async () => {
    vm.runInContext(fn('async function loadLibraryPage('), context);
    newGrid(100);
    context.libraryGen = 7;
    context.libraryInFlight = new Map();
    context.libraryRequested = new Set();
    const aborted = Object.assign(new Error('aborted'), { name: 'AbortError' });
    let reject;
    context.pageAnswers[350] = new Promise((_, no) => { reject = no; });
    const done = context.loadLibraryPage(7, 7);
    // A landing aborted it and asked for page 7 again meanwhile
    const newer = new AbortController();
    context.libraryInFlight.set(7, newer);
    context.libraryRequested.add(7);
    reject(aborted);
    await done;
    assert.ok(context.libraryRequested.has(7), 'the newer request keeps its mark');
    assert.equal(context.libraryInFlight.get(7), newer, 'and its controller');
});

test('a changed total keeps the cells in view, updates the count in place, and asks the index once it settles (F25, F28)', async () => {
    vm.runInContext(fn('function libraryChanged('), context);
    const grid = newGrid(100);
    context.libraryIndex = Array.from({ length: 100 }, (_, i) => item(i));
    context.libraryItems = [];
    context.libraryInFlight = new Map();
    context.libraryRequested = new Set([0, 1]);
    context.libraryGen = 3;
    context.window.scrollY = 0;
    vm.runInContext('loadLibraryPage = (gen, page) => { calls.loaded.push(page); }; timers.length = 0; calls.index = 0; calls.loaded = [];', context);
    context.libraryRenderRange(0, 20);
    const before = new Map(context.libraryView.cells);
    const index = context.libraryIndex;

    context.libraryChanged(105); // a scan added items
    context.libraryChanged(107);
    const v = context.libraryView;
    for (const [i, cell] of before) {
        assert.equal(v.cells.get(i), cell, `cell ${i} kept`);
        assert.equal(cell.getAttribute('aria-setsize'), '107');
    }
    assert.equal(grid.style.height, `${v.height}px`);
    assert.equal(v.height, Math.ceil(107 / COLS) * v.pitch - v.gap);
    assert.equal(context.libraryIndex, index, 'the index stays until it is asked again');
    assert.ok(context.calls.loaded.includes(0), 'the screen\'s page is asked again: it may have moved');
    const live = context.timers.filter(t => t.live);
    assert.equal(live.length, 1, 'one index refresh queued, the earlier one cleared');
    assert.equal(live[0].ms, context.LIBRARY_INDEX_SETTLE_MS);
    live[0].f();
    assert.equal(context.calls.index, 1, 'the index is asked once the total held');

    // A page brings another item at a position on screen: that card is replaced
    context.libraryItems[4] = item(55);
    const four = v.cells.get(4);
    context.libraryFillCell(4);
    assert.notEqual(v.cells.get(4), four);
    assert.equal(v.cells.get(4).firstElementChild.dataset.id, 'id55');

    // The same item with another picture (a metadata refresh) gets a new card too
    context.libraryItems[5] = item(5, 'new-poster');
    const five = v.cells.get(5);
    context.libraryFillCell(5);
    assert.notEqual(v.cells.get(5), five);

    // A scan ends while the list is open (LibraryChanged): it is brought up to date in place
    vm.runInContext(fn('function libraryRefreshInPlace('), context);
    const kept = new Map(v.cells);
    context.calls.loaded = [];
    context.timers.length = 0;
    const stale = new AbortController();
    context.libraryInFlight.set(9, stale);
    assert.equal(context.libraryRefreshInPlace(), true);
    assert.ok(stale.signal.aborted, 'requests of the old list are dropped');
    assert.ok(context.calls.loaded.includes(0), 'the screen\'s page is asked again');
    for (const [i, cell] of kept) assert.equal(v.cells.get(i), cell, `cell ${i} stays meanwhile`);
    assert.equal(context.timers.filter(t => t.live).length, 1, 'one index refresh, after the settle');

    // Everything removed: the empty state, focus on it, no view
    context.container = new El('div');
    v.cells.get(2).firstElementChild.focus();
    context.libraryChanged(0);
    assert.equal(context.libraryView, null);
    assert.equal(context.container.children.length, 1, 'the empty state');
    assert.equal(context.document.activeElement, context.container.firstElementChild, 'focus on it');
});

test('a changed total keeps in-flight pages marked, and an older answer never sets the total back (F31)', async () => {
    vm.runInContext(fn('async function loadLibraryPage('), context);
    newGrid(100);
    context.libraryIndex = Array.from({ length: 100 }, (_, i) => item(i));
    context.libraryInFlight = new Map();
    context.libraryRequested = new Set();
    context.libraryGen = 9;
    context.libraryTotalSeq = context.librarySeq;
    context.libraryRenderRange(0, 9);
    let answerOld, answerNew;
    context.pageAnswers[0] = new Promise(r => { answerOld = r; });
    const older = context.loadLibraryPage(9, 0); // asked first
    context.pageAnswers[50] = new Promise(r => { answerNew = r; });
    const newer = context.loadLibraryPage(9, 1); // asked later
    // The later request answers first, with the scan's new total
    answerNew({ total: 120, items: Array.from({ length: 50 }, (_, j) => item(50 + j)) });
    await newer;
    assert.equal(context.libraryTotal, 120);
    assert.ok(context.libraryRequested.has(0), 'the page still in flight keeps its mark');
    // The earlier request answers last, with the old total: left out
    answerOld({ total: 100, items: Array.from({ length: 50 }, (_, j) => item(j)) });
    await older;
    assert.equal(context.libraryTotal, 120, 'the total does not go back');
    // Its old items were left out, so the page is asked again under the new total (F33)
    assert.ok(!context.libraryRequested.has(0), 'page 0 is no longer marked');
    context.pageAnswers[0] = { total: 120, items: Array.from({ length: 50 }, (_, j) => item(j)) };
    context.window.scrollY = 0;
    context.libraryUpdate(0);
    assert.ok(context.libraryRequested.has(0), 'the next update asks for page 0 again');
    for (let k = 0; k < 5 && context.libraryInFlight.has(0); k++) await new Promise(r => setImmediate(r));
    for (let i = 0; i < 10; i++) {
        const cell = context.libraryView.cells.get(i);
        assert.ok(!cell.hasAttribute('data-i'), `cell ${i} is no skeleton`);
        assert.equal(cell.firstElementChild.dataset.lite, undefined, `cell ${i} is a full card`);
        assert.equal(cell.firstElementChild.dataset.id, `id${i}`);
    }
});

test('a refresh in place redraws a full card whose text changed, keeping its picture (F32)', async () => {
    vm.runInContext('loadLibraryPage = (gen, page) => { calls.loaded.push(page); };', context);
    newGrid(100);
    context.libraryIndex = null;
    context.libraryItems = Array.from({ length: 100 }, (_, i) => item(i));
    context.libraryInFlight = new Map();
    context.libraryRequested = new Set();
    context.libraryRenderRange(0, 9);
    const cell = context.libraryView.cells.get(1);
    const card = cell.firstElementChild;
    const img = card.querySelector('img.jf-thumb-img');
    assert.equal(card.dataset.lite, undefined, 'a full card');
    context.libraryRefreshInPlace();
    assert.equal(card.dataset.stale, '1');
    // Its page comes again: the same item and picture, a new title
    context.libraryItems[1] = item(1, 't1', { Name: 'Renamed' });
    context.libraryFillCell(1);
    assert.equal(context.libraryView.cells.get(1), cell, 'the same cell');
    assert.equal(cell.firstElementChild, card, 'and card');
    assert.equal(card.querySelector('img.jf-thumb-img'), img, 'its picture element is kept');
    assert.equal(card.dataset.stale, undefined, 'redrawn');
    assert.equal(card.dataset.letter, 'R', 'from the new item');
});

test('the place in the list is restored only through history (F27)', () => {
    const calls = [];
    const router = vm.createContext({
        URLSearchParams,
        history: { replaceState: (_, __, url) => { router.window.location.hash = url; } },
        sessionStorage: { setItem() {} },
        window: { location: { hash: '' } },
        navigate: (page, data, opts) => calls.push({ page, data, fromHistory: opts?.fromHistory ?? false }),
    });
    vm.runInContext(`var navigatedHash = null;\nvar startedDiag = 0; function startGridDiag(asked) { if (asked) startedDiag++; }\n${fn('function hashRoute(')}\n${fn('function handleHash(')}`, router);
    // navigate() set this hash itself: its own hashchange is not history
    router.navigatedHash = 'library/1';
    router.window.location.hash = '#library/1';
    router.handleHash();
    // Back to the library from an item: a hash navigate() did not set
    router.navigatedHash = 'detail/9';
    router.window.location.hash = '#library/1';
    router.handleHash();
    assert.deepEqual(calls.map(c => c.fromHistory), [false, true]);
    // A ?query after the route is split off, never part of the page's parameter; debug=grid there
    // turns the diagnostics on; a leading slash is allowed
    calls.length = 0;
    router.window.location.hash = '#/library/abc?debug=grid';
    router.handleHash();
    router.window.location.hash = '#library/abc?x=1';
    router.handleHash();
    router.window.location.hash = '#library/abc';
    router.handleHash();
    assert.deepEqual(calls.map(c => [c.page, c.data]), [['library', 'abc'], ['library', 'abc'], ['library', 'abc']]);
    assert.equal(router.startedDiag, 1, 'debug=grid in the hash starts the panel');
    router.window.location.hash = '#?debug=grid';
    router.handleHash();
    assert.deepEqual(calls.at(-1), { page: 'home', data: undefined, fromHistory: false });
    // A typed hash in another form is made canonical in place: one render, no history entry
    // added, no second hashchange; debug=grid is kept for the tab
    {
        let entries = 0;
        let renders = 0;
        const stored = new Map();
        const sim = vm.createContext({ URLSearchParams });
        const location = {
            current: '',
            get hash() { return this.current; },
            // As a browser: a new value adds a history entry and fires hashchange
            set hash(value) {
                const next = value.startsWith('#') ? value : `#${value}`;
                if (next === this.current) return;
                this.current = next;
                entries++;
                sim.handleHash();
            },
        };
        Object.assign(sim, {
            window: { location },
            history: { replaceState: (_, __, url) => { location.current = url; } },
            sessionStorage: { setItem: (k, v) => stored.set(k, v) },
            navigate: (page, data) => {
                renders++;
                sim.navigatedHash = data ? `${page}/${data}` : page;
                location.hash = sim.navigatedHash;
            },
        });
        vm.runInContext(`var navigatedHash = null;\nfunction startGridDiag() {}\n${fn('function hashRoute(')}\n${fn('function handleHash(')}`, sim);
        location.hash = '#/library/abc?debug=grid'; // typed: one entry, its hashchange
        assert.equal(renders, 1, 'drawn once');
        assert.equal(entries, 1, 'only the typed entry');
        assert.equal(location.hash, '#library/abc', 'replaced by the canonical hash');
        assert.equal(stored.get('rustyfin.gridDebug'), '1', 'debug=grid kept for a reload');
    }
    // navigate() turns that into the restore decision, and fillLibraryGrid uses only it
    assert.match(fn('function navigate('), /libraryRestore = fromHistory && page === 'library'/);
    assert.match(fn('function navigate('), /navigatedHash = data \?/);
    const fill = fn('function fillLibraryGrid(');
    assert.match(fill, /const memory = libraryRestore \? libraryMemory : null;/);
    assert.match(fill, /libraryMemory = null;/);
});

test('slowing under the flying speed, the screen\'s rows become cards while the overscan stays colour cells', () => {
    newGrid(3654);
    context.libraryIndex = Array.from({ length: 3654 }, (_, i) => item(i));
    vm.runInContext('loadLibraryPage = () => {};', context);
    context.window.scrollY = 20000;
    context.libraryUpdate(20);
    assert.ok(context.libraryFlyingNow);
    // 7 screens a second: still flying (over three quarters of 8), but slower than the limit
    context.libraryUpdate(7);
    assert.ok(context.libraryFlyingNow);
    const screen = screenOf(20000);
    const cells = [...context.libraryView.cells];
    assert.ok(cells.filter(([i]) => i >= screen.first && i <= screen.last).every(([, c]) => c.firstElementChild.dataset.lite === '1'), 'screen rows are cards');
    const ahead = cells.filter(([i]) => i > screen.last);
    assert.ok(ahead.length > 0 && ahead.every(([, c]) => c.hasAttribute('data-i')), 'the overscan stays colour cells');
    context.libraryUpdate(0);
    context.flushFrames();
});

test('once flying, the page keeps flying down to three quarters of the limit, so it does not switch every frame', () => {
    newGrid(3654);
    context.libraryIndex = Array.from({ length: 3654 }, (_, i) => item(i));
    vm.runInContext('loadLibraryPage = () => {};', context);
    context.window.scrollY = 20000;
    const limit = context.LIBRARY_FLYING_SPEED;
    context.libraryUpdate(limit * 0.8);
    assert.ok(!context.libraryFlyingNow, 'below the limit: not flying');
    context.libraryUpdate(limit * 1.5);
    assert.ok(context.libraryFlyingNow, 'above it: flying');
    context.libraryUpdate(limit * 0.8);
    assert.ok(context.libraryFlyingNow, 'still above three quarters of it: still flying');
    context.libraryUpdate(limit * 0.7);
    assert.ok(!context.libraryFlyingNow, 'below three quarters: it stops flying');
    context.libraryUpdate(0); // it settles
    assert.ok(!context.libraryFlyingNow);
});

test('the overscan grows with the scroll speed, from 6 rows ahead at rest to 10', () => {
    const ahead = speed => context.libraryRowsAhead(speed, 316, 800);
    assert.equal(ahead(0), context.LIBRARY_OVERSCAN.aheadMin);
    // 2 screens a second cover 480 px in 300 ms: two more rows of 316 px
    assert.equal(ahead(2), context.LIBRARY_OVERSCAN.aheadMin + 2);
    assert.equal(ahead(20), context.LIBRARY_OVERSCAN.aheadMax);
    assert.equal(context.libraryRowsAhead(5, 0, 800), context.LIBRARY_OVERSCAN.aheadMin, 'no geometry yet');
});

test('a cell colour is never near black: dark BlurHash averages are lifted, light ones kept', () => {
    const DIGITS = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~';
    const hash = v => '00' + Array.from({ length: 4 }, (_, k) => DIGITS[Math.floor(v / 83 ** (3 - k)) % 83]).join('');
    assert.equal(context.blurhashTint(hash(0x8090a0)), 'rgb(128, 144, 160)', 'light enough: unchanged');
    // Darker averages reach the floor's luma exactly, never past it: a darker poster never gets a
    // lighter cell
    const luma = s => { const c = s.match(/\d+/g).map(Number); return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]; };
    assert.equal(context.blurhashTint(hash(0)), 'rgb(58, 65, 75)', 'black: lifted to the floor');
    for (const v of [0, 0x101010, 0x203040, 0x404040]) {
        assert.ok(Math.abs(luma(context.blurhashTint(hash(v))) - context.TINT_MIN_LUMA) < 1, v.toString(16));
    }
    assert.equal(context.blurhashTint('bad'), null);
});

test('the grid takes the list\'s height and paints nothing else (no list-tall skeleton)', () => {
    const grid = newGrid(3650);
    const v = context.libraryView;
    context.librarySize(v, 3650);
    assert.equal(grid.style.height, `${v.height}px`);
    assert.deepEqual(Object.keys(grid.style).filter(k => k.startsWith('--vg')), []);
});

test('an index whose pictures mostly have no BlurHash yet is asked again later, a few times', async () => {
    vm.runInContext(fn('async function loadLibraryIndex('), context);
    newGrid(100);
    const answers = [];
    context.API = { get: async () => answers.shift() };
    context.libraryQuery = {};
    context.libraryIndexHashRetries = 0;
    const bare = { Items: Array.from({ length: 100 }, (_, i) => item(i)) };
    const hashed = { Items: Array.from({ length: 100 }, (_, i) => item(i, `t${i}`, { ImageBlurHashes: { Primary: { [`t${i}`]: 'L00000fQfQfQfQfQfQfQfQfQfQfQ' } } })) };
    const live = () => context.timers.filter(t => t.live && t.ms === context.LIBRARY_INDEX_HASH_RETRY_MS);
    answers.push(bare);
    await context.loadLibraryIndex(1);
    assert.equal(context.libraryIndex.length, 100);
    assert.equal(live().length, 1, 'asked again later');
    assert.equal(context.libraryIndexHashRetries, 1);
    // The retry finds the BlurHashes: no more retries
    answers.push(hashed);
    live()[0].live = false;
    await context.loadLibraryIndex(1);
    assert.equal(live().length, 0);
    // Retries stop at LIBRARY_INDEX_HASH_RETRIES
    context.libraryIndexHashRetries = context.LIBRARY_INDEX_HASH_RETRIES;
    answers.push(bare);
    await context.loadLibraryIndex(1);
    assert.equal(live().length, 0);
    vm.runInContext('function loadLibraryIndex(gen) { calls.index = (calls.index || 0) + 1; }', context);
});
