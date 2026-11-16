// The player's remembered tracks and keys: the next episode starts with the last one's audio and subtitle
// tracks (by language, a same-titled track first, as Jellyfin Web's autoSetNextTracks) only while the user
// keeps RememberAudioSelections and RememberSubtitleSelections on; a remembered subtitle is not used while
// SubtitleMode is None; C and S toggle subtitles; a focused menu or text field keeps every key.
// Usage: node r3prefs_player.mjs (run by api_tests.rs r3prefs_player).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const prefs = await import(new URL('modules/playerprefs.js', src));
const playerJs = fs.readFileSync(new URL('modules/player.js', src), 'utf8');

function slice(head) {
    const start = playerJs.indexOf(head);
    assert.ok(start >= 0, `player.js has ${head}`);
    return playerJs.slice(start, playerJs.indexOf('\n}\n', start) + 3);
}
const plain = v => JSON.parse(JSON.stringify(v));

function sandbox(heads, globals = {}) {
    const elements = {};
    const context = vm.createContext({
        $: id => (elements[id] ||= { value: '' }),
        preferredSubtitleIndex: prefs.defaultSubtitleIndex,
        defaultSubtitleIndex: prefs.defaultSubtitleIndex,
        ...globals,
    });
    vm.runInContext(`var activePlayback = null; ${heads.map(slice).join('\n')}
        this.setActive = s => { activePlayback = s; };`, context);
    return { context, elements };
}

const TRACKS = ['function isStreamType(', 'function findStream(', 'function episodeTrackCarry(', 'function carriedTracks('];
const s = (Index, CodecType, extra = {}) => ({ Index, CodecType, ...extra });
// Episode 1: English main and commentary audio, English full and German subtitles
const EPISODE1 = [
    s(0, 'video'), s(1, 'audio', { Language: 'eng', IsDefault: true }), s(2, 'audio', { Language: 'eng', Title: 'Commentary' }),
    s(3, 'audio', { Language: 'ger' }), s(4, 'subtitle', { Language: 'eng' }), s(5, 'subtitle', { Language: 'ger', Title: 'Full' }),
];
// Episode 2 holds the same tracks in another order
const EPISODE2 = [
    s(0, 'video'), s(1, 'audio', { Language: 'ger' }), s(2, 'audio', { Language: 'eng', IsDefault: true }),
    s(3, 'audio', { Language: 'eng', Title: 'Commentary' }), s(4, 'subtitle', { Language: 'ger', Title: 'Signs' }),
    s(5, 'subtitle', { Language: 'ger', Title: 'Full' }), s(6, 'subtitle', { Language: 'eng' }),
];

test('the next episode starts with the matching tracks, a same-titled one first, not the same index', () => {
    const { context } = sandbox(TRACKS);
    const carry = context.episodeTrackCarry({ streams: EPISODE1, audioIndex: 2, subtitleIndex: 5 });
    assert.deepEqual(plain(carry), { audioLanguage: 'eng', audioTitle: 'Commentary', subtitleLanguage: 'ger', subtitleTitle: 'Full', subtitleForced: false, subtitlesOff: false });
    const on = prefs.trackPrefs({});
    assert.deepEqual(plain(context.carriedTracks(EPISODE2, carry, 2, -1, false, on)), { audioIndex: 3, subtitleIndex: 5 });
    // German audio with no title goes to the German track; the server's remembered subtitle stays when none matches
    const ger = context.episodeTrackCarry({ streams: EPISODE1, audioIndex: 3, subtitleIndex: 4 });
    assert.deepEqual(plain(context.carriedTracks(EPISODE2, { ...ger, subtitleLanguage: 'fre' }, 2, 6, true, on)), { audioIndex: 1, subtitleIndex: 6 });
    // An undetermined language matches by title alone, and no title means no match
    const und = [s(1, 'audio', { Language: 'und' }), s(2, 'audio', { Language: 'und', Title: 'Director' })];
    assert.equal(context.carriedTracks(und, { audioLanguage: 'und', audioTitle: 'Director' }, 1, -1, false, on).audioIndex, 2);
    assert.equal(context.carriedTracks(und, { audioLanguage: 'und', audioTitle: '' }, 2, -1, false, on).audioIndex, 2);
});

test('remembering turned off leaves that kind of track to the next episode\'s defaults', () => {
    const { context } = sandbox(TRACKS);
    const carry = context.episodeTrackCarry({ streams: EPISODE1, audioIndex: 2, subtitleIndex: 5 });
    const noAudio = prefs.trackPrefs({ RememberAudioSelections: false });
    assert.deepEqual(plain(context.carriedTracks(EPISODE2, carry, 2, -1, false, noAudio)), { audioIndex: 2, subtitleIndex: 5 });
    const noSubs = prefs.trackPrefs({ RememberSubtitleSelections: false });
    assert.deepEqual(plain(context.carriedTracks(EPISODE2, carry, 2, -1, true, noSubs)), { audioIndex: 3, subtitleIndex: -1 });
    // Subtitles turned off carry over only while subtitle choices are remembered
    const off = context.episodeTrackCarry({ streams: EPISODE1, audioIndex: 1, subtitleIndex: -1 });
    assert.equal(context.carriedTracks(EPISODE2, off, 2, 6, false, prefs.trackPrefs({})).subtitleIndex, -1);
    assert.equal(context.carriedTracks(EPISODE2, off, 2, 6, false, noSubs).subtitleIndex, 6);
});

