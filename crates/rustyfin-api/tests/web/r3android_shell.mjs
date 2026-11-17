// The Jellyfin Android app's bridge (modules/nativeshell.js) against a fake app: window.NativeInterface,
// NativePlayer and ExternalPlayer as the app adds them, and window.NativeShell with player plugins shaped like
// the app's own (assets/native/*.js in jellyfin-android). Checks the calls the page makes into the app, the
// calls the app makes into the page, and that a browser (no NativeInterface) gets none of it.
// Usage: node r3android_shell.mjs (run by api_tests.rs r3android_shell).
import assert from 'node:assert/strict';
import test from 'node:test';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);

// --- A minimal page: storage, location, history, elements by id, dialogs and the menu entries ---
const calls = [];
const record = (name, ...args) => calls.push([name, ...args]);
const take = name => calls.filter(c => c[0] === name).map(c => c.slice(1));
const reset = () => { calls.length = 0; };

function element(props = {}) {
    const listeners = {};
    const classes = new Set(props.classes || []);
    return {
        dataset: props.dataset || {},
        volume: 1, muted: false, paused: true,
        classList: {
            toggle: (name, on) => (on ? classes.add(name) : classes.delete(name)),
            contains: name => classes.has(name),
        },
        addEventListener: (type, fn) => (listeners[type] ||= []).push(fn),
        fire: type => (listeners[type] || []).forEach(fn => fn({ type })),
        close() { record('dialog.close'); this.open = false; },
        ...props,
    };
}

const video = element({ id: 'video-player' });
const nativeMenu = element({ id: 'native-menu', classes: ['hidden'] });
const menuEntries = ['clientsettings', 'downloadmanagement', 'multiserver', 'exitmenu', 'unknownfeature']
    .map(feature => element({ dataset: { nativeFeature: feature }, classes: ['hidden'] }));
let dialogs = [];
const docListeners = {};
const store = new Map();

globalThis.window = globalThis;
globalThis.localStorage = {
    getItem: k => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: k => store.delete(k),
};
globalThis.document = {
    readyState: 'complete',
    getElementById: id => ({ 'video-player': video, 'native-menu': nativeMenu })[id] || null,
    querySelectorAll: sel => {
        if (sel === 'dialog[open]') return dialogs.filter(d => d.open);
        if (sel === '[data-native-feature]') return menuEntries;
        return [];
    },
    addEventListener: (type, fn) => (docListeners[type] ||= []).push(fn),
};
globalThis.location = { hash: '#home', href: 'http://127.0.0.1:8250/web/index.html#home', origin: 'http://127.0.0.1:8250' };
globalThis.history = { length: 3, back: () => record('history.back') };
globalThis.addEventListener = (type, fn) => (docListeners['window:' + type] ||= []).push(fn);
for (const name of ['togglePlayPause', 'playAdjacentEpisode', 'skipTime', 'seekVideo', 'setVolume', 'closePlayer', 'stopNowPlaying', 'navigate', 'toast']) {
    globalThis[name] = (...args) => record(name, ...args);
}
globalThis.t = key => key;

// api.js's client (a classic script's global)
globalThis.API = {
    base: 'http://127.0.0.1:8250',
    token: 'token-a',
    userId: 'user-a',
    async get(path) {
        record('API.get', path);
        return { Id: path.split('/').pop(), Name: 'Movie A', RunTimeTicks: 6e10, UserData: { PlaybackPositionTicks: 2e9 } };
    },
    async getPublicInfo() { return { Id: 'server-a' }; },
    async reportPlaying(...args) { record('API.reportPlaying', ...args); },
    async reportStopped(...args) { record('API.reportStopped', ...args); },
};

// --- The browser: no NativeInterface, so nothing is set up ---
test('a browser gets none of the bridge', async () => {
    const shell = await import(new URL('modules/nativeshell.js?browser', src));
    assert.equal(await shell.nativeShellReady, null);
    assert.equal(shell.nativeVideoPlayer(), null);
    assert.equal(await shell.playNatively('item-a'), false);
    assert.equal(shell.canDownload({ CanDownload: true }), false);
    assert.equal(shell.downloadItems([{ Id: 'item-a' }]), false);
    shell.setNativeNowPlaying({ itemId: 'item-a' }, () => assert.fail('no metadata is made in a browser'));
    assert.equal(store.get('jellyfin_credentials'), undefined, 'no credentials stored');
    assert.equal(globalThis.Emby, undefined);
    assert.equal(docListeners.click, undefined, 'no click interception');
});

