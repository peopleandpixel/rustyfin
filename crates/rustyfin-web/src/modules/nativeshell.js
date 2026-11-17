// The Jellyfin Android app's bridge. The app is a WebView around this client: it adds window.NativeInterface
// (and NativePlayer, ExternalPlayer, MediaSegments) before any script runs, and swaps the page's
// main.*.bundle.js (public/main.jellyfin.bundle.js) for its own scripts, which define window.NativeShell and
// player plugins. Here the client answers that contract as Jellyfin Web does: videos play in the app's
// player, the notification follows the web player, Back walks the page, and the user menu offers the app's
// settings, downloads, server choice and exit. Without NativeInterface (any browser) nothing here runs.
// No DOM-heavy imports, so tests load this module as it is.
import { BITRATE_STEPS, maxStreamingBitrate } from './playerprefs.js';

const inApp = () => typeof window !== 'undefined' && !!window.NativeInterface;
// api.js's client, a classic script's global (not a window property)
const api = () => (typeof API !== 'undefined' ? API : null);
// The app's AppHost features this client acts on, and what each menu entry calls
const MENU_COMMANDS = {
    clientsettings: s => s.openClientSettings(),
    downloadmanagement: s => s.openDownloadManager(),
    multiserver: s => s.selectServer(),
    // Jellyfin's web client offers Exit in the menu only to an app with this feature; the Android app exits
    // from Back on Home instead
    exitmenu: s => s.AppHost.exit(),
};
const SHELL_WAIT_MS = 10000;
const MEDIA_SESSION_EVERY_MS = 1000;

// window.NativeShell once the app's script ran; null in a browser or if it never came
let shell = null;
// The app's player plugins (type mediaplayer), first by priority
const players = [];
// The web player's item for the app's notification: { session, meta } (setNativeNowPlaying)
let nowPlaying = null;
let lastSessionUpdate = 0;
// The item an external player (VLC, mpv…) plays: { itemId, mediaSourceId, runtimeTicks }
let external = null;
let serverId = '';

// --- Jellyfin Web's events: on, off and trigger on any object; a handler gets (event, ...args) ---
const handlers = new WeakMap();
const events = {
    on(obj, name, fn) {
        if (!handlers.has(obj)) handlers.set(obj, new Map());
        const byName = handlers.get(obj);
        if (!byName.has(name)) byName.set(name, new Set());
        byName.get(name).add(fn);
    },
    off(obj, name, fn) {
        handlers.get(obj)?.get(name)?.delete(fn);
    },
    trigger(obj, name, args = []) {
        for (const fn of [...(handlers.get(obj)?.get(name) || [])]) fn({ type: name, target: obj }, ...args);
    },
};

// --- What the plugins are built with, as Jellyfin Web passes them ---
const media = () => document.getElementById('video-player');
const call = (name, ...args) => window[name]?.(...args);

// The page's player, which the notification, headset and volume keys drive (window.NavigationHelper.playbackManager)
const playbackManager = {
    syncPlayEnabled: false,
    _playNextAfterEnded: false,
    // The external player's item length in ticks, which it reports as the position when it played to the end
    duration: () => external?.runtimeTicks || null,
    unpause: () => media()?.play(),
    pause: () => media()?.pause(),
    playPause: () => call('togglePlayPause'),
    previousTrack: () => call('playAdjacentEpisode', -1),
    nextTrack: () => call('playAdjacentEpisode', 1),
    rewind: () => call('skipTime', -10),
    fastForward: () => call('skipTime', 30),
    stop: () => (nowPlaying?.session.streamType === 'Audio' ? call('stopNowPlaying') : call('closePlayer')),
    volumeUp: () => stepVolume(0.05),
    volumeDown: () => stepVolume(-0.05),
    seekMs(ms) {
        const total = nowPlaying?.session.total?.() || 0;
        if (total > 0) call('seekVideo', Math.min(Math.max(ms / 1000 / total, 0), 1) * 100);
    },
    sendCommand(command) {
        const volume = Number(command?.Arguments?.Volume);
        if (command?.Name === 'SetVolume' && Number.isFinite(volume)) call('setVolume', Math.min(Math.max(volume / 100, 0), 1));
    },
};

function stepVolume(delta) {
    const v = media();
    if (v) call('setVolume', Math.min(Math.max(v.volume + delta, 0), 1));
}

// Jellyfin Web's appSettings: per-user values under `<user id>-<name>`, as playerprefs.js keeps them
const appSettings = {
    get(name, userId) {
        try {
            return localStorage.getItem(userId ? `${userId}-${name}` : name);
        } catch {
            return null;
        }
    },
    // Auto is the highest step, so the app's player only transcodes for the user's own limit
    maxStreamingBitrate: () => maxStreamingBitrate() || BITRATE_STEPS[0],
};
const ServerConnections = {};
const pluginDeps = {
    events, playbackManager, appSettings, ServerConnections,
    loading: { show() {}, hide() {} },
    dashboard: { getCurrentUserId: () => api()?.userId || '' },
};

