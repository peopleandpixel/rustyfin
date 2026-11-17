// The music player's lyrics: the timed-line helpers, the panel's loading and empty state, the highlight of the
// line sung, and the wiring in player.js, router.js, app.js, index.html and the styles.
// Runs lyrics.js in a vm sandbox (its import dropped) with a stub DOM and API.
// Usage: node r3music2_lyrics.mjs (run by api_tests.rs r3music2_lyrics).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const lyricsSource = read('modules/lyrics.js');
const player = read('modules/player.js');
const router = read('modules/router.js');
const app = read('app.js');
const html = read('index.html');
const css = read('input.css');
const en = JSON.parse(read('translations/en.json'));

const escapeHtml = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const settle = () => new Promise(resolve => setImmediate(resolve));

// lyrics.js with its import dropped and its export list turned into this.fns; $ hands out stub elements by id
function sandbox(api) {
    const els = new Map();
    const $ = id => {
        if (!els.has(id)) els.set(id, { id, hidden: false, innerHTML: '', aria: {}, setAttribute(k, v) { this.aria[k] = v; } });
        return els.get(id);
    };
    const context = vm.createContext({ $, escapeHtml, t: key => key, API: api, console });
    const code = lyricsSource.replace(/^import .*$/m, '').replace(/^export \{/m, 'this.fns = {');
    vm.runInContext(code, context);
    return { fns: context.fns, $ };
}

// A line of the panel with the class list and scrollIntoView that highlightLyrics uses
function fakeLine() {
    const classes = new Set();
    return {
        classes,
        scrolls: 0,
        classList: { add: c => classes.add(c), remove: c => classes.delete(c) },
        scrollIntoView() { this.scrolls += 1; },
    };
}

test('activeLyricIndex is the last timed line that has started, skipping untimed lines', () => {
    const { fns } = sandbox({ get: async () => null });
    const lines = [{ Text: 'intro', Start: 0 }, { Text: 'plain' }, { Text: 'verse', Start: 10e7 }, { Text: 'chorus', Start: 20e7 }];
    assert.equal(fns.activeLyricIndex(lines, 0), 0);
    assert.equal(fns.activeLyricIndex(lines, 15e7), 2);
    assert.equal(fns.activeLyricIndex(lines, 20e7), 3);
    assert.equal(fns.activeLyricIndex(lines, 999e7), 3);
    assert.equal(fns.activeLyricIndex(lines, -1), -1, 'before the first line');
    assert.equal(fns.activeLyricIndex([{ Text: 'a' }, { Text: 'b' }], 5e7), -1, 'untimed lyrics have no line sung');
});

test('timed lines are seek buttons, untimed lyrics are plain lines', () => {
    const { fns } = sandbox({ get: async () => null });
    assert.equal(fns.isSyncedLyrics([{ Text: 'a' }, { Text: 'b', Start: 1e7 }]), true);
    assert.equal(fns.isSyncedLyrics([{ Text: 'a' }]), false);
    const synced = fns.lyricsLinesHtml([{ Text: '<a>', Start: 1e7 }, { Text: 'x' }]);
    assert.equal(
        synced,
        '<button type="button" class="jf-lyric-line" data-action="seek-lyric" data-start="10000000" data-lyric-index="0">&lt;a&gt;</button>'
        + '<p class="jf-lyric-line jf-lyric-plain">x</p>',
    );
    const plain = fns.lyricsLinesHtml([{ Text: 'a' }, { Text: 'b' }]);
    assert.doesNotMatch(plain, /<button/, 'unsynced lines are not clickable');
    assert.equal((plain.match(/<p /g) || []).length, 2);
});

test('the panel loads the playing song once, and a song without lyrics says so', async () => {
    const calls = [];
    const api = {
        get: async url => {
            calls.push(url);
            if (url.includes('/nolyrics/')) throw Object.assign(new Error('Not found'), { status: 404 });
            return { Metadata: { IsSynced: false }, Lyrics: [{ Text: 'hello' }] };
        },
    };
    const { fns, $ } = sandbox(api);
    fns.setLyricsTrack('abc', true);
    assert.equal($('now-playing-lyrics').hidden, false, 'shown for a song with lyrics');
    fns.toggleLyrics(true);
    await settle();
    assert.deepEqual(calls, ['/Audio/abc/Lyrics']);
    assert.equal($('lyrics-panel').hidden, false);
    assert.match($('lyrics-body').innerHTML, /hello/);
    fns.setLyricsTrack('abc', true);
    fns.toggleLyrics(true);
    await settle();
    assert.equal(calls.length, 1, 'the same song is not fetched again');
    fns.setLyricsTrack('nolyrics');
    await settle();
    assert.deepEqual(calls, ['/Audio/abc/Lyrics', '/Audio/nolyrics/Lyrics'], 'a new song is fetched');
    assert.match($('lyrics-body').innerHTML, /player\.no_lyrics/);
});

test('the lyrics button hides for a song the server says has none, and closes with the song', async () => {
    const { fns, $ } = sandbox({ get: async () => ({ Lyrics: [] }) });
    fns.setLyricsTrack('xyz', false);
    assert.equal($('now-playing-lyrics').hidden, true);
    fns.setLyricsTrack('xyz2', undefined);
    assert.equal($('now-playing-lyrics').hidden, false, 'without HasLyrics the button tries the lyrics');
    fns.toggleLyrics(true);
    await settle();
    fns.setLyricsTrack(null);
    assert.equal($('now-playing-lyrics').hidden, true);
    assert.equal($('lyrics-panel').hidden, true, 'no song, no panel');
    fns.toggleLyrics(true);
    assert.equal($('lyrics-panel').hidden, true, 'the panel does not open without a song');
});

test('highlightLyrics marks the line sung in timed lyrics and scrolls it into view, and ignores untimed ones', async () => {
    const synced = { get: async () => ({ Lyrics: [{ Text: 'one', Start: 0 }, { Text: 'two', Start: 10e7 }] }) };
    const { fns, $ } = sandbox(synced);
    fns.setLyricsTrack('s1');
    fns.toggleLyrics(true);
    await settle();
    const lines = [fakeLine(), fakeLine()];
    const body = $('lyrics-body');
    body.querySelectorAll = () => lines.filter(l => l.classes.has('active'));
    body.querySelector = sel => lines[Number(sel.match(/\d+/)[0])];
    fns.highlightLyrics(0);
    assert.deepEqual([...lines[0].classes], ['active']);
    assert.equal(lines[0].scrolls, 1);
    fns.highlightLyrics(5e7);
    assert.equal(lines[0].scrolls, 1, 'the same line is not scrolled again');
    fns.highlightLyrics(10e7);
    assert.equal(lines[0].classes.has('active'), false);
    assert.deepEqual([...lines[1].classes], ['active']);
    assert.equal(lines[1].scrolls, 1);

    const plain = { get: async () => ({ Lyrics: [{ Text: 'no timing' }] }) };
    const other = sandbox(plain);
    let queried = 0;
    other.fns.setLyricsTrack('u1');
    other.fns.toggleLyrics(true);
    await settle();
    other.$('lyrics-body').querySelector = () => { queried += 1; return null; };
    other.fns.highlightLyrics(10e7);
    assert.equal(queried, 0, 'untimed lyrics have nothing to highlight');
});

test('player.js, router.js, app.js, index.html and the styles wire the lyrics in', () => {
    assert.ok(player.includes("import { setLyricsTrack, highlightLyrics } from './lyrics.js';"));
    assert.ok(player.includes('setLyricsTrack(audio ? session.itemId : null, item?.HasLyrics);'));
    assert.ok(player.includes('highlightLyrics(pos * 1e7);'));
    assert.ok(router.includes("'seek-lyric': el => seekToTicks(Number(el.dataset.start)),"));
    assert.ok(app.includes("import { toggleLyrics } from './modules/lyrics.js';"));
    assert.match(app, /showTriggerFields, toggleLyrics,/);
    assert.ok(html.includes('id="now-playing-lyrics"'));
    assert.ok(html.includes('onclick="toggleLyrics()"'));
    assert.ok(html.includes('id="lyrics-panel"') && html.includes('id="lyrics-body"'));
    assert.ok(css.includes('.jf-lyric-line.active'));
    assert.equal(en.player.lyrics, 'Lyrics');
    assert.equal(en.player.no_lyrics, 'No lyrics');
});