// --- The app: the Java objects, then NativeShell and its plugins as the app's scripts define them ---
const native = {
    exo: true,
    ext: false,
    features: ['clientsettings', 'downloadmanagement', 'exit', 'externallinks', 'filedownload', 'multiserver'],
};
const nativeCall = name => (...args) => { record(name, ...args); return true; };

class FakeExoPlayerPlugin {
    constructor({ playbackManager, appSettings, loading }) {
        window.ExoPlayer = this;
        Object.assign(this, { playbackManager, appSettings, loading, type: 'mediaplayer', id: 'exoplayer', priority: -1 });
    }
    canPlayMediaType(mediaType) { return mediaType === 'Video'; }
    canPlayItem(item, playOptions) {
        return window.NativePlayer.isEnabled() && playOptions.fullscreen && !this.playbackManager.syncPlayEnabled;
    }
    async play(options) {
        options.ids = options.items.map(item => item.Id);
        delete options.items;
        const preferences = {
            maxStreamingBitrateLocal: this.appSettings.maxStreamingBitrate(true, 'Video'),
            maxStreamingBitrateRemote: this.appSettings.maxStreamingBitrate(false, 'Video'),
        };
        window.NativePlayer.loadPlayer(JSON.stringify(options), JSON.stringify(preferences));
        this.loading.hide();
    }
}

class FakeExternalPlayerPlugin {
    constructor({ events, playbackManager }) {
        window.ExtPlayer = this;
        Object.assign(this, { events, playbackManager, type: 'mediaplayer', id: 'externalplayer', priority: -2, isExternalPlayer: true });
        this._currentTime = 0;
    }
    canPlayMediaType(mediaType) { return mediaType === 'Video'; }
    canPlayItem(item, playOptions) {
        return window.ExternalPlayer.isEnabled() && playOptions.fullscreen && !this.playbackManager.syncPlayEnabled;
    }
    async play(options) {
        this._currentTime = options.playerStartPositionTicks / 10000 || 0;
        record('ExtPlayer.url', options.url);
        const playOptions = options.item.playOptions;
        playOptions.ids = options.item ? [options.item.Id] : [];
        window.ExternalPlayer.initPlayer(JSON.stringify(playOptions));
    }
    currentTime() { return this._currentTime || 0; }
    async notifyEnded() {
        this.events.trigger(this, 'stopped', [{}]);
        this._currentTime = null;
    }
    async notifyTimeUpdate(currentTime) {
        currentTime = currentTime || this.playbackManager.duration(this) / 10000;
        this._currentTime = currentTime;
        this.events.trigger(this, 'timeupdate');
    }
}

class FakeNavigationPlugin {
    constructor({ playbackManager }) {
        window.NavigationHelper = this;
        this.playbackManager = playbackManager;
    }
    goBack() {
        if (window.Emby.Page.canGoBack()) window.Emby.Page.back();
        else window.NativeInterface.exitApp();
    }
}

class FakeMediaSegmentsPlugin {
    constructor({ events, appSettings, dashboard, ServerConnections }) {
        events.on(ServerConnections, 'localusersignedin', () => {
            for (const type of ['Intro', 'Outro']) {
                const action = appSettings.get(`segmentTypeAction__${type}`, dashboard.getCurrentUserId());
                if (action != null) record('MediaSegments.setSegmentTypeAction', type, action);
            }
        });
    }
}

