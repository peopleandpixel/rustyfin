// Google Cast, as Jellyfin Web's chromecastPlayer: this browser is the sender, and the Jellyfin receiver app
// (the user's CastReceiverId, one of the server's CastReceiverApplications) plays from this server itself with
// this user's token. The sender SDK loads from Google's CDN only once the user has opened the "Play on" picker in
// this browser: then, and on later page loads when the page is idle (so the header button shows and a running cast
// is rejoined). A browser that never opened the picker never contacts Google, and one that can't cast (Firefox,
// Safari, iOS) never loads it. Commands go to the receiver on Jellyfin's connectsdk
// namespace; what it reports back drives the cast bar. Stopping the cast resumes here where the receiver was.
import { $, icon, toast, escapeHtml, formatDurationSeconds } from './util.js';
import { nowPlaying, closePlayer, playItem, setPlayInterceptor } from './player.js';
import { maxStreamingBitrate } from './playerprefs.js';

const SDK_URL = 'https://www.gstatic.com/cv/js/sender/v1/cast_sender.js?loadCastFramework=1';
const SDK_TIMEOUT_MS = 15000;
const NAMESPACE = 'urn:x-cast:com.connectsdk';
// Jellyfin's stable receiver, for a user configuration without a CastReceiverId
const DEFAULT_RECEIVER = 'F007D354';
// Set when the user first opens the picker in a browser that can cast: from then on the SDK loads when idle
const CAST_USED_KEY = 'rustyfin_cast_used';

let sdk = null;
let castState = 'NO_DEVICES_AVAILABLE';
let casting = null;
let remote = null;
let serverInfo = null;
let tickTimer = null;

// Chromium on the desktop and Android, on HTTPS or localhost: what the Cast sender SDK supports
function castSupported() {
    const ua = window.navigator?.userAgent || '';
    return !!window.chrome && /Chrome\//.test(ua) && !/Firefox|FxiOS|CriOS|EdgiOS/.test(ua)
        && window.isSecureContext !== false;
}

// Loads the SDK once; resolves true when the Cast context is set up, false when this browser can't cast
function loadCastSdk() {
    if (!castSupported()) return Promise.resolve(false);
    if (sdk) return sdk;
    sdk = new Promise(resolve => {
        const timer = setTimeout(() => resolve(false), SDK_TIMEOUT_MS);
        window.__onGCastApiAvailable = available => {
            clearTimeout(timer);
            resolve(available ? initCast().catch(() => false) : false);
        };
        const script = document.createElement('script');
        script.src = SDK_URL;
        script.async = true;
        script.onerror = () => { clearTimeout(timer); resolve(false); };
        document.head.appendChild(script);
    }).then(ok => {
        if (!ok) sdk = null;
        return ok;
    });
    return sdk;
}

async function receiverAppId() {
    try {
        return (await API.getMe())?.Configuration?.CastReceiverId || DEFAULT_RECEIVER;
    } catch (e) {
        return DEFAULT_RECEIVER;
    }
}

async function initCast() {
    const framework = window.cast.framework;
    const context = framework.CastContext.getInstance();
    context.setOptions({
        receiverApplicationId: await receiverAppId(),
        autoJoinPolicy: window.chrome.cast.AutoJoinPolicy.ORIGIN_SCOPED,
    });
    context.addEventListener(framework.CastContextEventType.CAST_STATE_CHANGED, e => {
        castState = e.castState;
        updateCastButtons();
    });
    context.addEventListener(framework.CastContextEventType.SESSION_STATE_CHANGED, e => {
        const state = framework.SessionState;
        if (e.sessionState === state.SESSION_STARTED || e.sessionState === state.SESSION_RESUMED) {
            attach(context.getCurrentSession());
        } else if (e.sessionState === state.SESSION_ENDED) {
            detach();
        }
    });
    castState = context.getCastState();
    updateCastButtons();
    return true;
}

const castContext = () => window.cast.framework.CastContext.getInstance();
const castLoaded = () => !!window.cast?.framework && sdk !== null;
const devicesFound = () => castLoaded() && castState !== 'NO_DEVICES_AVAILABLE';
const isCasting = () => !!casting;

// The header's cast button shows once a Cast device is found (or while casting)
function updateCastButtons() {
    const button = $('nav-cast');
    if (button) {
        button.classList.toggle('hidden', !(casting || devicesFound()));
        button.classList.toggle('text-[#00a4dc]', !!casting);
    }
    const entry = $('play-on-cast');
    if (entry) entry.outerHTML = castPickerEntry();
}

