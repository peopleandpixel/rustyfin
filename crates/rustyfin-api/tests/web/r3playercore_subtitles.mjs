// The player's text subtitles: drawn by the player in the user's appearance (stored per user on this device under
// Jellyfin Web's key), moved by the subtitle offset (G and H, 0.1 s steps within ±30 s), and the menu's fields.
// Loads playerprefs.js as a module and runs player.js's subtitle functions in a vm sandbox.
// Usage: node r3playercore_subtitles.mjs (run by api_tests.rs r3playercore_subtitles).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, src), 'utf8');
const playerJs = read('modules/player.js');
const html = read('index.html');

function slice(head) {
    const start = playerJs.indexOf(head);
    assert.ok(start >= 0, `player.js has ${head}`);
    return playerJs.slice(start, playerJs.indexOf('\n}\n', start) + 3);
}

// A localStorage for the module, and the signed-in user its keys are prefixed with
const store = new Map();
globalThis.localStorage = {
    getItem: k => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: k => store.delete(k),
};
globalThis.API = { userId: 'u1' };
const prefs = await import(new URL('modules/playerprefs.js', src));

test('the appearance is stored per user under Jellyfin Web\'s key and merges with the defaults', () => {
    store.clear();
    assert.deepEqual(prefs.subtitleAppearance(), prefs.APPEARANCE_DEFAULTS);
    prefs.saveSubtitleAppearance({ textSize: 'large' });
    prefs.saveSubtitleAppearance({ textBackground: '#000000' });
    assert.deepEqual(JSON.parse(store.get('u1-localplayersubtitleappearance3')),
        { ...prefs.APPEARANCE_DEFAULTS, textSize: 'large', textBackground: '#000000' });
    store.set('u1-localplayersubtitleappearance3', 'not json');
    assert.deepEqual(prefs.subtitleAppearance(), prefs.APPEARANCE_DEFAULTS, 'a broken value falls back');
});

test('subtitleCss maps the fields as Jellyfin Web\'s appearance helper', () => {
    const css = prefs.subtitleCss({});
    assert.equal(css.text.fontSize, '1.36em');
    assert.equal(css.text.textShadow, '#000000 0px 0px 7px');
    assert.equal(css.text.backgroundColor, 'transparent');
    assert.deepEqual(css.window, { top: 'auto', bottom: '0', marginTop: '0', marginBottom: '2.7em' }, 'default -3: two lines up');
    const custom = prefs.subtitleCss({ textSize: 'extralarge', font: 'smallcaps', textWeight: 'bold', dropShadow: 'none', verticalPosition: 2 });
    assert.equal(custom.text.fontSize, '2.2em');
    assert.equal(custom.text.fontVariant, 'small-caps');
    assert.equal(custom.text.fontWeight, 'bold');
    assert.equal(custom.text.textShadow, 'none');
    assert.deepEqual(custom.window, { top: '0', bottom: 'auto', marginTop: '2.7em', marginBottom: '0' }, 'from 0 on: from the top');
    assert.equal(prefs.subtitleCss({ verticalPosition: -1 }).window.marginBottom, '0em', '-1 sits on the bottom edge');
});

test('the offset steps by 0.1 s within ±30 s', () => {
    assert.equal(prefs.stepSubtitleOffset(0, 1), 0.1);
    assert.equal(prefs.stepSubtitleOffset(0.2, -1), 0.1);
    assert.equal(prefs.stepSubtitleOffset(0.1, 1) + 0, 0.2, 'no float drift');
    assert.equal(prefs.stepSubtitleOffset(30, 1), 30);
    assert.equal(prefs.stepSubtitleOffset(-30, -1), -30);
});

