// The web client's Google Cast sender (cast.js) with the Cast SDK mocked: the SDK loads from Google's CDN only when
// asked (the picker) and never in a browser that can't cast; the receiver app is the user's CastReceiverId; the
// picker lists the Cast entry with the sessions (not the receiver's own session); the hand-off sends Jellyfin Web's
// PlayNow with item, position and tracks; remote controls send Jellyfin's commands; the receiver's reports drive
// the state; stopping the cast plays the item here where the receiver was.
// Usage: node r3cast_web.mjs (run by api_tests.rs r3cast_web_sender).
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

const CHROME = 'Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0 Safari/537.36';
const FIREFOX = 'Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0';
const SAFARI = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 14_0) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15';
const NS = 'urn:x-cast:com.connectsdk';

class FakeSession {
    constructor() { this.sent = []; this.listeners = {}; this.volume = null; this.muted = null; }
    getCastDevice() { return { friendlyName: 'Living Room' }; }
    addMessageListener(ns, fn) { this.listeners[ns] = fn; }
    removeMessageListener(ns) { delete this.listeners[ns]; }
    sendMessage(ns, text) { this.sent.push([ns, JSON.parse(text)]); return Promise.resolve(); }
    setVolume(v) { this.volume = v; }
    setMute(m) { this.muted = m; }
}

function castFramework() {
    const context = {
        options: null, listeners: {}, state: 'NO_DEVICES_AVAILABLE', current: null, ended: null, refuse: null,
        setOptions(o) { this.options = o; },
        addEventListener(type, fn) { this.listeners[type] = fn; },
        getCastState() { return this.state; },
        requestSession() {
            if (this.refuse) return Promise.reject(this.refuse);
            this.current = new FakeSession();
            this.listeners.sessionstatechanged({ sessionState: 'SESSION_STARTED' });
            return Promise.resolve();
        },
        getCurrentSession() { return this.current; },
        endCurrentSession(stop) {
            this.ended = stop;
            this.current = null;
            this.listeners.sessionstatechanged({ sessionState: 'SESSION_ENDED' });
        },
    };
    const framework = {
        CastContext: { getInstance: () => context },
        CastContextEventType: { CAST_STATE_CHANGED: 'caststatechanged', SESSION_STATE_CHANGED: 'sessionstatechanged' },
        SessionState: { SESSION_STARTED: 'SESSION_STARTED', SESSION_RESUMED: 'SESSION_RESUMED', SESSION_ENDED: 'SESSION_ENDED' },
    };
    return { context, framework };
}