// ── Messages to the receiver ──

function ownDeviceId() {
    return 'web-' + (localStorage.getItem('rustyfin_device_id') || '');
}

// The address the receiver reaches this server at: a page on localhost gives the server's LAN address
async function castServer() {
    if (!serverInfo) serverInfo = await API.get('/System/Info/Public').catch(() => ({}));
    const host = location.hostname;
    const local = host === 'localhost' || host === '[::1]' || host.startsWith('127.');
    return { ...serverInfo, address: (local && serverInfo.LocalAddress) || API.base };
}

// Jellyfin Web's message: the command and its options, with the server and user the receiver plays for
async function castMessage(command, options = {}) {
    const target = casting;
    if (!target) return;
    const server = await castServer();
    const message = {
        command, options,
        userId: API.userId, deviceId: ownDeviceId(), accessToken: API.token,
        serverAddress: server.address, serverId: server.Id, serverVersion: server.Version,
        receiverName: target.deviceName,
    };
    const bitrate = maxStreamingBitrate();
    if (bitrate) message.maxBitrate = bitrate;
    return target.session.sendMessage(NAMESPACE, JSON.stringify(message));
}

// Plays an item on the receiver: from startTicks, else the resume point unless fromStart, with the tracks given
async function castPlay(itemId, { startTicks = null, fromStart = false, mediaSourceId = null, audioIndex = null, subtitleIndex = null } = {}) {
    const item = await API.get(`/Users/${API.userId}/Items/${encodeURIComponent(itemId)}`);
    const start = startTicks ?? (fromStart ? 0 : item?.UserData?.PlaybackPositionTicks || 0);
    const options = {
        items: [{ Id: item.Id, ServerId: item.ServerId, Name: item.Name, Type: item.Type, MediaType: item.MediaType, IsFolder: item.IsFolder }],
        startIndex: 0,
        startPositionTicks: start,
    };
    if (mediaSourceId) options.mediaSourceId = mediaSourceId;
    if (audioIndex !== null && audioIndex !== undefined) options.audioStreamIndex = audioIndex;
    if (subtitleIndex !== null && subtitleIndex !== undefined) options.subtitleStreamIndex = subtitleIndex;
    remote = {
        itemId: item.Id, item, name: item.Name, mediaType: item.MediaType, runtimeTicks: item.RunTimeTicks || 0,
        positionTicks: start, isPaused: false, audioIndex, subtitleIndex, mediaSourceId, at: Date.now(),
    };
    await castMessage('PlayNow', options);
    renderCastBar();
}

// While casting, the player's playItem plays on the receiver
function castInterceptor(itemId, streamType, title, { fromStart, selection }) {
    if (!casting) return false;
    castPlay(itemId, {
        fromStart, startTicks: selection?.startTicks ?? null, mediaSourceId: selection?.mediaSourceId ?? null,
        audioIndex: selection?.audioIndex ?? null, subtitleIndex: selection?.subtitleIndex ?? null,
    }).catch(() => toast(t('cast.playback_error'), 'error'));
    return true;
}

// ── The Cast session ──

function attach(session) {
    if (!session || casting?.session === session) return;
    casting = { session, deviceName: session.getCastDevice?.()?.friendlyName || '' };
    session.addMessageListener(NAMESPACE, onReceiverMessage);
    setPlayInterceptor(castInterceptor);
    castMessage('Identify').catch(() => {});
    clearInterval(tickTimer);
    tickTimer = setInterval(updateCastBar, 1000);
    renderCastBar();
    updateCastButtons();
}

function detach() {
    if (!casting) return;
    casting.session.removeMessageListener?.(NAMESPACE, onReceiverMessage);
    casting = null;
    remote = null;
    setPlayInterceptor(null);
    clearInterval(tickTimer);
    tickTimer = null;
    renderCastBar();
    updateCastButtons();
}

