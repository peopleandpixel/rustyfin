// Music under the now-playing bar: a song plays without the full player, the bar shows its title, artists and
// art and follows the queue, closing the full player leaves the music playing, and the bar's × stops it.
// Runs player.js's functions in a vm sandbox and checks playItem's wiring as text.
// Usage: node r3playercore_music.mjs (run by api_tests.rs r3playercore_music).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const playerJs = fs.readFileSync(new URL('modules/player.js', src), 'utf8');
const html = fs.readFileSync(new URL('index.html', src), 'utf8');

function slice(head) {
    const start = playerJs.indexOf(head);
    assert.ok(start >= 0, `player.js has ${head}`);
    return playerJs.slice(start, playerJs.indexOf('\n}\n', start) + 3);
}

function sandbox(active = null) {
    const elements = {};
    const calls = [];
    const classes = new Set();
    const make = () => ({
        hidden: true, disabled: false, textContent: '', value: '', src: '', poster: '', open: false, attrs: {},
        removeAttribute(k) { this[k] = ''; },
        setAttribute(k, v) { this.attrs[k] = v; },
        close() { this.open = false; calls.push('close'); },
        showModal() { this.open = true; calls.push('showModal'); },
    });
    const context = vm.createContext({
        $: id => (elements[id] ||= make()),
        t: (k, o) => (o ? `${k}|${JSON.stringify(o)}` : k),
        document: { body: { classList: { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)) } }, fullscreenElement: null },
        API: { imageUrl: (id, type, o) => `/Items/${id}/Images/${type}?maxHeight=${o.maxHeight}&tag=${o.tag}` },
        formatDurationSeconds: s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`,
        seekDragging: false,
        activePlayback: active,
        episodeNav: null,
        playerBars: { reset: () => calls.push('bars reset') },
        stopPlayback: () => calls.push('stop'),
        clearQueue: () => calls.push('clear queue'),
        resetEpisodeNav: () => calls.push('reset nav'),
        clearMediaSession: () => calls.push('clear media session'),
        hideSkipSegment: () => {},
        hideTrickplay: () => {},
        togglePlaybackStats: () => {},
        closePlayerSettings: () => calls.push('close menus'),
        setMediaSessionHandler: () => {},
        setLyricsTrack: () => {},
        highlightLyrics: () => {},
    });
    vm.runInContext(`${['function episodeNumberLabel(', 'function itemPicture(', 'function itemArtist(', 'function updateNowPlaying(',
        'function updateNowPlayingProgress(', 'function openFullPlayer(', 'function stopNowPlaying(', 'function endNowPlaying(', 'function onPlayerClosed(',
        'function updateEpisodeButtons('].map(slice).join('\n')}
        this.fns = { itemPicture, updateNowPlaying, openFullPlayer, stopNowPlaying, onPlayerClosed, updateEpisodeButtons };
        this.setNav = n => { episodeNav = n; };`, context);
    return { context, elements, calls, classes };
}

const song = { Id: 's1', Type: 'Audio', Name: 'Movement 2', Artists: ['Detour Orchestra'], AlbumId: 'a1', AlbumPrimaryImageTag: 'tagA' };
const audioSession = { streamType: 'Audio', total: () => 60, position: () => 15 };

test('a song shows the bar with its title, artists, art and time; the full player gets the art', () => {
    const { context, elements, classes } = sandbox();
    context.fns.updateNowPlaying(audioSession, song);
    assert.equal(elements['now-playing'].hidden, false);
    assert.equal(elements['now-playing-title'].textContent, 'Movement 2');
    assert.equal(elements['now-playing-artist'].textContent, 'Detour Orchestra');
    assert.equal(elements['now-playing-img'].src, '/Items/a1/Images/Primary?maxHeight=96&tag=tagA');
    assert.equal(elements['video-player'].poster, '/Items/a1/Images/Primary?maxHeight=512&tag=tagA');
    assert.equal(elements['now-playing-seek'].value, 25);
    assert.equal(elements['now-playing-time'].textContent, '0:15 / 1:00');
    assert.ok(classes.has('jf-has-now-playing'), 'the page scrolls clear of the bar');
    context.fns.updateNowPlaying({ streamType: 'Videos' }, { Id: 'v' });
    assert.equal(elements['now-playing'].hidden, true, 'a video hides it');
    assert.equal(elements['video-player'].poster, '', 'and has no poster');
    assert.equal(classes.has('jf-has-now-playing'), false);
});

test('the art falls back to the album, the series, then the parent Jellyfin names', () => {
    const { context } = sandbox();
    const pic = item => JSON.parse(JSON.stringify(context.fns.itemPicture(item)));
    assert.deepEqual(pic({ Id: 'x', ImageTags: { Primary: 'p' } }), { id: 'x', tag: 'p' });
    assert.deepEqual(pic({ ParentPrimaryImageItemId: 'alb', ParentPrimaryImageTag: 't' }), { id: 'alb', tag: 't' });
    assert.equal(context.fns.itemPicture({ ImageTags: {} }), null);
});

test('closing the full player leaves music playing; a video stops', () => {
    const music = sandbox(audioSession);
    music.context.fns.onPlayerClosed();
    assert.deepEqual(music.calls, ['close menus', 'bars reset']);
    const video = sandbox({ streamType: 'Videos' });
    video.context.fns.onPlayerClosed();
    assert.ok(video.calls.includes('stop') && video.calls.includes('clear queue'));
});

test('the bar opens the full player and its × stops everything', () => {
    const { context, elements, calls } = sandbox(audioSession);
    context.fns.openFullPlayer();
    assert.equal(elements['player-modal'].open, true);
    context.fns.stopNowPlaying();
    assert.deepEqual(calls, ['showModal', 'stop', 'clear queue', 'reset nav', 'clear media session', 'close']);
    assert.equal(elements['now-playing'].hidden, true);
});

test('the bar\'s previous and next follow the queue', () => {
    const { context, elements } = sandbox(audioSession);
    context.setNav({ queue: true, prev: null, next: { Id: 'n' } });
    context.fns.updateEpisodeButtons();
    assert.equal(elements['now-playing-prev'].hidden, false);
    assert.equal(elements['now-playing-prev'].disabled, true);
    assert.equal(elements['now-playing-next'].disabled, false);
});

test('the end of the queue ends the music; the next song keeps the bar up', () => {
    const play = slice('async function playItem(');
    assert.match(play, /playLoading = request;/);
    assert.match(play, /session\.streamType === 'Audio' && !playLoading && !activePlayback\) endNowPlaying\(\)/);
    assert.match(play, /if \(activePlayback === null && !playLoading && !session\.ended\) updateNowPlaying\(null\)/);
    assert.match(play, /session\.ended = true;\s+session\.stop\(\);/);
    const { context, elements, calls } = sandbox(null);
    elements['now-playing'] = { hidden: false };
    vm.runInContext('endNowPlaying()', context);
    assert.deepEqual(calls, ['clear queue', 'reset nav', 'clear media session']);
    assert.equal(elements['now-playing'].hidden, true);
});

test('playItem leaves the full player shut for music and shows the bar', () => {
    const play = slice('async function playItem(');
    assert.match(play, /if \(!modal\.open && streamType !== 'Audio'\)/);
    assert.match(play, /\(!modal\.open && streamType !== 'Audio'\) \|\| request !== playRequest/);
    assert.match(play, /updateNowPlaying\(session, item\)/);
    assert.match(html, /<div id="now-playing" class="jf-now-playing"/);
    assert.match(slice('function bindMediaSession('), /stopNowPlaying\(\)/, 'the OS stop key stops the music');
});