test('a remembered subtitle is not used while SubtitleMode is None; the remembered audio still is', () => {
    const source = { MediaStreams: EPISODE2, DefaultAudioStreamIndex: 3, DefaultSubtitleStreamIndex: 5 };
    assert.deepEqual(prefs.defaultTracks(source, prefs.trackPrefs({})), { audioIndex: 3, subtitleIndex: 5, subtitleFromServer: true });
    assert.deepEqual(prefs.defaultTracks(source, prefs.trackPrefs({ SubtitleMode: 'None' })), { audioIndex: 3, subtitleIndex: -1, subtitleFromServer: false });
});

test('C and S turn subtitles off, then back on with the last track, else the preferred language', () => {
    const changes = [];
    const { context, elements } = sandbox(['function isStreamType(', 'function findStream(', 'function toggleSubtitles('], {
        changeSubtitleTrack: v => { changes.push(v); context.active.subtitleIndex = Number(v); },
    });
    const session = { streamType: 'Videos', streams: EPISODE2, audioIndex: 2, subtitleIndex: 5, prefs: prefs.trackPrefs({ SubtitleLanguagePreference: 'eng' }) };
    context.active = session;
    context.setActive(session);
    context.toggleSubtitles();
    assert.deepEqual(changes, ['-1']);
    assert.equal(elements['player-subs'].value, '-1');
    context.toggleSubtitles();
    assert.deepEqual(changes, ['-1', '5']);
    // Nothing shown before: the preferred language's track, else the first
    session.subtitleIndex = -1;
    session.lastSubtitleIndex = undefined;
    context.toggleSubtitles();
    assert.equal(changes.at(-1), '6');
    session.subtitleIndex = -1;
    session.prefs = prefs.trackPrefs({ SubtitleLanguagePreference: 'jpn' });
    context.toggleSubtitles();
    assert.equal(changes.at(-1), '4');
    // No subtitles, or music: nothing
    changes.length = 0;
    context.setActive({ ...session, streams: [s(1, 'audio')] });
    context.toggleSubtitles();
    context.setActive({ ...session, streamType: 'Audio' });
    context.toggleSubtitles();
    assert.deepEqual(changes, []);
});

test('the keys: C and S toggle subtitles; Jellyfin Web\'s Shift+N and Shift+P stay', () => {
    const calls = [];
    const { context } = sandbox(['function playerKeyAction('], {
        toggleSubtitles: () => calls.push('subs'), playAdjacentEpisode: n => calls.push(['item', n]),
    });
    for (const key of ['c', 'C', 's']) context.playerKeyAction({ key, code: `Key${key.toUpperCase()}` })();
    assert.deepEqual(calls, ['subs', 'subs', 'subs']);
    assert.equal(context.playerKeyAction({ key: 'C', code: 'KeyC', shiftKey: true }), null);
});

test('a focused menu, text field or editable area keeps every key, focused by mouse or keyboard', () => {
    const start = playerJs.indexOf('// Global Keyboard Shortcuts');
    const handler = playerJs.slice(start, playerJs.indexOf('\n});\n', start) + 5);
    class Element {
        constructor(tag, keyboard) { this.tag = tag; this.keyboard = keyboard; }
        matches(selector) { return selector === ':focus-visible' && this.keyboard; }
        closest(selector) { return selector.split(', ').some(part => part.startsWith(this.tag)) ? this : null; }
    }
    let listener = null;
    const calls = [];
    const context = vm.createContext({
        Element,
        document: { addEventListener: (type, fn) => { if (type === 'keydown') listener = fn; } },
        $: () => ({ open: true }),
        closePlayer: () => calls.push('close'),
        togglePlayPause: () => calls.push('play'),
        playerKeyAction: () => () => calls.push('action'),
    });
    vm.runInContext(handler, context);
    const press = (key, target) => {
        const e = { key, code: key === ' ' ? 'Space' : '', target, prevented: false, preventDefault() { this.prevented = true; } };
        listener(e);
        return e.prevented;
    };
    for (const tag of ['select', 'textarea', 'input', '[contenteditable']) {
        for (const keyboard of [true, false]) {
            for (const key of [' ', 'ArrowUp', '5', 'c']) assert.equal(press(key, new Element(tag, keyboard)), false, `${tag} ${key}`);
        }
    }
    assert.deepEqual(calls, []);
    assert.equal(press('c', new Element('button', false)), true);
    assert.equal(press(' ', new Element('button', true)), false);
    assert.equal(press('Escape', new Element('select', true)), false);
    assert.deepEqual(calls, ['action', 'close']);
    // A menu chosen with the mouse lets go of focus, so the keys work again
    assert.match(playerJs, /if \(!e\.target\.matches\?\.\(':focus-visible'\)\) e\.target\.blur\?\.\(\);/);
});
