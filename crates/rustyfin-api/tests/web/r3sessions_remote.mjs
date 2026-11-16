// The web client as a remote control target and controller: remote.js opens the socket with the token, reports
// SupportsMediaControl and its commands, keeps the socket alive, and obeys Play, Playstate and GeneralCommand
// messages through the player's exports; sessions.js lists the sessions with their now-playing item and controls,
// and "Play on" hands the playing item to another session. Runs both modules in a vm sandbox with the player stubbed.
// Usage: node r3sessions_remote.mjs (run by api_tests.rs r3sessions_remote).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, src), 'utf8');

// A module's code without its import and export statements, ending in an object of the names asked for
function moduleBody(file, names) {
    const code = read(file)
        .replace(/^import[\s\S]*?from '[^']+';$/gm, '')
        .replace(/^export \{[\s\S]*?\};$/gm, '');
    return `${code}\n({ ${names.join(', ')} });`;
}

function sandbox({ playing = { itemId: 'i1', streamType: 'Videos', positionTicks: 1234 }, items = {} } = {}) {
    const calls = [];
    const log = name => (...args) => { calls.push([name, ...args]); };
    const elements = {
        'video-player': { volume: 1, muted: false, pause: log('pause'), play: log('play') },
        'player-audio': { value: '' },
        'player-subs': { value: '' },
    };
    const sockets = [];
    const intervals = [];
    class WebSocket {
        static OPEN = 1;
        constructor(url) { this.url = url; this.readyState = 1; this.sent = []; sockets.push(this); }
        send(data) { this.sent.push(JSON.parse(data)); }
        close() { this.onclose?.(); }
    }
    const posts = [];
    const ctx = {
        console, Promise, JSON, Math, Number, String, Object, Date, Intl, URLSearchParams, encodeURIComponent,
        WebSocket,
        location: { protocol: 'http:', host: 'rf.test' },
        localStorage: { getItem: k => (k === 'rustyfin_device_id' ? 'me' : null) },
        window: { addEventListener: log('addEventListener') },
        document: { documentElement: { lang: 'en' } },
        setInterval: (fn, ms) => { intervals.push({ fn, ms }); return intervals.length; },
        clearInterval: () => {},
        setTimeout: () => 0,
        clearTimeout: () => {},
        prompt: () => 'hello there',
        t: (key, vars) => (vars ? `${key}:${JSON.stringify(vars)}` : key),
        API: {
            token: 'tok', userId: 'u1', userName: 'Admin',
            post: async (path, body) => { posts.push([path, body]); return {}; },
            get: async path => {
                const id = decodeURIComponent(path.replace('/Items/', ''));
                if (path.startsWith('/Sessions')) return items.sessions || [];
                if (!items[id]) throw new Error('404');
                return items[id];
            },
            getSessions: async () => items.sessions || [],
        },
        $: id => elements[id] || null,
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;'),
        formatDurationSeconds: s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`,
        toast: log('toast'),
        nowPlaying: () => playing,
        seekToTicks: log('seekToTicks'),
        playItem: log('playItem'),
        closePlayer: log('closePlayer'),
        togglePlayPause: log('togglePlayPause'),
        setVolume: log('setVolume'),
        toggleMute: () => { elements['video-player'].muted = !elements['video-player'].muted; calls.push(['toggleMute']); },
        changeAudioTrack: log('changeAudioTrack'),
        changeSubtitleTrack: log('changeSubtitleTrack'),
        skipTime: log('skipTime'),
        playAdjacentEpisode: log('playAdjacentEpisode'),
        toggleFullscreen: log('toggleFullscreen'),
        playContainer: log('playContainer'),
        playQueueAt: log('playQueueAt'),
        queueStreamType: item => (item.MediaType === 'Audio' ? 'Audio' : 'Videos'),
    };
    vm.createContext(ctx);
    const remote = vm.runInContext(moduleBody('modules/remote.js', ['handleMessage', 'onSocketMessage', 'sendSocket', 'COMMANDS']), ctx);
    Object.assign(ctx, remote);
    const sessions = vm.runInContext(moduleBody('modules/sessions.js', ['sessionListHtml', 'playOnSession', 'sessionControl', 'followSessions', 'unfollowSessions']), ctx);
    const socket = sockets[0];
    return { ctx, remote, sessions, socket, sockets, calls, posts, intervals, elements };
}

const flush = () => new Promise(resolve => setImmediate(resolve));
// Values from the sandbox's realm, as plain data of this one (an undefined in a list reads as null)
const plain = v => JSON.parse(JSON.stringify(v));
const send = (sb, MessageType, Data) => sb.remote.handleMessage({ MessageType, Data });

test('remote.js opens the socket with the token and reports media control on open', async () => {
    const sb = sandbox();
    assert.equal(sb.socket.url, 'ws://rf.test/socket?ApiKey=tok');
    sb.socket.onopen();
    await flush();
    const [path, body] = sb.posts[0];
    assert.equal(path, '/Sessions/Capabilities/Full');
    assert.equal(body.SupportsMediaControl, true);
    assert.deepEqual(plain(body.PlayableMediaTypes), ['Audio', 'Video']);
    for (const c of ['SetVolume', 'ToggleMute', 'DisplayMessage', 'SetAudioStreamIndex', 'SetSubtitleStreamIndex']) {
        assert.ok(body.SupportedCommands.includes(c), c);
    }
});

test('ForceKeepAlive sets a KeepAlive at half the interval', () => {
    const sb = sandbox();
    send(sb, 'ForceKeepAlive', 60);
    assert.equal(sb.intervals.at(-1).ms, 30000);
    sb.intervals.at(-1).fn();
    assert.deepEqual(sb.socket.sent.at(-1), { MessageType: 'KeepAlive' });
});

test('Playstate commands drive the player', () => {
    const sb = sandbox();
    for (const Command of ['Pause', 'Unpause', 'PlayPause', 'NextTrack', 'PreviousTrack', 'Rewind', 'FastForward', 'Stop']) {
        send(sb, 'Playstate', { Command });
    }
    send(sb, 'Playstate', { Command: 'Seek', SeekPositionTicks: 300000000 });
    assert.deepEqual(sb.calls.filter(c => c[0] !== 'addEventListener'), [
        ['pause'], ['play'], ['togglePlayPause'], ['playAdjacentEpisode', 1], ['playAdjacentEpisode', -1],
        ['skipTime', -10], ['skipTime', 30], ['closePlayer'], ['seekToTicks', 300000000],
    ]);
});

test('Playstate commands do nothing while nothing plays', () => {
    const sb = sandbox({ playing: null });
    send(sb, 'Playstate', { Command: 'Pause' });
    send(sb, 'Playstate', { Command: 'Seek', SeekPositionTicks: 1 });
    assert.deepEqual(sb.calls.filter(c => c[0] !== 'addEventListener'), []);
});

test('GeneralCommands set the volume, mute, tracks and show messages', () => {
    const sb = sandbox();
    send(sb, 'GeneralCommand', { Name: 'SetVolume', Arguments: { Volume: '50' } });
    send(sb, 'GeneralCommand', { Name: 'Mute', Arguments: {} });
    send(sb, 'GeneralCommand', { Name: 'Mute', Arguments: {} });
    send(sb, 'GeneralCommand', { Name: 'ToggleMute' });
    send(sb, 'GeneralCommand', { Name: 'SetAudioStreamIndex', Arguments: { Index: '2' } });
    send(sb, 'GeneralCommand', { Name: 'SetSubtitleStreamIndex', Arguments: { Index: '-1' } });
    send(sb, 'GeneralCommand', { Name: 'DisplayMessage', Arguments: { Header: 'H', Text: 'T', TimeoutMs: '5000' } });
    send(sb, 'GeneralCommand', { Name: 'GoHome', Arguments: {} });
    assert.deepEqual(sb.calls.filter(c => c[0] !== 'addEventListener'), [
        ['setVolume', 0.5], ['toggleMute'], ['toggleMute'], ['changeAudioTrack', '2'], ['changeSubtitleTrack', '-1'],
        ['toast', 'H: T'],
    ]);
    assert.equal(sb.elements['player-audio'].value, '2');
    assert.equal(sb.elements['player-subs'].value, '-1');
});

test('Play starts one item at the given point, a folder as Play all, several as the queue', async () => {
    const items = {
        a: { Id: 'a', Name: 'Film', MediaType: 'Video' },
        b: { Id: 'b', Name: 'Song', MediaType: 'Audio' },
        f: { Id: 'f', Name: 'Album', IsFolder: true },
    };
    const sb = sandbox({ playing: null, items });
    send(sb, 'Play', { ItemIds: ['a'], PlayCommand: 'PlayNow', StartPositionTicks: 100000000, AudioStreamIndex: 1 });
    await flush();
    send(sb, 'Play', { ItemIds: ['f'], PlayCommand: 'PlayShuffle' });
    await flush();
    send(sb, 'Play', { ItemIds: ['a', 'b'], PlayCommand: 'PlayNow', StartIndex: 1 });
    await flush();
    const got = sb.calls.filter(c => c[0] !== 'addEventListener');
    assert.deepEqual(plain(got[0]), ['playItem', 'a', 'Videos', 'Film', { selection: { startTicks: 100000000, audioIndex: 1 } }]);
    assert.deepEqual(plain(got[1]), ['playContainer', 'f', { shuffle: true }]);
    assert.equal(got[2][0], 'playQueueAt');
    assert.deepEqual(got[2][1].map(i => i.Id), ['a', 'b']);
    assert.equal(got[2][2], 1);
});

test('PlayNext while playing is refused, not played over the current item', async () => {
    const sb = sandbox({ items: { a: { Id: 'a', Name: 'Film' } } });
    send(sb, 'Play', { ItemIds: ['a'], PlayCommand: 'PlayNext' });
    await flush();
    assert.deepEqual(sb.calls.filter(c => c[0] !== 'addEventListener'), [['toast', 'remote.queue_unsupported', 'error']]);
});

test('listeners get the messages of their type', () => {
    const sb = sandbox();
    const seen = [];
    const off = sb.remote.onSocketMessage('Sessions', d => seen.push(d));
    send(sb, 'Sessions', [1]);
    off();
    send(sb, 'Sessions', [2]);
    assert.deepEqual(seen, [[1]]);
});

const session = (over = {}) => ({
    Id: 's1', UserName: 'Ann', DeviceName: 'TV', DeviceId: 'tv', Client: 'Jellyfin Android TV',
    LastActivityDate: new Date().toISOString(), SupportsMediaControl: true, SupportedCommands: ['DisplayMessage'],
    PlayState: { PositionTicks: 600000000, IsPaused: false, PlayMethod: 'Transcode' },
    NowPlayingItem: { Id: 'i', Name: 'Pilot', SeriesName: 'Show', RunTimeTicks: 1200000000 },
    ...over,
});

test('the dashboard lists now playing, progress, play method and the controls', () => {
    const sb = sandbox();
    const html = sb.sessions.sessionListHtml([session(), session({ Id: 's2', DeviceId: 'web-me', NowPlayingItem: null, SupportedCommands: [] })]);
    assert.match(html, /Show – Pilot/);
    assert.match(html, /aria-valuenow="50"/);
    assert.match(html, /1:00 \/ 2:00 · remote\.transcode/);
    assert.match(html, /sessionControl\('s1', 'Pause'\)/);
    assert.match(html, /sessionControl\('s1', 'Stop'\)/);
    assert.match(html, /sessionControl\('s1', 'Message'\)/);
    assert.match(html, /remote\.idle/);
    assert.match(html, /remote\.this_device/);
    assert.doesNotMatch(html, /sessionControl\('s2'/);
    const paused = sb.sessions.sessionListHtml([session({ PlayState: { IsPaused: true, PositionTicks: 0 } })]);
    assert.match(paused, /sessionControl\('s1', 'Unpause'\)/);
    const old = sb.sessions.sessionListHtml([session({ LastActivityDate: new Date(Date.now() - 3600e3).toISOString() })]);
    assert.match(old, /remote\.no_sessions/);
});

test('the dashboard controls post Jellyfin\'s session commands', async () => {
    const sb = sandbox();
    await sb.sessions.sessionControl('s1', 'Pause');
    await sb.sessions.sessionControl('s1', 'Message');
    assert.deepEqual(plain(sb.posts), [
        ['/Sessions/s1/Playing/Pause', null],
        ['/Sessions/s1/Message', { Header: 'Admin', Text: 'hello there', TimeoutMs: 5000 }],
    ]);
});

test('the Sessions tab follows the socket and stops when left', () => {
    const sb = sandbox();
    sb.sessions.followSessions();
    assert.deepEqual(sb.socket.sent.at(-1), { MessageType: 'SessionsStart', Data: '0,1500' });
    sb.sessions.unfollowSessions();
    assert.deepEqual(sb.socket.sent.at(-1), { MessageType: 'SessionsStop' });
});

test('Play on hands the playing item over at its position and closes the player', async () => {
    const sb = sandbox();
    await sb.sessions.playOnSession('s9');
    assert.deepEqual(plain(sb.posts), [['/Sessions/s9/Playing?playCommand=PlayNow&itemIds=i1&startPositionTicks=1234', null]]);
    assert.ok(sb.calls.some(c => c[0] === 'closePlayer'));
});
