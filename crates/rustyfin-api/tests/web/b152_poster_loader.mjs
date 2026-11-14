// B152: the poster loader's planning. Runs app.js's posterWindow and posterPlan in a vm sandbox.
// Usage: node b152_poster_loader.mjs PATH/TO/app.js (run by api_tests.rs b152_poster_loader_plan).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
function fn(head) {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return source.slice(start, source.indexOf('\n}\n', start) + 3);
}
const context = vm.createContext({});
const widths = source.split('\n').find(l => l.startsWith('const POSTER_WIDTHS ='));
assert.ok(widths, 'app.js has POSTER_WIDTHS');
vm.runInContext(
    widths.replace('const ', 'var ') + '\n' + fn('function posterWidth(cssWidth, dpr) {')
        + fn('function posterWindow(velocity, vh) {') + fn('function posterPlan(entries, vw, vh, win, max) {'),
    context,
);
const { posterWidth, posterWindow, posterPlan } = context;

test('a card asks for the smallest width step that covers about its device pixels', () => {
    assert.equal(posterWidth(123, 1), 160); // 1280 px desktop, 7 columns
    assert.equal(posterWidth(123, 2), 240);
    assert.equal(posterWidth(170, 2), 320); // 390 px phone, 2 columns
    assert.equal(posterWidth(170, 3), 480); // 459 device px wanted: 480 covers it
    assert.equal(posterWidth(288, 2), 640); // a landscape row card
    assert.equal(posterWidth(2000, 3), 960); // never more than the largest step
    assert.equal(posterWidth(100, undefined), 160);
});

const VH = 800;
const VW = 1280;
// A card 200 px tall whose top is at `top`
const card = (top, extra = {}) => ({ top, bottom: top + 200, left: 0, right: 150, loading: false, ...extra });

test('the window looks 2 screens ahead at rest and down, 3 when fast, and half a screen behind', () => {
    assert.deepEqual({ ...posterWindow(0, VH) }, { top: -400, bottom: VH + 1600 });
    assert.deepEqual({ ...posterWindow(VH / 1000, VH) }, { top: -400, bottom: VH + 2000 }); // 1 screen/s
    assert.deepEqual({ ...posterWindow(10, VH) }, { top: -400, bottom: VH + 3600 }); // capped at 4.5
    assert.deepEqual({ ...posterWindow(-10, VH) }, { top: -3600, bottom: VH + 400 }); // scrolling up
});

test('on-screen cards start first, then the nearest ahead, up to the limit', () => {
    const entries = [card(2000), card(900), card(100), card(-150), card(1200), card(500)];
    const { start, stop } = posterPlan(entries, VW, VH, posterWindow(0, VH), 3);
    assert.deepEqual([...stop], []);
    // on screen: index 2 (100), 3 (-150, still showing 50 px) and 5 (500); 1 (900) is next
    assert.deepEqual([...start].sort(), [2, 3, 5]);
    const more = posterPlan(entries, VW, VH, posterWindow(0, VH), 5);
    assert.deepEqual([...more.start].slice(3), [1, 4]);
});

test('loads in flight count against the limit, and loads that left the window stop', () => {
    const win = posterWindow(1, VH); // scrolling down: the band is -400 .. 2800
    const entries = [card(-1000, { loading: true }), card(100, { loading: true }), card(300), card(3000), card(2500)];
    const { start, stop } = posterPlan(entries, VW, VH, win, 2);
    assert.deepEqual([...stop], [0]);
    assert.deepEqual([...start], [2]);
});

test('scrolling up looks above, not below', () => {
    const entries = [card(-1500), card(1500)];
    assert.deepEqual([...posterPlan(entries, VW, VH, posterWindow(-2, VH), 5).start], [0]);
    assert.deepEqual([...posterPlan(entries, VW, VH, posterWindow(2, VH), 5).start], [1]);
});

test('cards scrolled sideways out of a row wait for the ones in view, and hidden cards never start', () => {
    const entries = [
        { top: 100, bottom: 300, left: VW + 300, right: VW + 450, loading: false },
        { top: 100, bottom: 300, left: 10, right: 160, loading: false },
        { top: 0, bottom: 0, left: 0, right: 0, loading: false }, // display: none
    ];
    assert.deepEqual([...posterPlan(entries, VW, VH, posterWindow(0, VH), 5).start], [1, 0]);
});