// The picker's Cast entry: Chrome's device chooser, then the playing item moves to the receiver where it is
async function castToDevice() {
    $('play-on-dialog')?.close();
    if (!(await loadCastSdk())) {
        toast(t('cast.unavailable'), 'error');
        return;
    }
    const np = nowPlaying();
    try {
        await castContext().requestSession();
    } catch (e) {
        if (e !== 'cancel') toast(t('cast.connect_failed'), 'error');
        return;
    }
    attach(castContext().getCurrentSession());
    if (!np || !casting) return;
    try {
        await castPlay(np.itemId, {
            startTicks: np.positionTicks, mediaSourceId: np.mediaSourceId, audioIndex: np.audioIndex, subtitleIndex: np.subtitleIndex,
        });
        closePlayer();
    } catch (e) {
        toast(t('cast.playback_error'), 'error');
    }
}

// Ends the cast and plays the item here from where the receiver was, with its tracks
async function stopCasting() {
    if (!casting) return;
    const last = remote;
    const at = remotePositionTicks();
    await castMessage('Stop').catch(() => {});
    detach();
    try {
        castContext().endCurrentSession(true);
    } catch (e) { /* already gone */ }
    if (!last?.itemId || last.stopped) return;
    playItem(last.itemId, last.mediaType === 'Audio' ? 'Audio' : 'Videos', last.name || '', {
        selection: { mediaSourceId: last.mediaSourceId, audioIndex: last.audioIndex, subtitleIndex: last.subtitleIndex, startTicks: at },
    });
}

// What the receiver reports: { type, data: { ItemId, PlayState, NowPlayingItem } }, or a playback error
function onReceiverMessage(namespace, text) {
    let message;
    try {
        message = typeof text === 'string' ? JSON.parse(text) : text;
    } catch (e) {
        return;
    }
    if (message?.type === 'playbackerror') {
        toast(t('cast.playback_error'), 'error');
        return;
    }
    const data = message?.data;
    if (!data?.ItemId) return;
    const ps = data.PlayState || {};
    const item = data.NowPlayingItem || {};
    const same = remote?.itemId === data.ItemId;
    const prev = same ? remote : {};
    const pick = (value, old) => (value === undefined ? old : value);
    remote = {
        ...prev,
        itemId: data.ItemId,
        name: pick(item.Name, prev.name),
        mediaType: pick(item.MediaType, prev.mediaType),
        runtimeTicks: pick(item.RunTimeTicks, prev.runtimeTicks) || 0,
        positionTicks: pick(ps.PositionTicks, prev.positionTicks) || 0,
        isPaused: pick(ps.IsPaused, prev.isPaused) ?? false,
        muted: pick(ps.IsMuted, prev.muted) ?? false,
        volume: pick(ps.VolumeLevel, prev.volume) ?? 100,
        audioIndex: pick(ps.AudioStreamIndex, prev.audioIndex),
        subtitleIndex: pick(ps.SubtitleStreamIndex, prev.subtitleIndex),
        mediaSourceId: pick(ps.MediaSourceId, prev.mediaSourceId),
        stopped: message.type === 'playbackstop',
        at: Date.now(),
    };
    if (!same || !remote.item) loadRemoteItem(data.ItemId);
    renderCastBar();
}

// The playing item's details (its audio and subtitle tracks for the bar's menus)
async function loadRemoteItem(itemId) {
    try {
        const item = await API.get(`/Users/${API.userId}/Items/${encodeURIComponent(itemId)}`);
        if (remote?.itemId !== itemId) return;
        remote.item = item;
        remote.name = remote.name || item.Name;
        remote.runtimeTicks = remote.runtimeTicks || item.RunTimeTicks || 0;
        renderCastBar(true);
    } catch (e) { /* the bar shows what the receiver reported */ }
}

// Where the receiver is now: its last report, moved on by the time since while it plays
function remotePositionTicks(now = Date.now()) {
    if (!remote) return 0;
    const moved = remote.isPaused || remote.stopped ? 0 : (now - remote.at) * 1e4;
    const ticks = remote.positionTicks + moved;
    return Math.round(remote.runtimeTicks ? Math.min(ticks, remote.runtimeTicks) : ticks);
}

// ── Remote controls ──

function castTogglePause() {
    if (!remote) return;
    remote.positionTicks = remotePositionTicks();
    remote.at = Date.now();
    remote.isPaused = !remote.isPaused;
    castMessage(remote.isPaused ? 'Pause' : 'Unpause');
    updateCastBar();
}

function castSeekTicks(ticks) {
    if (!remote) return;
    const max = remote.runtimeTicks || Infinity;
    remote.positionTicks = Math.max(0, Math.min(ticks, max));
    remote.at = Date.now();
    castMessage('Seek', { position: remote.positionTicks / 1e7 });
    updateCastBar();
}