// --- Start-up: the app's scripts load in no fixed order, so NativeShell is awaited ---
function waitForShell(timeoutMs = SHELL_WAIT_MS) {
    return new Promise(resolve => {
        const until = Date.now() + timeoutMs;
        (function poll() {
            if (window.NativeShell) resolve(window.NativeShell);
            else if (Date.now() > until) resolve(null);
            else setTimeout(poll, 50);
        })();
    });
}

async function loadPlugins() {
    for (const name of shell.getPlugins?.() || []) {
        try {
            const Plugin = await window[name]?.();
            if (!Plugin) continue;
            const plugin = new Plugin(pluginDeps);
            if (plugin.type === 'mediaplayer') players.push(plugin);
        } catch (e) {
            console.warn(`Could not load the app's ${name}:`, e);
        }
    }
    players.sort((a, b) => (a.priority || 0) - (b.priority || 0));
    for (const player of players) events.on(player, 'stopped', () => externalStopped(player));
    signedIn();
}

// The app's media segment actions follow the user's (MediaSegmentsPlugin reads them on sign-in)
function signedIn() {
    if (api()?.token) events.trigger(ServerConnections, 'localusersignedin', [{ Id: api().userId }]);
}

// The app reads the user and token for its own player and downloads from Jellyfin Web's stored credentials
// when the page reports its capabilities (remote.js), so they are kept current on every page change
function syncCredentials() {
    const client = api();
    try {
        if (!client?.token || !client.userId) {
            localStorage.removeItem('jellyfin_credentials');
            return;
        }
        localStorage.setItem('jellyfin_credentials', JSON.stringify({
            Servers: [{
                Id: serverId, ManualAddress: client.base, LocalAddress: client.base, UserId: client.userId,
                AccessToken: client.token, DateLastAccessed: Date.now(),
            }],
        }));
    } catch (e) {
        console.warn('Could not store the credentials for the app:', e);
    }
}

// --- Playback in the app's player (ExoPlayer, or the external player the user chose there) ---
// The app's video player when the user chose one in the app's settings; null plays in the page
function nativeVideoPlayer() {
    return players.find(p => p.canPlayMediaType?.('Video') && p.canPlayItem?.({ MediaType: 'Video' }, { fullscreen: true })) || null;
}

// Plays a video in the app's player, from the detail page's choice (player.js's selection), the start, or the
// resume point (resumeTicks(item)). False leaves it to the page's player.
async function playNatively(itemId, { fromStart = false, selection = null, resumeTicks = () => 0 } = {}) {
    const player = nativeVideoPlayer();
    if (!player) return false;
    const item = await API.get('/Items/' + itemId).catch(e => {
        console.warn('Could not load the item for the app player:', e);
        return null;
    });
    if (!item) return false;
    const options = {
        startPositionTicks: selection?.startTicks ?? (fromStart ? 0 : resumeTicks(item)),
        mediaSourceId: selection?.mediaSourceId ?? undefined,
        audioStreamIndex: selection?.audioIndex ?? undefined,
        subtitleStreamIndex: selection?.subtitleIndex ?? undefined,
    };
    if (player.isExternalPlayer) return playExternal(player, item, options);
    signedIn();
    await player.play({ ...options, items: [item], startIndex: 0, fullscreen: true });
    return true;
}

// An external app reports nothing itself, so the page reports the start here and the stop when it returns
async function playExternal(player, item, options) {
    external = { itemId: item.Id, mediaSourceId: options.mediaSourceId || item.Id, runtimeTicks: item.RunTimeTicks || 0 };
    await player.play({
        url: `${API.base}/Videos/${item.Id}/stream?static=true`,
        item: { ...item, playOptions: options },
        playerStartPositionTicks: options.startPositionTicks || 0,
    });
    API.reportPlaying(item.Id, options.startPositionTicks || 0, {
        MediaSourceId: external.mediaSourceId, PlayMethod: 'DirectPlay', CanSeek: true, IsPaused: false,
    }).catch(e => console.warn('Could not report the start:', e));
    return true;
}

// currentTime() is the position in ms; the plugin clears it right after this event
function externalStopped(player) {
    if (!external || !player.isExternalPlayer) return;
    const { itemId, mediaSourceId } = external;
    external = null;
    API.reportStopped(itemId, Math.round((player.currentTime() || 0) * 1e4), { MediaSourceId: mediaSourceId })
        .catch(e => console.warn('Could not report the stop:', e));
}

// --- The page's player in the app ---
// player.js gives each item's session (streamType, and position() and total() in seconds) and a function that
// makes its media metadata, called only in the app; null when the player closed. As Jellyfin's web client
// does: music shows in the app's notification, and a video in the page makes the app full screen (a local
// video player does its own notification).
function setNativeNowPlaying(session, meta = () => ({})) {
    if (!shell) return;
    const was = nowPlaying;
    nowPlaying = session ? { session, video: session.streamType !== 'Audio', meta: {} } : null;
    if (was?.video && !nowPlaying?.video) shell.disableFullscreen();
    if (nowPlaying?.video && !was?.video) shell.enableFullscreen();
    if (was && !was.video && (!nowPlaying || nowPlaying.video)) shell.hideMediaSession();
    if (nowPlaying && !nowPlaying.video) {
        nowPlaying.meta = meta() || {};
        reportMediaSession('playbackstart');
    }
}