let app;
test('the app: credentials, plugins, menu and Back', async () => {
    window.NativeInterface = {
        exitApp: nativeCall('exitApp'),
        openServerSelection: nativeCall('openServerSelection'),
    };
    window.NativePlayer = { isEnabled: () => native.exo, loadPlayer: nativeCall('NativePlayer.loadPlayer') };
    window.ExternalPlayer = { isEnabled: () => native.ext, initPlayer: nativeCall('ExternalPlayer.initPlayer') };
    store.set('user-a-segmentTypeAction__Intro', 'Skip');
    const plugins = { NavigationPlugin: FakeNavigationPlugin, ExoPlayerPlugin: FakeExoPlayerPlugin, ExternalPlayerPlugin: FakeExternalPlayerPlugin, MediaSegmentsPlugin: FakeMediaSegmentsPlugin };
    for (const [name, Plugin] of Object.entries(plugins)) window[name] = async () => Plugin;
    // The app's nativeshell.js runs after the page's scripts started (its loads are not ordered)
    setTimeout(() => {
        window.NativeShell = {
            enableFullscreen: nativeCall('enableFullscreen'),
            disableFullscreen: nativeCall('disableFullscreen'),
            openUrl: nativeCall('openUrl'),
            updateMediaSession: info => record('updateMediaSession', JSON.parse(JSON.stringify(info))),
            hideMediaSession: nativeCall('hideMediaSession'),
            updateVolumeLevel: nativeCall('updateVolumeLevel'),
            downloadFiles: info => record('downloadFiles', JSON.parse(JSON.stringify(info))),
            openDownloadManager: nativeCall('openDownloadManager'),
            openClientSettings: nativeCall('openClientSettings'),
            selectServer: () => window.NativeInterface.openServerSelection(),
            getPlugins: () => Object.keys(plugins),
            AppHost: {
                deviceId: () => 'device-a',
                init: nativeCall('AppHost.init'),
                supports: command => native.features.includes(command.toLowerCase()),
                exit: () => window.NativeInterface.exitApp(),
            },
        };
    }, 120);

    app = await import(new URL('modules/nativeshell.js?app', src));
    const credentials = JSON.parse(store.get('jellyfin_credentials'));
    assert.equal(credentials.Servers[0].UserId, 'user-a', 'stored before NativeShell came, for /Sessions/Capabilities/Full');
    assert.equal(credentials.Servers[0].AccessToken, 'token-a');

    assert.ok(await app.nativeShellReady, 'the bridge waits for NativeShell');
    await new Promise(r => setTimeout(r, 0));
    assert.equal(take('AppHost.init').length, 1);
    assert.equal(JSON.parse(store.get('jellyfin_credentials')).Servers[0].Id, 'server-a');
    assert.deepEqual(take('MediaSegments.setSegmentTypeAction'), [['Intro', 'Skip']], "the user's segment actions reach the app");

    // The menu shows what the app offers and this client knows
    assert.equal(nativeMenu.classList.contains('hidden'), false);
    // The app supports exit but not exitmenu, so Jellyfin's web client shows it no Exit entry
    assert.deepEqual(menuEntries.map(e => e.classList.contains('hidden')), [false, false, false, true, true]);
    app.nativeShellCommand('multiserver');
    app.nativeShellCommand('downloadmanagement');
    app.nativeShellCommand('clientsettings');
    assert.equal(take('openServerSelection').length, 1);
    assert.equal(take('openDownloadManager').length, 1);
    assert.equal(take('openClientSettings').length, 1);

    // Back: a dialog closes first, then history, and the home page exits the app
    reset();
    const dialog = element({ open: true });
    dialogs = [dialog];
    window.NavigationHelper.goBack();
    assert.equal(take('dialog.close').length, 1);
    location.hash = '#detail/item-a';
    window.NavigationHelper.goBack();
    assert.equal(take('history.back').length, 1);
    location.hash = '#home';
    window.NavigationHelper.goBack();
    assert.equal(take('exitApp').length, 1);
    native.features.push('exitmenu');
    app.nativeShellCommand('exitmenu');
    native.features.pop();
    assert.equal(take('exitApp').length, 2);

    // Signing out drops the stored credentials on the next page change
    API.token = null;
    docListeners['window:hashchange'].forEach(fn => fn());
    assert.equal(store.get('jellyfin_credentials'), undefined);
    API.token = 'token-a';
    docListeners['window:hashchange'].forEach(fn => fn());
});