function sandbox({ ua = CHROME, playing = null, receiverId = '6F511C87', sessions = [], stored = {}, token = 'tok' } = {}) {
    const calls = [];
    const log = name => (...args) => { calls.push([name, ...args]); };
    const elements = {};
    const scripts = [];
    function element(tag) {
        const el = {
            tag, dataset: {}, innerHTML: '', className: '', listeners: {}, open: false,
            classList: {
                set: new Set(),
                toggle(c, on) { if (on) this.set.add(c); else this.set.delete(c); },
                contains(c) { return this.set.has(c); },
            },
            addEventListener(type, fn) { this.listeners[type] = fn; },
            appendChild(child) { return child; },
            remove() { delete elements[this._id]; },
            showModal() { this.open = true; },
            close() { this.open = false; },
            set id(v) { this._id = v; elements[v] = this; },
            get id() { return this._id; },
        };
        return el;
    }
    const navCast = element('button');
    navCast.id = 'nav-cast';
    navCast.classList.set.add('hidden');
    const intercept = { fn: null };
    const { context, framework } = castFramework();
    const items = {
        i1: { Id: 'i1', ServerId: 'srv', Name: 'Test Film', Type: 'Movie', MediaType: 'Video', IsFolder: false, RunTimeTicks: 1e10,
            UserData: { PlaybackPositionTicks: 0 }, MediaStreams: [{ Type: 'Audio', Index: 1, DisplayTitle: 'English' }, { Type: 'Audio', Index: 2, DisplayTitle: 'French' }, { Type: 'Subtitle', Index: 3, DisplayTitle: 'English SRT' }] },
        i2: { Id: 'i2', ServerId: 'srv', Name: 'Other Film', Type: 'Movie', MediaType: 'Video', IsFolder: false, RunTimeTicks: 2e10,
            UserData: { PlaybackPositionTicks: 7e8 } },
    };
    const idle = [];
    const storage = { rustyfin_device_id: 'me', ...stored };
    const window = {
        chrome: { cast: { AutoJoinPolicy: { ORIGIN_SCOPED: 'origin_scoped' } } }, navigator: { userAgent: ua }, isSecureContext: true,
        requestIdleCallback: fn => { idle.push(fn); },
    };
    if (!/Chrome\//.test(ua)) delete window.chrome;
    const ctx = {
        console, Promise, JSON, Math, Number, String, Object, Date, Set, Infinity, encodeURIComponent,
        window,
        document: {
            head: { appendChild: s => { scripts.push(s); } },
            body: { appendChild: el => el },
            createElement: element,
            activeElement: null,
        },
        location: { hostname: 'rf.test' },
        localStorage: { getItem: k => storage[k] ?? null, setItem: (k, v) => { storage[k] = String(v); } },
        setTimeout: () => 0,
        clearTimeout: () => {},
        setInterval: () => 1,
        clearInterval: () => {},
        t: (key, vars) => (vars ? `${key}:${JSON.stringify(vars)}` : key),
        API: {
            token, userId: 'u1', base: 'https://rf.test',
            getMe: async () => ({ Configuration: receiverId ? { CastReceiverId: receiverId } : {} }),
            get: async path => {
                if (path === '/System/Info/Public') return { Id: 'srv', Version: '10.11.0', LocalAddress: 'http://192.0.2.5:8096' };
                if (path.startsWith('/Sessions')) return sessions;
                const id = decodeURIComponent(path.split('/').pop());
                if (!items[id]) throw new Error('404');
                return items[id];
            },
        },
        $: id => elements[id] || null,
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s ?? '').replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'),
        formatDurationSeconds: s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`,
        toast: log('toast'),
        nowPlaying: () => playing,
        stopNowPlaying: log('stopNowPlaying'),
        playItem: log('playItem'),
        setPlayInterceptor: fn => { intercept.fn = fn; calls.push(['setPlayInterceptor', !!fn]); },
        maxStreamingBitrate: () => 8000000,
    };
    vm.createContext(ctx);
    const cast = vm.runInContext(moduleBody('modules/cast.js', [
        'castSupported', 'loadCastSdk', 'isCasting', 'castPickerEntry', 'castToDevice', 'stopCasting', 'castTogglePause',
        'castSeek', 'castSkip', 'castSetVolume', 'castToggleMute', 'castAdjacent', 'castAudioTrack', 'castSubtitleTrack',
        'remotePositionTicks', 'updateCastButtons',
    ]), ctx);
    // sessions.js in a context of its own (both modules declare ownDeviceId), with cast.js's exports
    const sessionsCtx = vm.createContext({ ...ctx, ...cast });
    const sessionsModule = vm.runInContext(moduleBody('modules/sessions.js', ['openPlayOn']), sessionsCtx);
    // The SDK script ran: cast.framework exists and the SDK calls back
    const sdkLoaded = async () => {
        window.cast = { framework };
        window.__onGCastApiAvailable(true);
        await flush();
    };
    return { ctx, cast, sessions: sessionsModule, calls, elements, scripts, context, intercept, sdkLoaded, navCast, idle, storage };
}

const flush = () => new Promise(resolve => setImmediate(resolve));
const plain = v => JSON.parse(JSON.stringify(v));
const sent = sb => sb.context.current?.sent.map(([, m]) => m) || [];

async function connected(opts = {}) {
    const sb = sandbox(opts);
    const load = sb.cast.loadCastSdk();
    await sb.sdkLoaded();
    assert.equal(await load, true);
    sb.context.listeners.caststatechanged({ castState: 'NOT_CONNECTED' });
    await sb.cast.castToDevice();
    await flush();
    return sb;
}

test('Firefox and Safari: no Cast, no script, no picker entry', async () => {
    for (const ua of [FIREFOX, SAFARI]) {
        const sb = sandbox({ ua });
        assert.equal(sb.cast.castSupported(), false);
        assert.equal(await sb.cast.loadCastSdk(), false);
        assert.equal(sb.scripts.length, 0);
        assert.equal(sb.cast.castPickerEntry(), '');
    }
});

test('The SDK loads from Google only on demand, once, and uses the user\'s receiver app', async () => {
    const sb = sandbox();
    assert.equal(sb.scripts.length, 0, 'nothing loads with the page');
    const first = sb.cast.loadCastSdk();
    assert.equal(sb.cast.loadCastSdk(), first);
    assert.equal(sb.scripts.length, 1);
    assert.equal(sb.scripts[0].src, 'https://www.gstatic.com/cv/js/sender/v1/cast_sender.js?loadCastFramework=1');
    await sb.sdkLoaded();
    assert.equal(await first, true);
    assert.equal(sb.context.options.receiverApplicationId, '6F511C87');
    assert.equal(sb.context.options.autoJoinPolicy, 'origin_scoped');

    const other = sandbox({ receiverId: null });
    other.cast.loadCastSdk();
    await other.sdkLoaded();
    assert.equal(other.context.options.receiverApplicationId, 'F007D354', 'Jellyfin\'s stable receiver by default');
});

test('Opening the picker remembers Cast; later page loads fetch the SDK when idle and rejoin a running cast', async () => {
    const fresh = sandbox({ playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 } });
    assert.equal(fresh.idle.length, 0, 'a browser that never opened the picker schedules nothing');
    assert.equal(fresh.scripts.length, 0);
    await fresh.sessions.openPlayOn();
    assert.equal(fresh.storage.rustyfin_cast_used, '1');

    const sb = sandbox({ stored: { rustyfin_cast_used: '1' } });
    assert.equal(sb.scripts.length, 0, 'not during the page load itself');
    assert.equal(sb.idle.length, 1);
    sb.idle[0]();
    assert.equal(sb.scripts.length, 1);
    await sb.sdkLoaded();
    sb.context.listeners.caststatechanged({ castState: 'CONNECTED' });
    assert.ok(!sb.navCast.classList.contains('hidden'), 'the header button shows without opening the picker');
    // The SDK's auto-join finds the cast this browser started before the reload
    sb.context.current = new FakeSession();
    sb.context.listeners.sessionstatechanged({ sessionState: 'SESSION_RESUMED' });
    await flush();
    assert.equal(sb.cast.isCasting(), true);
    assert.equal(sent(sb)[0].command, 'Identify');
    assert.ok(sb.elements['cast-bar']);

    // Never in a browser that can't cast, nor signed out
    for (const opts of [{ ua: FIREFOX }, { ua: SAFARI }, { token: null }]) {
        const other = sandbox({ ...opts, stored: { rustyfin_cast_used: '1' } });
        assert.equal(other.idle.length, 0, JSON.stringify(opts));
        assert.equal(other.scripts.length, 0);
    }
});

test('A failed SDK load can be tried again', async () => {
    const sb = sandbox();
    const first = sb.cast.loadCastSdk();
    sb.scripts[0].onerror();
    assert.equal(await first, false);
    sb.cast.loadCastSdk();
    assert.equal(sb.scripts.length, 2);
});

test('The header button and the picker entry follow the devices found', async () => {
    const sb = sandbox();
    assert.match(sb.cast.castPickerEntry(), /cast\.searching/);
    sb.cast.loadCastSdk();
    await sb.sdkLoaded();
    assert.match(sb.cast.castPickerEntry(), /cast\.no_devices/);
    assert.match(sb.cast.castPickerEntry(), /disabled/);
    assert.ok(sb.navCast.classList.contains('hidden'));
    sb.context.listeners.caststatechanged({ castState: 'NOT_CONNECTED' });
    assert.match(sb.cast.castPickerEntry(), /cast\.choose_device/);
    assert.doesNotMatch(sb.cast.castPickerEntry(), /disabled/);
    assert.ok(!sb.navCast.classList.contains('hidden'));
});

test('The picker lists Cast with the sessions, not the receiver\'s own session', async () => {
    const recent = new Date().toISOString();
    const sessions = [
        { Id: 's1', DeviceId: 'tv', DeviceName: 'Lounge TV', Client: 'Jellyfin Android TV', SupportsMediaControl: true, LastActivityDate: recent },
        { Id: 's2', DeviceId: 'TGl2aW5n', DeviceName: 'Living Room', Client: 'Chromecast', SupportsMediaControl: true, LastActivityDate: recent },
        { Id: 's3', DeviceId: 'web-me', DeviceName: 'Browser', Client: 'Rustyfin Web', SupportsMediaControl: true, LastActivityDate: recent },
    ];
    const sb = sandbox({ playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 }, sessions });
    await sb.sessions.openPlayOn();
    const dialog = sb.elements['play-on-dialog'];
    assert.ok(dialog.open);
    assert.match(dialog.innerHTML, /id="play-on-cast"/);
    assert.match(dialog.innerHTML, /playOnSession\('s1'\)/);
    assert.doesNotMatch(dialog.innerHTML, /'s2'/);
    assert.doesNotMatch(dialog.innerHTML, /'s3'/);
    assert.equal(sb.scripts.length, 1, 'opening the picker loads the SDK');

    // Nothing playing: Cast only
    const idle = sandbox({ sessions });
    await idle.sessions.openPlayOn();
    assert.match(idle.elements['play-on-dialog'].innerHTML, /id="play-on-cast"/);
    assert.doesNotMatch(idle.elements['play-on-dialog'].innerHTML, /playOnSession/);

    // Firefox: the sessions only, as before
    const ff = sandbox({ ua: FIREFOX, playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 }, sessions });
    await ff.sessions.openPlayOn();
    assert.doesNotMatch(ff.elements['play-on-dialog'].innerHTML, /play-on-cast/);
    assert.match(ff.elements['play-on-dialog'].innerHTML, /playOnSession\('s1'\)/);
    assert.equal(ff.scripts.length, 0);
    const none = sandbox({ ua: FIREFOX, playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 } });
    await none.sessions.openPlayOn();
    assert.deepEqual(plain(none.calls.find(c => c[0] === 'toast')), ['toast', 'remote.no_targets', 'error']);
});

test('Casting hands the playing item over with its position and tracks', async () => {
    const playing = { itemId: 'i1', streamType: 'Videos', positionTicks: 1234500000, mediaSourceId: 'ms1', audioIndex: 2, subtitleIndex: 3 };
    const sb = await connected({ playing });
    assert.equal(sb.cast.isCasting(), true);
    const messages = sent(sb);
    assert.ok(sb.context.current.sent.every(([ns]) => ns === NS));
    assert.equal(messages[0].command, 'Identify');
    const play = messages.find(m => m.command === 'PlayNow');
    assert.deepEqual(play.options, {
        items: [{ Id: 'i1', ServerId: 'srv', Name: 'Test Film', Type: 'Movie', MediaType: 'Video', IsFolder: false }],
        startIndex: 0, startPositionTicks: 1234500000, mediaSourceId: 'ms1', audioStreamIndex: 2, subtitleStreamIndex: 3,
    });
    assert.equal(play.accessToken, 'tok');
    assert.equal(play.userId, 'u1');
    assert.equal(play.deviceId, 'web-me');
    assert.equal(play.serverId, 'srv');
    assert.equal(play.serverVersion, '10.11.0');
    assert.equal(play.serverAddress, 'https://rf.test');
    assert.equal(play.receiverName, 'Living Room');
    assert.equal(play.maxBitrate, 8000000);
    assert.ok(sb.calls.some(c => c[0] === 'stopNowPlaying'), 'local playback, music too, stops');
    assert.ok(sb.calls.some(c => c[0] === 'setPlayInterceptor' && c[1] === true));
    assert.ok(sb.elements['cast-bar'], 'the cast bar shows');
    assert.ok(!sb.navCast.classList.contains('hidden'));
});

test('A page on localhost gives the receiver the server\'s LAN address', async () => {
    const sb = sandbox({ playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 } });
    sb.ctx.location.hostname = 'localhost';
    sb.cast.loadCastSdk();
    await sb.sdkLoaded();
    sb.context.listeners.caststatechanged({ castState: 'NOT_CONNECTED' });
    await sb.cast.castToDevice();
    await flush();
    assert.equal(sent(sb).find(m => m.command === 'PlayNow').serverAddress, 'http://192.0.2.5:8096');
});

test('Closing Chrome\'s device chooser keeps local playback', async () => {
    const sb = sandbox({ playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 } });
    sb.cast.loadCastSdk();
    await sb.sdkLoaded();
    sb.context.refuse = 'cancel';
    await sb.cast.castToDevice();
    assert.equal(sb.cast.isCasting(), false);
    assert.ok(!sb.calls.some(c => c[0] === 'stopNowPlaying' || c[0] === 'toast'));
});

test('While casting, playing an item plays it on the receiver', async () => {
    const sb = await connected();
    assert.equal(sb.intercept.fn('i2', 'Videos', '', { fromStart: false, selection: null }), true);
    await flush();
    assert.equal(sent(sb).at(-1).command, 'PlayNow');
    assert.equal(sent(sb).at(-1).options.startPositionTicks, 7e8, 'from the resume point');
    sb.intercept.fn('i2', 'Videos', '', { fromStart: true, selection: null });
    await flush();
    assert.equal(sent(sb).at(-1).options.startPositionTicks, 0);
    sb.intercept.fn('i2', 'Videos', '', { fromStart: false, selection: { startTicks: 42, audioIndex: 1 } });
    await flush();
    assert.equal(sent(sb).at(-1).options.startPositionTicks, 42);
    assert.equal(sent(sb).at(-1).options.audioStreamIndex, 1);
});

function report(sb, type, data) {
    sb.context.current.listeners[NS](NS, JSON.stringify({ type, data }));
}

test('Remote controls send Jellyfin\'s commands; the receiver\'s reports set the state', async () => {
    const sb = await connected({ playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 } });
    report(sb, 'playbackprogress', {
        ItemId: 'i1',
        PlayState: { PositionTicks: 5e8, IsPaused: true, IsMuted: false, VolumeLevel: 40, AudioStreamIndex: 2, SubtitleStreamIndex: -1, MediaSourceId: 'ms1' },
        NowPlayingItem: { Id: 'i1', Name: 'Test Film', RunTimeTicks: 1e10, MediaType: 'Video' },
    });
    await flush();
    assert.equal(sb.cast.remotePositionTicks(), 5e8, 'paused: no drift');
    report(sb, 'timeupdate', { ItemId: 'i1', PlayState: { PositionTicks: 6e8 } });
    assert.equal(sb.cast.remotePositionTicks(), 6e8, 'a partial report keeps the rest');

    const last = () => sent(sb).at(-1);
    sb.cast.castTogglePause();
    await flush();
    assert.equal(last().command, 'Unpause');
    sb.cast.castTogglePause();
    await flush();
    assert.equal(last().command, 'Pause');
    sb.cast.castSeek(50);
    await flush();
    assert.deepEqual(plain([last().command, last().options]), ['Seek', { position: 500 }]);
    sb.cast.castSkip(-30);
    await flush();
    assert.deepEqual(plain(last().options), { position: 470 });
    sb.cast.castAdjacent(1);
    await flush();
    assert.equal(last().command, 'NextTrack');
    sb.cast.castAdjacent(-1);
    await flush();
    assert.equal(last().command, 'PreviousTrack');
    sb.cast.castAudioTrack('1');
    await flush();
    assert.deepEqual(plain([last().command, last().options]), ['SetAudioStreamIndex', { index: 1 }]);
    sb.cast.castSubtitleTrack('3');
    await flush();
    assert.deepEqual(plain([last().command, last().options]), ['SetSubtitleStreamIndex', { index: 3 }]);
    sb.cast.castSetVolume(30);
    assert.equal(sb.context.current.volume, 0.3);
    sb.cast.castToggleMute();
    assert.equal(sb.context.current.muted, true);

    report(sb, 'playbackerror', null);
    assert.ok(sb.calls.some(c => c[0] === 'toast' && c[1] === 'cast.playback_error'));
});

test('Stopping the cast plays the item here where the receiver was, with its tracks', async () => {
    const sb = await connected({ playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 } });
    report(sb, 'playbackprogress', {
        ItemId: 'i1',
        PlayState: { PositionTicks: 3e9, IsPaused: true, AudioStreamIndex: 2, SubtitleStreamIndex: 3, MediaSourceId: 'ms1' },
        NowPlayingItem: { Id: 'i1', Name: 'Test Film', RunTimeTicks: 1e10, MediaType: 'Video' },
    });
    const session = sb.context.current;
    await sb.cast.stopCasting();
    assert.equal(session.sent.at(-1)[1].command, 'Stop');
    assert.equal(sb.context.ended, true, 'the receiver app stops too');
    assert.equal(sb.cast.isCasting(), false);
    assert.ok(sb.calls.some(c => c[0] === 'setPlayInterceptor' && c[1] === false));
    assert.equal(sb.elements['cast-bar'], undefined, 'the cast bar goes');
    const play = sb.calls.find(c => c[0] === 'playItem');
    assert.deepEqual(plain(play), ['playItem', 'i1', 'Videos', 'Test Film', {
        selection: { mediaSourceId: 'ms1', audioIndex: 2, subtitleIndex: 3, startTicks: 3e9 },
    }]);
});

test('A cast the receiver ended (another sender, the TV) does not start local playback', async () => {
    const sb = await connected({ playing: { itemId: 'i1', streamType: 'Videos', positionTicks: 0 } });
    sb.context.endCurrentSession(false);
    assert.equal(sb.cast.isCasting(), false);
    assert.ok(!sb.calls.some(c => c[0] === 'playItem'));
});