function reportMediaSession(action) {
    if (!shell || !nowPlaying || nowPlaying.video) return;
    const { session, meta } = nowPlaying;
    const seconds = fn => {
        try {
            return Number(fn?.call(session)) || 0;
        } catch {
            return 0;
        }
    };
    const total = seconds(session.total);
    lastSessionUpdate = Date.now();
    shell.updateMediaSession({
        action, isLocalPlayer: true, itemId: session.itemId, title: meta.title || '', artist: meta.artist || '',
        album: meta.album || '', imageUrl: meta.artwork?.[0]?.src || '', duration: Math.round(total * 1000),
        position: Math.round(seconds(session.position) * 1000), canSeek: total > 0, isPaused: media()?.paused ?? true,
    });
}

function watchPlayer(video) {
    video.addEventListener('play', () => reportMediaSession('unpause'));
    video.addEventListener('pause', () => reportMediaSession('pause'));
    video.addEventListener('timeupdate', () => {
        if (Date.now() - lastSessionUpdate >= MEDIA_SESSION_EVERY_MS) reportMediaSession('timeupdate');
    });
    video.addEventListener('volumechange', () => shell?.updateVolumeLevel(Math.round((video.muted ? 0 : video.volume) * 100)));
}

// --- Back: the app calls NavigationHelper.goBack(), which asks Emby.Page; it exits where that cannot go back ---
const openDialog = () => [...document.querySelectorAll('dialog[open]')].pop() || null;
const onHome = () => ['', 'home', 'login'].includes(location.hash.replace(/^#\/?/, '').split(/[/?]/)[0]);

const page = {
    canGoBack: () => !!openDialog() || !onHome(),
    back() {
        const dialog = openDialog();
        if (dialog) dialog.close();
        else if (history.length > 1) history.back();
        else call('navigate', 'home');
    },
};

// --- Downloads: the app downloads by item id (/Items/{id}/Download) into its own library ---
const canDownload = item => !!shell?.AppHost.supports('filedownload') && !!item?.CanDownload && item.Type !== 'Book';

function downloadItems(items) {
    if (!shell?.AppHost.supports('filedownload')) return false;
    shell.downloadFiles(items.map(item => ({
        itemId: item.Id, serverId, title: item.Name || '', filename: item.Name || item.Id,
        url: `${API.base}/Items/${item.Id}/Download`,
    })));
    return true;
}

// A download link anywhere in the page (/Items/{id}/Download) goes to the app, as the WebView saves no files
function interceptDownload(e) {
    const link = e.target.closest?.('a[href]');
    const id = link && new URL(link.href, location.href).pathname.match(/\/Items\/([^/]+)\/Download$/i)?.[1];
    if (!id || !downloadItems([{ Id: id, Name: link.getAttribute('download') || '' }])) return;
    e.preventDefault();
    call('toast', window.t?.('native.download_started'));
}

// A link out of the client opens in the phone's browser
function interceptExternalLink(e) {
    const link = e.target.closest?.('a[href][target="_blank"]');
    if (!link || !shell?.AppHost.supports('externallinks')) return;
    const url = new URL(link.href, location.href);
    if (url.origin === location.origin || !/^https?:$/.test(url.protocol)) return;
    e.preventDefault();
    shell.openUrl(url.href, '_blank');
}

// --- User menu: the app's settings, downloads, server choice and exit, where the app offers them ---
function nativeShellCommand(feature) {
    if (shell && MENU_COMMANDS[feature]) MENU_COMMANDS[feature](shell);
}

function showMenu() {
    let any = false;
    for (const el of document.querySelectorAll('[data-native-feature]')) {
        const on = !!MENU_COMMANDS[el.dataset.nativeFeature] && !!shell.AppHost.supports(el.dataset.nativeFeature);
        el.classList.toggle('hidden', !on);
        any ||= on;
    }
    document.getElementById('native-menu')?.classList.toggle('hidden', !any);
}

async function start() {
    syncCredentials();
    window.addEventListener('hashchange', syncCredentials);
    shell = await waitForShell();
    if (!shell) return;
    shell.AppHost.init?.();
    window.Emby = { ...window.Emby, Page: page };
    const ready = () => {
        showMenu();
        const video = media();
        if (video) watchPlayer(video);
    };
    if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', ready);
    else ready();
    document.addEventListener('click', interceptDownload, true);
    document.addEventListener('click', interceptExternalLink, true);

    await loadPlugins();
    API.getPublicInfo().then(info => {
        serverId = info?.Id || '';
        syncCredentials();
    }).catch(() => {});
}

// Resolves once the bridge is set up (null outside the app), so tests and callers can wait for it
const nativeShellReady = inApp() ? start().then(() => shell) : Promise.resolve(null);

export {
    nativeShellReady, nativeVideoPlayer, playNatively, setNativeNowPlaying, canDownload, downloadItems,
    nativeShellCommand, events, playbackManager, appSettings,
};