test("videos play in the app's player with the page's choice", async () => {
    reset();
    native.exo = true;
    native.ext = false;
    assert.equal(app.nativeVideoPlayer().id, 'exoplayer');
    assert.equal(await app.playNatively('item-a', { resumeTicks: item => item.UserData.PlaybackPositionTicks }), true);
    const [[optionsJson, prefsJson]] = take('NativePlayer.loadPlayer');
    const options = JSON.parse(optionsJson);
    assert.deepEqual(options.ids, ['item-a']);
    assert.equal(options.startPositionTicks, 2e9, 'from the resume point');
    assert.equal(options.fullscreen, true);
    assert.deepEqual(JSON.parse(prefsJson), { maxStreamingBitrateLocal: 120e6, maxStreamingBitrateRemote: 120e6 }, 'Auto is the top step');

    reset();
    await app.playNatively('item-b', { fromStart: true, selection: { mediaSourceId: 'v720', audioIndex: 2, subtitleIndex: -1 } });
    const chosen = JSON.parse(take('NativePlayer.loadPlayer')[0][0]);
    assert.deepEqual(
        { start: chosen.startPositionTicks, source: chosen.mediaSourceId, audio: chosen.audioStreamIndex, sub: chosen.subtitleStreamIndex },
        { start: 0, source: 'v720', audio: 2, sub: -1 },
    );

    reset();
    await app.playNatively('item-c', { selection: { startTicks: 5e8 } });
    assert.equal(JSON.parse(take('NativePlayer.loadPlayer')[0][0]).startPositionTicks, 5e8, "a chapter's start");

    // The web player when the user picked it in the app
    native.exo = false;
    assert.equal(app.nativeVideoPlayer(), null);
    assert.equal(await app.playNatively('item-a'), false);
});

test('an external player: the page reports the start and the stop', async () => {
    reset();
    native.exo = false;
    native.ext = true;
    assert.equal(app.nativeVideoPlayer().id, 'externalplayer');
    assert.equal(await app.playNatively('item-a', { resumeTicks: () => 2e9 }), true);
    assert.deepEqual(JSON.parse(take('ExternalPlayer.initPlayer')[0][0]).ids, ['item-a']);
    assert.equal(take('API.reportPlaying')[0][1], 2e9);
    // The direct stream Jellyfin's web client gives an external player: version, device and token
    assert.equal(take('ExtPlayer.url')[0][0],
        'http://127.0.0.1:8250/Videos/item-a/stream?Static=true&mediaSourceId=item-a&deviceId=device-a&ApiKey=token-a');
    // The app calls these when VLC or mpv returns at 10 minutes
    window.ExtPlayer.notifyTimeUpdate(600000);
    await window.ExtPlayer.notifyEnded();
    const [[itemId, ticks]] = take('API.reportStopped');
    assert.equal(itemId, 'item-a');
    assert.equal(ticks, 6e9);

    // Played to the end: the position is the runtime
    reset();
    await app.playNatively('item-a', { fromStart: true });
    window.ExtPlayer.notifyTimeUpdate();
    await window.ExtPlayer.notifyEnded();
    assert.equal(take('API.reportStopped')[0][1], 6e10);

    // The detail page's version and tracks reach the stream, with the version's container
    reset();
    const get = API.get;
    API.get = async path => ({ ...(await get(path)), MediaSources: [{ Id: 'v1080', Container: 'mkv' }, { Id: 'v720', Container: 'mp4,m4v' }] });
    await app.playNatively('item-a', { selection: { mediaSourceId: 'v720', audioIndex: 2, subtitleIndex: 3 } });
    API.get = get;
    assert.equal(take('ExtPlayer.url')[0][0],
        'http://127.0.0.1:8250/Videos/item-a/stream.mp4?Static=true&mediaSourceId=v720&deviceId=device-a&AudioStreamIndex=2&SubtitleStreamIndex=3&ApiKey=token-a');
    const playOptions = JSON.parse(take('ExternalPlayer.initPlayer')[0][0]);
    assert.deepEqual([playOptions.mediaSourceId, playOptions.audioStreamIndex, playOptions.subtitleStreamIndex], ['v720', 2, 3]);
    window.ExtPlayer.notifyCanceled?.();
    await window.ExtPlayer.notifyEnded();
});

