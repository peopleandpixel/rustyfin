// The web player's controls beyond play and seek: trickplay thumbnails over the seek bar, skip intro and
// outro, Jellyfin Web's keyboard shortcuts, the Media Session (OS media keys, lock screen) and
// picture-in-picture. Runs player.js's functions in vm sandboxes and checks the markup as text.
// Usage: node r3player_controls.mjs (run by api_tests.rs r3player_controls).
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
// A top-level `const NAME = ...;` statement, one line or several
function decl(name) {
    const m = new RegExp(`^const ${name} = [\\s\\S]*?;$`, 'm').exec(playerJs);
    assert.ok(m, `player.js declares ${name}`);
    return m[0];
}

function style() { return {}; }
const element = (extra = {}) => ({
    hidden: false, disabled: false, textContent: '', title: '', style: style(), value: '',
    classList: { set: new Set(), add(c) { this.set.add(c); }, remove(c) { this.set.delete(c); }, contains(c) { return this.set.has(c); } },
    ...extra,
});

// A sandbox with the named player.js functions and the given globals; elements are made on first use
function sandbox(heads, globals = {}, prelude = '') {
    const elements = {};
    const timers = [];
    const context = vm.createContext({
        console: { warn: () => {} },
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        $: id => (elements[id] ||= element()),
        formatDurationSeconds: s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`,
        setNativeNowPlaying: () => {},
        setTimeout: fn => { timers.push(fn); return timers.length; },
        clearTimeout: () => {},
        API: { base: 'http://srv', token: 'tok', imageUrl: (id, type, o) => `/Items/${id}/Images/${type}?maxHeight=${o.maxHeight}&tag=${o.tag}` },
        ...globals,
    });
    vm.runInContext(`var activePlayback = null; var episodeNav = null; ${prelude}\n${heads.map(slice).join('\n')}
        this.setActive = s => { activePlayback = s; }; this.setNav = n => { episodeNav = n; };`, context);
    return { context, elements, timers };
}

// --- Trickplay ---
const TRICKPLAY = ['function trickplayInfo(', 'function trickplayThumb(', 'function showTrickplay(', 'function hideTrickplay(', 'function hoverSeek(',
    'function chapterStart(', 'function chapterAt('];
const sets = {
    160: { Width: 160, Height: 90, TileWidth: 10, TileHeight: 10, ThumbnailCount: 250, Interval: 10000 },
    320: { Width: 320, Height: 180, TileWidth: 10, TileHeight: 10, ThumbnailCount: 250, Interval: 10000 },
};

test('trickplay picks the widest tile set up to a fifth of the screen for the playing source', () => {
    const { context } = sandbox(TRICKPLAY, { screen: { width: 1920 }, devicePixelRatio: 1 });
    const item = { Id: 'i1', Trickplay: { src1: sets } };
    assert.equal(context.trickplayInfo(item, 'src1').Width, 320);
    // A source with no sets of its own falls back to the item id's
    assert.equal(context.trickplayInfo({ Id: 'i1', Trickplay: { i1: sets } }, 'other').Width, 320);
    const small = sandbox(TRICKPLAY, { screen: { width: 1000 }, devicePixelRatio: 1 }).context;
    assert.equal(small.trickplayInfo(item, 'src1').Width, 160);
    const tiny = sandbox(TRICKPLAY, { screen: { width: 400 }, devicePixelRatio: 1 }).context;
    assert.equal(tiny.trickplayInfo(item, 'src1').Width, 160, 'nothing fits: the narrowest');
    assert.equal(context.trickplayInfo({ Id: 'i1' }, 'src1'), null);
    assert.equal(context.trickplayInfo(null, 'src1'), null);
});

test('a thumbnail comes from its tile image with the source and api_key, at its place in the grid', () => {
    const { context } = sandbox(TRICKPLAY);
    const info = sets[320];
    // 1234 s / 10 s = thumbnail 123: tile 1, place 23 = column 3, row 2
    const thumb = context.trickplayThumb(info, 'i1', 'src1', 1234);
    assert.equal(thumb.url, 'http://srv/Videos/i1/Trickplay/320/1.jpg?MediaSourceId=src1&api_key=tok');
    assert.deepEqual([thumb.x, thumb.y, thumb.width, thumb.height], [960, 360, 320, 180]);
    // Past the last thumbnail it stays on the last (249: tile 2, place 49)
    assert.equal(context.trickplayThumb(info, 'i1', 'src1', 99999).url, 'http://srv/Videos/i1/Trickplay/320/2.jpg?MediaSourceId=src1&api_key=tok');
    assert.equal(context.trickplayThumb(null, 'i1', 'src1', 10), null);
});

test('hovering or dragging shows the thumbnail and time over the bar; without trickplay only the time', () => {
    const box = sandbox(TRICKPLAY);
    const session = { itemId: 'i1', mediaSourceId: 'src1', trickplay: sets[160], total: () => 2000 };
    box.context.setActive(session);
    // Mouse or finger at a quarter of a 400 px bar that starts 50 px in (the position comes from clientX,
    // since offsetX is relative to whatever child the pointer is over)
    box.context.hoverSeek({ currentTarget: { getBoundingClientRect: () => ({ left: 50, width: 400 }) }, clientX: 150, offsetX: 7 });
    const { elements } = box;
    assert.equal(elements['player-trickplay'].hidden, false);
    assert.equal(elements['player-trickplay'].style.left, '25%');
    assert.equal(elements['player-trickplay-time'].textContent, '8:20');
    assert.equal(elements['player-trickplay-img'].hidden, false);
    assert.equal(elements['player-trickplay-img'].style.backgroundImage, 'url("http://srv/Videos/i1/Trickplay/160/0.jpg?MediaSourceId=src1&api_key=tok")');
    assert.equal(elements['player-trickplay-img'].style.backgroundPosition, '-0px -450px');
    assert.equal(elements['player-trickplay-img'].style.width, '160px');
    box.context.hideTrickplay();
    assert.equal(elements['player-trickplay'].hidden, true);

    session.trickplay = null;
    box.context.showTrickplay(50);
    assert.equal(elements['player-trickplay'].hidden, false);
    assert.equal(elements['player-trickplay-img'].hidden, true, 'no trickplay: time only');
    assert.equal(elements['player-trickplay-time'].textContent, '16:40');
});

test('the seek bar wires hover, touch and drag to the preview; playItem reads the item\'s trickplay', () => {
    const seek = /<input id="player-seek"[^>]*>/.exec(html)[0];
    assert.match(seek, /oninput="previewSeek\(this\.value\)"/);
    assert.match(seek, /onpointerdown="hoverSeek\(event\)"/);
    assert.match(seek, /onpointermove="hoverSeek\(event\)"/);
    assert.match(seek, /onpointerleave="if \(!seekDragging\) hideTrickplay\(\)"/);
    assert.match(html, /<div id="player-trickplay"[^>]*\bhidden>/);
    assert.ok(html.includes('id="player-trickplay-img"') && html.includes('id="player-trickplay-time"'));
    assert.match(slice('function previewSeek('), /showTrickplay\(Number\(percent\)\)/);
    assert.match(slice('function seekVideo('), /hideTrickplay\(\)/);
    assert.match(slice('async function playItem('), /session\.trickplay = trickplayInfo\(item, session\.mediaSourceId\)/);
    assert.match(read('app.js'), /hoverSeek, hideTrickplay/);
});

// --- Skip intro and outro ---
const SKIP = ['function skippableSegment(', 'async function checkSkipSegment(', 'function showSkipSegment(', 'function hideSkipSegment(', 'function skipSegment(',
    'function segmentTypeName('];
// Jellyfin Web's default actions: ask to skip intros and outros, nothing for the rest (playerprefs.js)
const SKIP_PRELUDE = `${decl('SKIP_PROMPT_MS')} let skipPrompt = null; ${decl('segmentStart')} ${decl('segmentEnd')}
    const SEGMENT_TYPES = ['Intro', 'Outro', 'Recap', 'Preview', 'Commercial'];
    const segmentAction = type => (['Intro', 'Outro'].includes(type) ? 'AskToSkip' : 'None');
    function showPlayerOsd() {}`;
const seg = (Type, start, end) => ({ Type, StartTicks: start * 1e7, EndTicks: end * 1e7 });

test('an intro or outro of 3 s or more is offered; an outro to the end with a next item is left to the card', () => {
    const { context } = sandbox(SKIP, {}, SKIP_PRELUDE);
    const segments = [seg('Intro', 10, 70), seg('Recap', 0, 10), seg('Outro', 1400, 1500), seg('Intro', 100, 102)];
    assert.equal(context.skippableSegment(segments, 30, 1500, false).Type, 'Intro');
    assert.equal(context.skippableSegment(segments, 5, 1500, false), null, 'recaps are not asked');
    assert.equal(context.skippableSegment(segments, 70, 1500, false), null, 'the end is outside');
    assert.equal(context.skippableSegment(segments, 101, 1500, false), null, 'under 3 s');
    assert.equal(context.skippableSegment(segments, 1450, 1500, false).Type, 'Outro');
    assert.equal(context.skippableSegment(segments, 1450, 1500, true), null);
});

test('the Skip button names the segment, expires after 8 s and seeks to its end', async () => {
    const seeks = [];
    const box = sandbox(SKIP, { seekTo: (s, to) => seeks.push(to) }, SKIP_PRELUDE);
    let pos = 30;
    const session = { streamType: 'Videos', segmentsLoad: Promise.resolve([seg('Intro', 10, 70)]), position: () => pos, total: () => 1500 };
    box.context.setActive(session);
    await box.context.checkSkipSegment(session);
    const button = box.elements['player-skip-segment'];
    assert.equal(button.hidden, false);
    assert.equal(button.textContent, 'player.skip_segment|{"type":"player.segment_intro"}');
    box.timers[0]();
    assert.ok(button.classList.contains('jf-skip-expired'), 'fades with the bars after 8 s');
    box.context.skipSegment();
    assert.deepEqual(seeks, [70]);
    assert.equal(button.hidden, true);
    // Leaving the segment hides it
    await box.context.checkSkipSegment(session);
    pos = 80;
    await box.context.checkSkipSegment(session);
    assert.equal(button.hidden, true);
    assert.match(read('input.css'), /\.jf-player-idle \.jf-skip-segment\.jf-skip-expired/);
    assert.match(html, /<button id="player-skip-segment"[^>]*onclick="skipSegment\(\)" hidden>/);
});

// --- Keyboard shortcuts ---
test('the keys map to Jellyfin Web\'s player actions', () => {
    const calls = [];
    const log = name => (...args) => calls.push([name, ...args]);
    const { context } = sandbox(['function playerKeyAction('], {
        seekPercent: log('percent'), stepPlaybackRate: log('rate'), seekFrames: log('frames'), playAdjacentEpisode: log('item'),
        stepVolume: log('volume'), skipTime: log('skip'), togglePlayPause: log('play'), toggleFullscreen: log('full'),
        toggleMute: log('mute'), nextChapter: log('nextChapter'), previousChapter: log('prevChapter'),
    });
    const press = (key, code = '', shiftKey = false) => {
        calls.length = 0;
        const action = context.playerKeyAction({ key, code, shiftKey });
        if (action) action();
        return calls[0] || null;
    };
    assert.deepEqual(press('7', 'Digit7'), ['percent', 70]);
    assert.deepEqual(press('0', 'Numpad0'), ['percent', 0]);
    // AZERTY: the digit row types &é"… unshifted and seeks by position; with Shift it is left alone
    assert.deepEqual(press('é', 'Digit2'), ['percent', 20]);
    assert.equal(press('2', 'Digit2', true), null);
    assert.deepEqual(press('k', 'KeyK'), ['play']);
    assert.deepEqual(press('ArrowUp', 'ArrowUp'), ['volume', 2]);
    assert.deepEqual(press('ArrowDown', 'ArrowDown'), ['volume', -2]);
    assert.deepEqual(press('l', 'KeyL'), ['skip', 30]);
    assert.deepEqual(press('ArrowRight', 'ArrowRight'), ['skip', 30]);
    assert.deepEqual(press('j', 'KeyJ'), ['skip', -10]);
    assert.deepEqual(press('ArrowLeft', 'ArrowLeft'), ['skip', -10]);
    assert.deepEqual(press('F', 'KeyF'), ['full']);
    assert.deepEqual(press('m', 'KeyM'), ['mute']);
    assert.deepEqual(press('>', 'Period', true), ['rate', 1]);
    assert.deepEqual(press('<', 'Comma', true), ['rate', -1]);
    assert.deepEqual(press('<', 'IntlBackslash'), ['rate', -1], 'a layout with its own < key');
    assert.deepEqual(press('.', 'Period'), ['frames', 1]);
    assert.deepEqual(press(',', 'Comma'), ['frames', -1]);
    assert.deepEqual(press('N', 'KeyN', true), ['item', 1]);
    assert.deepEqual(press('P', 'KeyP', true), ['item', -1]);
    assert.equal(press('n', 'KeyN'), null, 'n without Shift does nothing');
    assert.equal(press('ArrowLeft', 'ArrowLeft', true), null, 'Shift turns the rest off');
    assert.deepEqual(press('Home', 'Home'), ['percent', 0]);
    assert.deepEqual(press('End', 'End'), ['percent', 100]);
    assert.deepEqual(press('PageUp', 'PageUp'), ['nextChapter']);
    assert.deepEqual(press('PageDown', 'PageDown'), ['prevChapter']);
    assert.equal(press('x', 'KeyX'), null);
});

test('the handler leaves Ctrl, Alt and Meta keys, and a control focused by keyboard, alone; Escape closes', () => {
    const start = playerJs.indexOf('// Global Keyboard Shortcuts');
    const handler = playerJs.slice(start, playerJs.indexOf('\n});\n', start) + 5);
    let listener = null;
    const calls = [];
    const context = vm.createContext({
        Element: class {},
        document: { addEventListener: (type, fn) => { if (type === 'keydown') listener = fn; } },
        $: () => ({ open: true }),
        closePlayer: () => calls.push('close'),
        togglePlayPause: () => calls.push('play'),
        playerKeyAction: () => () => calls.push('action'),
    });
    class Element {
        constructor(match, keyboard = true) { this.match = match; this.keyboard = keyboard; }
        matches(selector) { return selector === ':focus-visible' && this.keyboard; }
        closest(selector) { return this.match && selector.includes(this.match) ? this : null; }
    }
    context.Element = Element;
    vm.runInContext(handler, context);
    const press = (e) => { const ev = { code: '', target: null, preventDefault() { this.prevented = true; }, ...e }; listener(ev); return ev; };
    for (const mod of ['ctrlKey', 'altKey', 'metaKey']) assert.equal(press({ key: 'f', [mod]: true }).prevented, undefined);
    assert.deepEqual(calls, []);
    // A menu or button focused by keyboard keeps its arrows, digits and letters
    for (const match of ['select', 'button']) {
        for (const key of ['ArrowDown', '5', 'm']) assert.equal(press({ key, target: new Element(match) }).prevented, undefined);
    }
    assert.deepEqual(calls, []);
    // A text field keeps its keys even after a click; a menu or button after a click, and the sliders, don't
    assert.equal(press({ key: 'ArrowDown', target: new Element('select', false) }).prevented, true);
    assert.equal(press({ key: 'k', target: new Element('input', false) }).prevented, undefined);
    assert.equal(press({ key: 'ArrowDown', target: new Element('button', false) }).prevented, true);
    assert.equal(press({ key: 'f' }).prevented, true);
    press({ key: 'Escape', target: new Element('select') });
    assert.deepEqual(calls, ['action', 'action', 'action', 'close']);
});

test('volume steps on Jellyfin\'s cube-root scale; speed steps through its rates and keeps the menu true', () => {
    const video = { volume: 1, playbackRate: 1, defaultPlaybackRate: 1 };
    const volumes = [];
    const box = sandbox(['function stepVolume(', 'function setPlaybackRate(', 'function stepPlaybackRate('], {
        setVolume: v => { volumes.push(v); video.volume = v; },
    }, decl('PLAYBACK_RATES'));
    box.elements['video-player'] = video;
    box.context.stepVolume(-2);
    assert.ok(Math.abs(volumes[0] - 0.98 ** 3) < 1e-9);
    video.volume = 1;
    box.context.stepVolume(2);
    assert.equal(volumes[1], 1, 'capped at 100');
    box.context.stepPlaybackRate(1);
    assert.equal(video.playbackRate, 1.25);
    assert.equal(video.defaultPlaybackRate, 1.25, 'the next item keeps the speed');
    assert.equal(box.elements['player-speed'].value, '1.25');
    video.playbackRate = 4;
    box.context.stepPlaybackRate(1);
    assert.equal(video.playbackRate, 4);
    video.playbackRate = 0.5;
    box.context.stepPlaybackRate(-1);
    assert.equal(video.playbackRate, 0.5);
    // Every rate is in the speed menu
    for (const rate of [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.5, 3, 3.5, 4]) assert.ok(html.includes(`<option value="${rate}"`), `menu has ${rate}`);
});

test('frames step only while paused; chapters jump forward, and back past the last 10 s, else to the next item', () => {
    const seeks = [];
    const items = [];
    const video = { paused: false };
    const box = sandbox(['function seekFrames(', 'function chapterStarts(', 'function nextChapter(', 'function previousChapter(',
        'function isStreamType('], { seekTo: (s, to) => seeks.push(to), playAdjacentEpisode: step => items.push(step) });
    box.elements['video-player'] = video;
    let pos = 100;
    const session = {
        streams: [{ CodecType: 'video', ReferenceFrameRate: 25 }],
        chapters: [{ StartPositionTicks: 0 }, { StartPositionTicks: 600e7 }, { StartPositionTicks: 95e7 }],
        position: () => pos,
    };
    box.context.setActive(session);
    box.context.seekFrames(1);
    assert.deepEqual(seeks, [], 'playing: no frame step');
    video.paused = true;
    box.context.seekFrames(-1);
    assert.deepEqual(seeks, [100 - 1 / 25]);
    seeks.length = 0;
    box.context.nextChapter();
    box.context.previousChapter(); // 100 - 10 = 90: the chapter at 0, not the one at 95
    pos = 700;
    box.context.nextChapter();
    assert.deepEqual(seeks, [600, 0]);
    assert.deepEqual(items, [1], 'past the last chapter: the next item');
});

// --- Media Session ---
test('the Media Session shows the item and binds Jellyfin Web\'s actions; previous and next follow the item', () => {
    const handlers = {};
    const mediaSession = { metadata: undefined, playbackState: '', setActionHandler: (a, fn) => { handlers[a] = fn; }, setPositionState(s) { this.position = s; } };
    const calls = [];
    const video = { play: () => calls.push('play'), pause: () => calls.push('pause'), playbackRate: 1.5 };
    const box = sandbox(['function mediaSessionMetadata(', 'function updateMediaSession(', 'function setMediaPlaybackState(',
        'function updateMediaPosition(', 'function clearMediaSession(', 'function setMediaSessionHandler(', 'function bindMediaSession(',
        'function episodeNumberLabel(', 'function itemPicture(', 'function itemArtist('], {
        navigator: { mediaSession },
        MediaMetadata: class { constructor(m) { Object.assign(this, m); } },
        stopNowPlaying: () => calls.push('close'),
        skipTime: d => calls.push(['skip', d]),
        seekTo: (s, to) => calls.push(['seek', to]),
    }, `${decl('hasMediaSession')} ${decl('ARTWORK_SIZES')} ${decl('MEDIA_SESSION_ACTIONS')}`);
    box.elements['video-player'] = video;
    const { context } = box;

    const episode = { Id: 'e', Type: 'Episode', Name: 'Pilot', SeriesName: 'Show', ParentIndexNumber: 1, IndexNumber: 2, SeriesId: 's', SeriesPrimaryImageTag: 'st' };
    context.updateMediaSession({}, episode, '');
    assert.equal(mediaSession.metadata.title, 'Pilot');
    assert.equal(mediaSession.metadata.artist, 'Show · player.episode_number|{"season":1,"episode":2}');
    assert.equal(mediaSession.metadata.artwork.length, 6);
    assert.deepEqual(JSON.parse(JSON.stringify(mediaSession.metadata.artwork[0])), { src: '/Items/s/Images/Primary?maxHeight=512&tag=st', sizes: '512x512' });
    const song = { Id: 'a', Type: 'Audio', Name: 'Song', Artists: ['A', 'B'], Album: 'LP', AlbumId: 'al', AlbumPrimaryImageTag: 'at' };
    const meta = context.mediaSessionMetadata(song, '');
    assert.deepEqual([meta.title, meta.artist, meta.album], ['Song', 'A, B', 'LP']);
    assert.match(meta.artwork[5].src, /^\/Items\/al\/.*maxHeight=96/);
    assert.equal(context.mediaSessionMetadata({ Name: 'Film' }, '').artwork.length, 0);

    context.bindMediaSession();
    handlers.play();
    handlers.pause();
    handlers.seekbackward({});
    handlers.seekforward({ seekOffset: 5 });
    handlers.stop();
    context.setActive({});
    handlers.seekto({ seekTime: 42 });
    assert.deepEqual(calls, ['play', 'pause', ['skip', -10], ['skip', 5], 'close', ['seek', 42]]);

    context.updateMediaPosition({ total: () => 100, position: () => 120 });
    assert.deepEqual({ ...mediaSession.position }, { duration: 100, position: 100, playbackRate: 1.5 });
    context.setMediaPlaybackState('playing');
    assert.equal(mediaSession.playbackState, 'playing');
    context.clearMediaSession();
    assert.equal(mediaSession.metadata, null);
    assert.equal(mediaSession.playbackState, 'none');
    for (const action of ['play', 'pause', 'stop', 'seekbackward', 'seekforward', 'seekto', 'previoustrack', 'nexttrack']) {
        assert.equal(handlers[action], null, `${action} is cleared on close`);
    }
    // The next item binds them again
    context.updateMediaSession({}, episode, '');
    assert.equal(typeof handlers.play, 'function');

    // previous/next track handlers come from updateEpisodeButtons, null where there is no neighbour
    assert.match(slice('function updateEpisodeButtons('), /setMediaSessionHandler\(step < 0 \? 'previoustrack' : 'nexttrack', episode \? \(\) => playAdjacentEpisode\(step\) : null\)/);
    const play = slice('async function playItem(');
    assert.match(play, /updateMediaSession\(session, item,/);
    assert.match(play, /updateMediaPosition\(session\)/);
    assert.match(slice('function onPlayerClosed('), /clearMediaSession\(\)/);
});

test('a browser without the Media Session is left alone', () => {
    const { context } = sandbox(['function updateMediaSession(', 'function clearMediaSession(', 'function setMediaSessionHandler('],
        { navigator: {} }, `${decl('hasMediaSession')} ${decl('MEDIA_SESSION_ACTIONS')}`);
    context.updateMediaSession({}, { Name: 'x' }, '');
    context.clearMediaSession();
    context.setMediaSessionHandler('play', () => {});
});

// --- Picture-in-picture ---
test('picture-in-picture: the button shows for videos where supported and toggles the float', async () => {
    const calls = [];
    const document = { pictureInPictureEnabled: true, pictureInPictureElement: null, exitPictureInPicture: () => { calls.push('exit'); return Promise.resolve(); } };
    const video = { requestPictureInPicture: () => { calls.push('enter'); return Promise.resolve(); } };
    const box = sandbox(['function pipSupported(', 'function updatePipButton(', 'function togglePictureInPicture('], { document });
    box.elements['video-player'] = video;
    box.context.updatePipButton({ streamType: 'Videos' });
    assert.equal(box.elements['player-btn-pip'].hidden, false);
    box.context.updatePipButton({ streamType: 'Audio' });
    assert.equal(box.elements['player-btn-pip'].hidden, true);
    box.context.togglePictureInPicture();
    document.pictureInPictureElement = video;
    box.context.togglePictureInPicture();
    assert.deepEqual(calls, ['enter', 'exit']);

    // Safari's presentation mode
    const safari = { webkitPresentationMode: 'inline', webkitSetPresentationMode(m) { this.webkitPresentationMode = m; } };
    const s = sandbox(['function pipSupported(', 'function updatePipButton(', 'function togglePictureInPicture('], { document: {} });
    s.elements['video-player'] = safari;
    s.context.updatePipButton({ streamType: 'Videos' });
    assert.equal(s.elements['player-btn-pip'].hidden, false);
    s.context.togglePictureInPicture();
    assert.equal(safari.webkitPresentationMode, 'picture-in-picture');
    // No support: no button
    const none = sandbox(['function pipSupported(', 'function updatePipButton('], { document: {} });
    none.elements['video-player'] = {};
    none.context.updatePipButton({ streamType: 'Videos' });
    assert.equal(none.elements['player-btn-pip'].hidden, true);

    assert.match(html, /<button id="player-btn-pip"[^>]*onclick="togglePictureInPicture\(\)" hidden>/);
    assert.match(slice('function onPlayerClosed('), /document\.exitPictureInPicture\(\)/);
    assert.match(read('icons.js'), /PictureInPicture2,[\s\S]*PictureInPicture2,/);
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`)).player;
        for (const key of ['picture_in_picture', 'skip_segment', 'segment_intro', 'segment_outro', 'speed_tooltip']) assert.ok(strings[key], `${lang} has player.${key}`);
    }
});