// A fake video with the player's own track and its cues
function sandbox() {
    const cues = [{ startTime: 10, endTime: 12, text: 'one' }, { startTime: 20, endTime: 22, text: 'two' }];
    const trackEl = { dataset: { playerSubtitle: '1' }, track: { mode: 'hidden', cues, activeCues: [cues[0]] } };
    const video = { querySelector: () => trackEl, textTracks: [trackEl.track] };
    const elements = { 'video-player': video };
    const make = tag => ({
        tag, children: [], className: '', style: {}, dataset: {},
        append(...c) { this.children.push(...c); },
        replaceChildren(...c) { this.children = c; },
    });
    const context = vm.createContext({
        $: id => (elements[id] ||= { ...make('el'), hidden: true, textContent: '' }),
        t: (key, o) => (o ? `${key}|${JSON.stringify(o)}` : key),
        document: { createElement: make, createTextNode: text => ({ text }) },
        setTimeout: () => 1,
        clearTimeout: () => {},
        stepSubtitleOffset: prefs.stepSubtitleOffset,
        activePlayback: null,
    });
    vm.runInContext(`let osdTimer = null;
        ${['function playerSubtitleTrack(', 'function renderSubtitleCues(', 'function applySubtitleOffset(',
        'function changeSubtitleOffset(', 'function formatOffset(', 'function showPlayerOsd('].map(slice).join('\n')}
        this.fns = { renderSubtitleCues, applySubtitleOffset, changeSubtitleOffset };
        this.setActive = s => { activePlayback = s; };`, context);
    return { context, elements, cues, trackEl };
}

test('the active cues are drawn one line each, as text', () => {
    const { context, elements } = sandbox();
    context.fns.renderSubtitleCues();
    const lines = elements['player-subtitle-window'].children;
    assert.equal(lines.length, 1);
    assert.equal(lines[0].children[0].className, 'jf-subtitle-text');
    assert.deepEqual(lines[0].children[0].children, [{ text: 'one' }]);
});

test('H and G move the cues by the change since the last offset, and say so', () => {
    const { context, elements, cues, trackEl } = sandbox();
    const session = { subtitleOffset: 0 };
    context.setActive(session);
    context.fns.changeSubtitleOffset(1);
    context.fns.changeSubtitleOffset(1);
    assert.equal(session.subtitleOffset, 0.2);
    assert.ok(Math.abs(cues[0].startTime - 9.8) < 1e-9, 'a positive offset shows them earlier');
    assert.ok(Math.abs(cues[1].endTime - 21.8) < 1e-9);
    assert.equal(trackEl.dataset.offset, '0.2');
    assert.equal(elements['player-osd'].textContent, 'player.subtitle_offset_value|{"offset":"+0.2 s"}');
    context.fns.changeSubtitleOffset(-1);
    assert.ok(Math.abs(cues[0].startTime - 9.9) < 1e-9);
    context.fns.changeSubtitleOffset(0);
    assert.ok(Math.abs(cues[0].startTime - 10) < 1e-9, '0 resets');
    // A reloaded track (a restart) has its cues at 0: its load applies the whole offset
    session.subtitleOffset = 0.5;
    trackEl.dataset.offset = '0';
    context.fns.applySubtitleOffset(session);
    assert.ok(Math.abs(cues[0].startTime - 9.5) < 1e-9);
});

test('without a text subtitle the offset stays and the player says why', () => {
    const { context, elements } = sandbox();
    const session = { subtitleOffset: 0 };
    context.setActive(session);
    vm.runInContext('$("video-player").querySelector = () => null', context);
    context.fns.changeSubtitleOffset(1);
    assert.equal(session.subtitleOffset, 0);
    assert.equal(elements['player-osd'].textContent, 'player.subtitle_offset_text_only');
});

test('the player keys, the hidden track and the markup', () => {
    assert.match(slice('function playerKeyAction('), /case 'g': return \(\) => changeSubtitleOffset\(-1\);\s+case 'h': return \(\) => changeSubtitleOffset\(1\);/);
    const show = slice('function showTextSubtitle(');
    assert.match(show, /track\.track\.mode = 'hidden'/);
    assert.match(show, /oncuechange = renderSubtitleCues/);
    assert.match(show, /addEventListener\('load', \(\) => applySubtitleOffset\(session\)\)/);
    assert.match(html, /id="player-subtitle-window"/);
    // Every appearance field in the menu is a stored field
    const names = [...html.matchAll(/<(?:select|input) name="(\w+)"[^>]*setSubtitleAppearance/g)].map(m => m[1]);
    assert.deepEqual(names.sort(), Object.keys(prefs.APPEARANCE_DEFAULTS).sort());
    // Every size, font and edge the menu offers maps to CSS
    for (const [name, table] of [['textSize', prefs.SUBTITLE_SIZES], ['font', prefs.SUBTITLE_FONTS], ['dropShadow', prefs.SUBTITLE_SHADOWS]]) {
        const select = html.slice(html.indexOf(`name="${name}"`), html.indexOf('</select>', html.indexOf(`name="${name}"`)));
        for (const [, value] of select.matchAll(/<option value="([^"]*)"/g)) assert.ok(value in table, `${name} ${value}`);
    }
});