test("the web player drives the app's notification, and its buttons drive the web player", async () => {
    reset();
    const session = { itemId: 'song-a', streamType: 'Audio', position: () => 12.5, total: () => 200 };
    app.setNativeNowPlaying(session, () => ({ title: 'Song A', artist: 'Artist B', album: 'Album C', artwork: [{ src: 'http://x/img' }] }));
    const [[start]] = take('updateMediaSession');
    assert.deepEqual(start, {
        action: 'playbackstart', isLocalPlayer: true, itemId: 'song-a', title: 'Song A', artist: 'Artist B', album: 'Album C',
        imageUrl: 'http://x/img', duration: 200000, position: 12500, canSeek: true, isPaused: true,
    });
    video.paused = false;
    video.fire('play');
    video.fire('pause');
    assert.deepEqual(take('updateMediaSession').slice(1).map(([s]) => s.action), ['unpause', 'pause']);
    video.volume = 0.4;
    video.fire('volumechange');
    assert.deepEqual(take('updateVolumeLevel'), [[40]]);

    const pm = window.NavigationHelper.playbackManager;
    pm.playPause();
    pm.nextTrack();
    pm.previousTrack();
    pm.rewind();
    pm.seekMs(50000);
    pm.sendCommand({ Name: 'SetVolume', Arguments: { Volume: 30 } });
    pm.stop();
    assert.equal(take('togglePlayPause').length, 1);
    assert.deepEqual(take('playAdjacentEpisode'), [[1], [-1]]);
    assert.deepEqual(take('skipTime'), [[-10]]);
    assert.deepEqual(take('seekVideo'), [[25]]);
    assert.deepEqual(take('setVolume'), [[0.3]]);
    assert.equal(take('stopNowPlaying').length, 1, 'music stops under the now-playing bar');

    app.setNativeNowPlaying(null);
    assert.equal(take('hideMediaSession').length, 1);

    // A video in the page: no notification (a local video player does its own) but the app goes full screen
    reset();
    const film = { itemId: 'movie-a', streamType: 'Videos', position: () => 1, total: () => 100 };
    app.setNativeNowPlaying(film, () => assert.fail('no metadata for a video'));
    video.fire('pause');
    video.fire('timeupdate');
    assert.equal(take('updateMediaSession').length, 0);
    assert.equal(take('enableFullscreen').length, 1);
    // The next episode keeps it; music after it ends it; closing ends the notification
    app.setNativeNowPlaying({ ...film, itemId: 'movie-b' });
    assert.equal(take('enableFullscreen').length, 1);
    app.setNativeNowPlaying(session, () => ({ title: 'Song A' }));
    assert.equal(take('disableFullscreen').length, 1);
    assert.equal(take('updateMediaSession')[0][0].action, 'playbackstart');
    app.setNativeNowPlaying(null);
    assert.equal(take('hideMediaSession').length, 1);
    assert.equal(take('disableFullscreen').length, 1);
});

test('downloads go to the app by item id', async () => {
    reset();
    assert.equal(app.canDownload({ CanDownload: true }), true);
    assert.equal(app.canDownload({ CanDownload: false }), false);
    assert.equal(app.downloadItems([{ Id: 'item-a', Name: 'Movie A' }]), true);
    assert.deepEqual(take('downloadFiles')[0][0], [{
        itemId: 'item-a', serverId: 'server-a', title: 'Movie A', filename: 'Movie A',
        url: 'http://127.0.0.1:8250/Items/item-a/Download?api_key=token-a',
    }]);

    // A download link in the page (the item page's) goes to the app instead of the WebView
    reset();
    let prevented = false;
    const link = { href: 'http://127.0.0.1:8250/Items/item-b/Download', getAttribute: () => null };
    const event = { target: { closest: sel => (sel === 'a[href]' ? link : null) }, preventDefault: () => { prevented = true; } };
    docListeners.click.forEach(fn => fn(event));
    assert.equal(prevented, true);
    assert.equal(take('downloadFiles')[0][0][0].itemId, 'item-b');

    // An external link opens in the phone's browser
    reset();
    const out = { href: 'https://example.org/page', getAttribute: () => null };
    docListeners.click.forEach(fn => fn({ target: { closest: sel => (sel.includes('_blank') ? out : null) }, preventDefault() {} }));
    assert.deepEqual(take('openUrl'), [['https://example.org/page', '_blank']]);

    // No download for a book, as in Jellyfin's web client
    assert.equal(app.canDownload({ CanDownload: true, Type: 'Book' }), false);
});
