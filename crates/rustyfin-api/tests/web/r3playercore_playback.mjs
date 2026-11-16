// The player's Playback settings: the quality steps (Jellyfin Web's, bounded by the source) and their restart
// with MaxStreamingBitrate, the version picker that keeps the position and track languages, and the aspect ratio.
// Loads playerprefs.js as a module and runs player.js's functions in a vm sandbox.
// Usage: node r3playercore_playback.mjs (run by api_tests.rs r3playercore_playback).
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

const store = new Map();
globalThis.localStorage = {
    getItem: k => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: k => store.delete(k),
};
const prefs = await import(new URL('modules/playerprefs.js', src));

test('quality steps: Auto, the step above the source, then every step up to it', () => {
    assert.deepEqual(prefs.qualityOptions(4.4e6, 'h264'), [0, 6e6, 4e6, 3e6, 1.5e6, 720e3, 420e3]);
    assert.deepEqual(prefs.qualityOptions(0), [0, ...prefs.BITRATE_STEPS], 'unknown bitrate: every step');
    assert.deepEqual(prefs.qualityOptions(200e6), [0, ...prefs.BITRATE_STEPS]);
    assert.equal(prefs.qualityOptions(5e6, 'hevc')[1], 8e6, 'HEVC counts at 1.5 times');
    assert.equal(prefs.bitrateLabel(1.5e6), '1.5 Mbps');
    assert.equal(prefs.bitrateLabel(720e3), '720 kbps');
});

test('the quality and aspect ratio are the device\'s, under Jellyfin Web\'s keys', () => {
    store.clear();
    assert.equal(prefs.maxStreamingBitrate(), 0);
    prefs.setMaxStreamingBitrate(3e6);
    assert.equal(store.get('maxStreamingBitrate'), '3000000');
    assert.equal(prefs.maxStreamingBitrate(), 3e6);
    prefs.setMaxStreamingBitrate(0);
    assert.equal(store.has('maxStreamingBitrate'), false, 'Auto removes the limit');
    prefs.setAspectRatio('cover');
    assert.equal(prefs.aspectRatio(), 'cover');
    prefs.setAspectRatio('bogus');
    assert.equal(prefs.aspectRatio(), 'auto');
});

function sandbox() {
    const elements = {};
    const restarts = [];
    const context = vm.createContext({
        $: id => (elements[id] ||= { hidden: false, value: '', textContent: '', style: {}, replaceChildren(...o) { this.options = o; } }),
        t: k => k,
        Option: function Option(text, value) { this.text = text; this.value = String(value); },
        ...Object.fromEntries(['qualityOptions', 'bitrateLabel', 'maxStreamingBitrate', 'setMaxStreamingBitrate', 'aspectRatio',
            'setAspectRatio', 'defaultTracks'].map(n => [n, prefs[n]])),
        versionLabels: sources => sources.map(s => s.Name),
        trickplayInfo: (item, id) => `tiles-${id}`,
        clearTextSubtitle: () => {},
        renderTrackMenus: () => {},
        scheduleRestart: (session, seconds) => restarts.push(seconds),
        activePlayback: null,
    });
    vm.runInContext(`${['function isStreamType(', 'function findStream(', 'function defaultSubtitleIndex(', 'function episodeTrackCarry(',
        'function carriedTracks(', 'function sourceVideoBitrate(', 'function renderPlaybackSettings(', 'function updateQualityTag(',
        'function changeQuality(', 'function changeVersion(', 'function applyAspectRatio(', 'function changeAspectRatio('].map(slice).join('\n')}
        function preferredSubtitleIndex() { return -1; }
        this.fns = { renderPlaybackSettings, changeQuality, changeVersion, changeAspectRatio };
        this.setActive = s => { activePlayback = s; };`, context);
    return { context, elements, restarts };
}

const stream = (Index, CodecType, extra = {}) => ({ Index, CodecType, ...extra });
const SOURCES = [
    { Id: 'mkv', Name: '1080p', Bitrate: 4.8e6, MediaStreams: [stream(0, 'video', { BitRate: 4.4e6, Codec: 'h264' }),
        stream(1, 'audio', { Language: 'eng', IsDefault: true }), stream(2, 'audio', { Language: 'fre' }), stream(3, 'subtitle', { Language: 'fre' })] },
    { Id: 'mp4', Name: 'MP4', MediaStreams: [stream(0, 'video'), stream(1, 'audio', { Language: 'eng', IsDefault: true }),
        stream(2, 'audio', { Language: 'fre' }), stream(5, 'subtitle', { Language: 'fre' })] },
];
const session = () => ({
    streamType: 'Videos', sources: SOURCES, streams: SOURCES[0].MediaStreams, mediaSourceId: 'mkv', item: {}, prefs: prefs.trackPrefs({}),
    audioIndex: 2, subtitleIndex: 3, tracksChanged: false, position: () => 42,
});

test('the menu lists the source\'s steps and its versions', () => {
    store.clear();
    const { context, elements } = sandbox();
    const s = session();
    context.setActive(s);
    context.fns.renderPlaybackSettings(s);
    assert.deepEqual(elements['player-quality'].options.map(o => o.value), ['0', '6000000', '4000000', '3000000', '1500000', '720000', '420000']);
    assert.equal(elements['player-quality'].value, '0');
    assert.deepEqual(elements['player-version'].options.map(o => o.text), ['1080p', 'MP4']);
    assert.equal(elements['player-version-row'].hidden, false);
    context.fns.renderPlaybackSettings({ ...s, sources: [SOURCES[0]] });
    assert.equal(elements['player-version-row'].hidden, true, 'one version: no picker');
    context.fns.renderPlaybackSettings({ ...s, streamType: 'Audio' });
    assert.equal(elements['player-quality-row'].hidden, true, 'music has no quality steps');
});

test('a quality change is stored and restarts at the same point', () => {
    store.clear();
    const { context, elements, restarts } = sandbox();
    const s = session();
    context.setActive(s);
    context.fns.changeQuality('3000000');
    assert.equal(prefs.maxStreamingBitrate(), 3e6);
    assert.deepEqual(restarts, [42]);
    assert.equal(s.tracksChanged, true);
    assert.equal(elements['player-quality-tag'].textContent, '3 Mbps');
});

test('another version restarts at the same point with the same languages', () => {
    const { context, restarts } = sandbox();
    const s = session();
    context.setActive(s);
    context.fns.changeVersion('mp4');
    assert.equal(s.mediaSourceId, 'mp4');
    assert.equal(s.audioIndex, 2, 'French audio stays');
    assert.equal(s.subtitleIndex, 5, 'French subtitles stay');
    assert.equal(s.trickplay, 'tiles-mp4');
    assert.deepEqual(restarts, [42]);
    context.fns.changeVersion('mp4');
    assert.deepEqual(restarts, [42], 'the same version does nothing');
});

test('the aspect ratio sets the video\'s object-fit', () => {
    const { context, elements } = sandbox();
    context.fns.changeAspectRatio('fill');
    assert.equal(elements['video-player'].style.objectFit, 'fill');
    context.fns.changeAspectRatio('auto');
    assert.equal(elements['video-player'].style.objectFit, '');
});

test('the stream request carries the limit and keeps the chosen version', () => {
    const open = slice('async function openStream(');
    assert.match(open, /maxBitrate: maxStreamingBitrate\(\) \|\| AUTO_BITRATE/);
    assert.match(open, /session\.mediaSourceId = session\.mediaSourceId \|\| mediaSourceId/);
    for (const id of ['player-quality', 'player-version', 'player-aspect']) assert.match(html, new RegExp(`id="${id}"`));
});
