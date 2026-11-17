// Phones and touch screens: the player's menus, dialogs and the library filter menu become bottom
// sheets that fit the screen; controls get 44 px touch targets under a coarse pointer; hover-only
// details get touch equivalents; the player keeps clear of the gesture bar and notch; and a video in
// fullscreen locks to landscape on a touch device, as Jellyfin 12.4's playbackorientation does.
// Every phone rule sits under a phone, coarse-pointer or no-hover media query, so desktop is unchanged.
// Usage: node r3mobile_touch.mjs (run by api_tests.rs r3mobile_touch).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = p => fs.readFileSync(new URL(p, src), 'utf8');
const css = read('input.css').replace(/\/\*[\s\S]*?\*\//g, '');
const html = read('index.html');
const player = read('modules/player.js');

// The CSS outside any @media block, and the inner text of every @media block whose condition matches
function split() {
    const top = [];
    const media = [];
    let i = 0;
    while (i < css.length) {
        const at = css.indexOf('@media', i);
        if (at < 0) { top.push(css.slice(i)); break; }
        top.push(css.slice(i, at));
        const open = css.indexOf('{', at);
        let depth = 1, j = open + 1;
        while (depth && j < css.length) { if (css[j] === '{') depth++; else if (css[j] === '}') depth--; j++; }
        media.push({ cond: css.slice(at + 6, open).trim(), body: css.slice(open + 1, j - 1) });
        i = j;
    }
    return { top: top.join('\n'), media };
}
const { top, media } = split();
const inMedia = re => media.filter(m => re.test(m.cond)).map(m => m.body).join('\n');
const PHONE = /width\s*<\s*40rem|max-width:\s*(39\.9+rem|639(\.9+)?px)/;
const COARSE = /pointer:\s*coarse/;
const NOHOVER = /hover:\s*none/;
// Declarations of every rule in `text` whose selector list has `sel` as one of its selectors
function decls(text, sel) {
    let out = '';
    for (const m of text.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
        const sels = m[1].split(',').map(s => s.trim().replace(/\s+/g, ' '));
        if (sels.includes(sel)) out += m[2] + ';';
    }
    return out;
}
const has = (text, sel, re, what) => assert.match(decls(text, sel), re, `${sel}: ${what || re}`);

test('B1: the player menus are a bottom sheet on phones, with 44 px rows on touch', () => {
    const phone = inMedia(PHONE);
    has(phone, '.jf-player-menu', /position:\s*fixed/);
    has(phone, '.jf-player-menu', /bottom:\s*0/);
    has(phone, '.jf-player-menu', /left:\s*0/);
    has(phone, '.jf-player-menu', /right:\s*0/);
    has(phone, '.jf-player-menu', /safe-area-inset-bottom/);
    has(phone, '.jf-player-menu', /max-height:\s*70dvh/);
    has(inMedia(COARSE), '.jf-player-menu-select', /min-height:\s*2\.75rem/);
    has(inMedia(COARSE), '.jf-player-menu-btn', /min-height:\s*2\.75rem/);
    // Desktop keeps its popover above the button
    has(top, '.jf-player-menu', /position:\s*absolute/);
});

test('B2: the player bars keep clear of the gesture bar and notch; touch-sized controls', () => {
    has(top, '.jf-player-controls', /padding-bottom:\s*max\([^;]*safe-area-inset-bottom/);
    has(top, '.jf-player-controls', /safe-area-inset-left/);
    has(top, '.jf-player-topbar', /safe-area-inset-top/);
    const coarse = inMedia(COARSE);
    has(coarse, '.jf-player-controls .jf-icon-btn', /min-height:\s*2\.75rem/);
    has(coarse, '#player-seek', /height:\s*1\.75rem/);
});

test('B3: card buttons are bigger on touch, and a tap leaves no hover lift', () => {
    has(inMedia(NOHOVER), '.jf-card-action', /width:\s*2\.5rem/);
    has(inMedia(NOHOVER), '.jf-card-action', /height:\s*2\.5rem/);
    assert.doesNotMatch(decls(top, '.jf-card:hover'), /transform/, 'no top-level .jf-card:hover lift');
    has(inMedia(/hover:\s*hover/), '.jf-card:hover', /transform:\s*translateY\(-6px\) scale\(1\.02\)/);
});

test('B4: the drawer scan button is visible without hover', () => {
    has(inMedia(NOHOVER), '[data-action="scan-library"]', /opacity:\s*0?\.6/);
});

test('B5: the filter menu is centred explicitly, and a bottom sheet on phones', () => {
    has(top, '.jf-filter-menu', /margin:\s*auto/);
    has(top, '.jf-filter-menu', /position:\s*fixed/);
    const phone = inMedia(PHONE);
    has(phone, '.jf-filter-menu', /inset:\s*auto 0 0( 0)?/);
    has(phone, '.jf-filter-menu', /width:\s*100%/);
    has(phone, '.jf-filter-menu', /safe-area-inset-bottom/);
    assert.match(inMedia(COARSE), /\.jf-filter-menu [^{]*\{[^}]*min-height:\s*2\.75rem/);
});

test('B6: dialogs are bottom sheets on phones', () => {
    const phone = inMedia(PHONE);
    has(phone, '.modal', /align-items:\s*flex-end/);
    has(phone, '.modal-box', /width:\s*100%/);
    has(phone, '.modal-box', /max-width:\s*100%/);
    has(phone, '.modal-box', /safe-area-inset-bottom/);
});

test('B7: checkboxes and radios are 20 px on touch, switches untouched', () => {
    const coarse = inMedia(COARSE);
    assert.match(coarse, /input\[type=["']?checkbox["']?\]:not\(\.toggle\)[^{]*\{[^}]*width:\s*1\.25rem/);
    assert.match(coarse, /input\[type=["']?radio["']?\][^{]*\{[^}]*height:\s*1\.25rem/);
});

test('B8: phones get 3 medium poster columns, as Jellyfin; wider breakpoints unchanged', () => {
    has(top, '.jf-lib-grid', /--jf-cols:\s*3/);
    has(top, '.jf-lib-grid.jf-shape-poster.jf-size-small', /--jf-cols:\s*4/);
    has(top, '.jf-lib-grid.jf-shape-poster.jf-size-large', /--jf-cols:\s*2/);
    const at = w => media.filter(m => new RegExp(`width\\s*>=\\s*${w}rem`).test(m.cond)).map(m => m.body).join('\n');
    has(at(40), '.jf-lib-grid', /--jf-cols:\s*3/);
    has(at(48), '.jf-lib-grid', /--jf-cols:\s*4/);
    has(at(40), '.jf-lib-grid.jf-shape-poster.jf-size-small', /--jf-cols:\s*4/);
    has(at(48), '.jf-lib-grid.jf-shape-poster.jf-size-large', /--jf-cols:\s*3/);
});

test('B9: shared buttons, tabs and selects are 44 px on touch', () => {
    const coarse = inMedia(COARSE);
    for (const sel of ['.jf-btn-ghost', '.jf-btn-primary', '.jf-tab', 'select.jf-input']) has(coarse, sel, /min-height:\s*2\.75rem/);
    has(coarse, '.jf-icon-btn', /min-width:\s*2\.75rem/);
    has(coarse, '.jf-icon-btn', /min-height:\s*2\.75rem/);
});

test('B10: the tab strip scrolls without a scrollbar', () => {
    has(top, '.jf-tabs', /scrollbar-width:\s*none/);
    assert.match(top, /\.jf-tabs::-webkit-scrollbar\s*\{[^}]*display:\s*none/);
});

test('B12: the player locks landscape in fullscreen and unlocks on exit and close', () => {
    assert.match(player, /import \{[^}]*lockLandscape[^}]*\} from '\.\/mobile\.js'/);
    const body = name => { const s = player.indexOf(`function ${name}(`); return player.slice(s, player.indexOf('\n}\n', s)); };
    assert.match(body('toggleFullscreen'), /lockLandscape/);
    assert.match(body('toggleFullscreen'), /unlockOrientation\(\)/);
    assert.match(body('closePlayer'), /unlockOrientation\(\)/);
});

test('B13: the header add-library button is hidden below sm', () => {
    const tag = html.match(/<button id="nav-add-lib" class="([^"]*)"/);
    assert.ok(tag);
    assert.match(tag[1], /max-sm:!hidden|jf-sm-up/);
    if (/jf-sm-up/.test(tag[1])) has(top, '.jf-sm-up', /display:\s*none/);
});

test('B11: lockLandscape locks only on a coarse pointer, and unlock undoes only its own lock', async () => {
    const calls = [];
    let coarse = true;
    globalThis.matchMedia = q => ({ matches: q.includes('coarse') ? coarse : false });
    globalThis.screen = { orientation: {
        lock: o => { calls.push(['lock', o]); return Promise.resolve(); },
        unlock: () => calls.push(['unlock']),
    } };
    const m = await import(new URL('modules/mobile.js', src));
    m.unlockOrientation();
    assert.deepEqual(calls, [], 'no lock taken, no unlock');
    m.lockLandscape();
    assert.deepEqual(calls, [['lock', 'landscape']]);
    m.unlockOrientation();
    m.unlockOrientation();
    assert.deepEqual(calls, [['lock', 'landscape'], ['unlock']]);
    coarse = false; calls.length = 0;
    m.lockLandscape(); m.unlockOrientation();
    assert.deepEqual(calls, [], 'a mouse pointer never locks');
    // A browser that rejects the lock (not in fullscreen) is fine
    coarse = true;
    screen.orientation.lock = () => Promise.reject(new Error('NotSupportedError'));
    m.lockLandscape();
    await new Promise(r => setTimeout(r, 0));
    // No orientation API at all
    globalThis.screen = {};
    m.lockLandscape(); m.unlockOrientation();
});