function castSeek(percent) {
    if (remote?.runtimeTicks) castSeekTicks(remote.runtimeTicks * Number(percent) / 100);
}

function castSkip(seconds) {
    castSeekTicks(remotePositionTicks() + Number(seconds) * 1e7);
}

function castSetVolume(value) {
    if (!casting) return;
    const level = Math.max(0, Math.min(100, Number(value)));
    if (remote) remote.volume = level;
    casting.session.setVolume?.(level / 100);
}

function castToggleMute() {
    if (!casting) return;
    const muted = !remote?.muted;
    if (remote) remote.muted = muted;
    casting.session.setMute?.(muted);
    updateCastBar();
}

function castAdjacent(step) {
    castMessage(step > 0 ? 'NextTrack' : 'PreviousTrack');
}

function castAudioTrack(value) {
    if (remote) remote.audioIndex = Number(value);
    castMessage('SetAudioStreamIndex', { index: Number(value) });
}

function castSubtitleTrack(value) {
    if (remote) remote.subtitleIndex = Number(value);
    castMessage('SetSubtitleStreamIndex', { index: Number(value) });
}

// ── The cast bar ──

function trackOptions(streams, type, selected, withOff) {
    const list = (streams || []).filter(s => s.Type === type);
    const options = list.map(s => `<option value="${s.Index}" ${s.Index === selected ? 'selected' : ''}>${escapeHtml(s.DisplayTitle || s.Language || `#${s.Index}`)}</option>`);
    if (withOff) options.unshift(`<option value="-1" ${selected === -1 || selected === null || selected === undefined ? 'selected' : ''}>${t('cast.subtitles_off')}</option>`);
    return list.length ? options.join('') : '';
}

function castBarHtml() {
    const streams = remote?.item?.MediaStreams || [];
    const audio = trackOptions(streams, 'Audio', remote?.audioIndex, false);
    const subs = trackOptions(streams, 'Subtitle', remote?.subtitleIndex, true);
    const btn = 'jf-icon-btn !bg-white/10 hover:!bg-white/20 text-white';
    const select = 'select select-xs bg-white/10 text-white max-w-[9rem]';
    return `
        <div class="flex items-center gap-2 min-w-0">
            ${icon('Cast', { size: 18, class: 'text-[#00a4dc] shrink-0' })}
            <div class="min-w-0">
                <p id="cast-bar-title" class="text-sm font-semibold text-white truncate">${escapeHtml(remote?.name || '')}</p>
                <p class="text-xs text-white/60 truncate">${escapeHtml(t('cast.casting_to', { name: casting.deviceName || t('cast.google_cast') }))}</p>
            </div>
        </div>
        <div class="flex items-center gap-2 flex-1 min-w-[12rem]">
            <button type="button" class="${btn}" onclick="castAdjacent(-1)" title="${t('cast.previous')}" aria-label="${t('cast.previous')}">${icon('SkipBack')}</button>
            <button type="button" id="cast-bar-play" class="${btn}" onclick="castTogglePause()" title="${t('cast.play_pause')}" aria-label="${t('cast.play_pause')}"></button>
            <button type="button" class="${btn}" onclick="castAdjacent(1)" title="${t('cast.next')}" aria-label="${t('cast.next')}">${icon('SkipForward')}</button>
            <span id="cast-bar-time" class="text-xs text-white/70 tabular-nums"></span>
            <input id="cast-bar-seek" type="range" min="0" max="100" step="0.1" value="0" class="range range-xs flex-1"
                aria-label="${t('cast.seek')}" onchange="castSeek(this.value)">
        </div>
        <div class="flex items-center gap-2">
            <button type="button" id="cast-bar-mute" class="${btn}" onclick="castToggleMute()" title="${t('cast.mute')}" aria-label="${t('cast.mute')}"></button>
            <input id="cast-bar-volume" type="range" min="0" max="100" step="1" class="range range-xs w-20"
                aria-label="${t('cast.volume')}" onchange="castSetVolume(this.value)">
            ${audio ? `<select id="cast-bar-audio" class="${select}" aria-label="${t('cast.audio')}" onchange="castAudioTrack(this.value)">${audio}</select>` : ''}
            ${subs ? `<select id="cast-bar-subs" class="${select}" aria-label="${t('cast.subtitles')}" onchange="castSubtitleTrack(this.value)">${subs}</select>` : ''}
            <button type="button" class="${btn}" onclick="stopCasting()" title="${t('cast.stop')}" aria-label="${t('cast.stop')}">${icon('Square')}</button>
        </div>`;
}

// Draws the bar again when the item or its tracks change; otherwise only its values move
function renderCastBar(force = false) {
    let bar = $('cast-bar');
    if (!casting) {
        bar?.remove();
        return;
    }
    if (!bar) {
        bar = document.createElement('div');
        bar.id = 'cast-bar';
        bar.className = 'fixed bottom-0 left-0 right-0 z-50 flex flex-wrap items-center gap-3 px-4 py-2 bg-[#101827]/95 border-t border-white/10';
        document.body.appendChild(bar);
    }
    const key = `${remote?.itemId || ''}|${remote?.item ? 1 : 0}`;
    if (force || bar.dataset.key !== key) {
        bar.dataset.key = key;
        bar.innerHTML = castBarHtml();
    }
    updateCastBar();
}

function updateCastBar() {
    if (!casting || !$('cast-bar')) return;
    const at = remotePositionTicks();
    const total = remote?.runtimeTicks || 0;
    const time = $('cast-bar-time');
    if (time) time.textContent = total ? `${formatDurationSeconds(at / 1e7)} / ${formatDurationSeconds(total / 1e7)}` : '';
    const seek = $('cast-bar-seek');
    if (seek && document.activeElement !== seek) seek.value = total ? String((at / total) * 100) : '0';
    const play = $('cast-bar-play');
    if (play) play.innerHTML = icon(!remote || remote.isPaused || remote.stopped ? 'Play' : 'Pause');
    const mute = $('cast-bar-mute');
    if (mute) mute.innerHTML = icon(remote?.muted ? 'VolumeX' : 'Volume2');
    const volume = $('cast-bar-volume');
    if (volume && document.activeElement !== volume) volume.value = String(remote?.volume ?? 100);
}

// ── The picker's Cast entry (sessions.js) ──

// The entry's markup: a device to cast to, the search while the SDK loads, none found, or stop casting
function castPickerEntry() {
    if (!castSupported()) return '';
    const row = 'w-full text-left rounded-lg px-3 py-2 text-white';
    if (casting) {
        return `<li id="play-on-cast"><button type="button" class="${row} hover:bg-white/10" onclick="$('play-on-dialog')?.close(); stopCasting()">
            <span class="block text-sm font-semibold">${icon('Cast', { size: 14, class: 'inline text-[#00a4dc]' })} ${escapeHtml(casting.deviceName || t('cast.google_cast'))}</span>
            <span class="block text-xs text-white/50">${t('cast.stop')}</span></button></li>`;
    }
    const found = devicesFound();
    const note = !castLoaded() ? t('cast.searching') : found ? t('cast.choose_device') : t('cast.no_devices');
    return `<li id="play-on-cast"><button type="button" class="${row} ${found ? 'hover:bg-white/10' : 'opacity-60'}"
        ${found ? '' : 'disabled'} onclick="castToDevice()">
        <span class="block text-sm font-semibold">${icon('Cast', { size: 14, class: 'inline' })} ${t('cast.google_cast')}</span>
        <span class="block text-xs text-white/50">${note}</span></button></li>`;
}

// Opening the picker loads the SDK; the entry is drawn again once it knows about devices
function refreshPickerEntry() {
    if (!castSupported()) return;
    try {
        localStorage.setItem(CAST_USED_KEY, '1');
    } catch (e) { /* storage off: the SDK loads with the picker only */ }
    loadCastSdk().then(updateCastButtons);
}

// After the user has used the picker in this browser, a page load fetches the SDK once the page is idle
function loadCastWhenIdle() {
    if (!castSupported() || !API.token || localStorage.getItem(CAST_USED_KEY) !== '1') return false;
    const idle = window.requestIdleCallback || (fn => setTimeout(fn, 2000));
    idle(() => { loadCastSdk().then(updateCastButtons); });
    return true;
}
loadCastWhenIdle();

export {
    castSupported, loadCastSdk, isCasting, castPickerEntry, refreshPickerEntry, castToDevice, stopCasting, castPlay,
    castTogglePause, castSeek, castSkip, castSetVolume, castToggleMute, castAdjacent, castAudioTrack, castSubtitleTrack,
    remotePositionTicks,
};
