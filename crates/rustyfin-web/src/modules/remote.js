// Remote control target: while signed in, this browser keeps a socket open as its Jellyfin session, reports that
// it can be controlled (SupportsMediaControl and the commands below), and obeys the Play, Playstate and
// GeneralCommand messages other clients send it, through the player's and the queue's exports. Other modules follow
// the socket's messages with onSocketMessage and send on it with sendSocket.
import { $, toast } from './util.js';
import {
    nowPlaying, seekToTicks, playItem, closePlayer, togglePlayPause, setVolume, toggleMute, changeAudioTrack,
    changeSubtitleTrack, skipTime, playAdjacentEpisode, toggleFullscreen,
} from './player.js';
import { playContainer, playQueueAt, queueStreamType } from './queue.js';

let socket = null;
let keepAlive = null;
let retries = 0;
let retryTimer = null;
const listeners = new Map();

const video = () => $('video-player');
const clamp = v => Math.min(Math.max(v, 0), 1);

// A track menu follows a remote choice, then the player switches to it
function selectTrack(id, index, change) {
    const menu = $(id);
    if (menu) menu.value = String(index);
    change(String(index));
}

// Jellyfin's GeneralCommand names this client obeys, with their Arguments (strings, as Jellyfin sends them)
const COMMANDS = {
    SetVolume: a => setVolume(clamp(Number(a.Volume) / 100)),
    VolumeUp: () => setVolume(clamp((video()?.volume ?? 1) + 0.05)),
    VolumeDown: () => setVolume(clamp((video()?.volume ?? 1) - 0.05)),
    ToggleMute: () => toggleMute(),
    Mute: () => { if (video() && !video().muted) toggleMute(); },
    Unmute: () => { if (video()?.muted) toggleMute(); },
    SetAudioStreamIndex: a => { if (nowPlaying()) selectTrack('player-audio', Number(a.Index), changeAudioTrack); },
    SetSubtitleStreamIndex: a => { if (nowPlaying()) selectTrack('player-subs', Number(a.Index), changeSubtitleTrack); },
    DisplayMessage: a => toast([a.Header, a.Text].filter(Boolean).join(': ')),
    ToggleFullscreen: () => { if (nowPlaying()) toggleFullscreen(); },
};

// Jellyfin's PlaystateCommand names; each acts only on a playing item
const PLAYSTATE = {
    Pause: () => video()?.pause(),
    Unpause: () => video()?.play(),
    PlayPause: () => togglePlayPause(),
    Stop: () => closePlayer(),
    Seek: d => seekToTicks(Number(d.SeekPositionTicks) || 0),
    NextTrack: () => playAdjacentEpisode(1),
    PreviousTrack: () => playAdjacentEpisode(-1),
    Rewind: () => skipTime(-10),
    FastForward: () => skipTime(30),
};

// A Play message: one item starts where the controller says (StartPositionTicks, else the beginning) with its
// version and tracks; a folder plays as Play all (or Shuffle) does; several items become the play queue.
// PlayNext and PlayLast while something plays would add to the queue, which this client can't do yet.
async function remotePlay(d) {
    const ids = d?.ItemIds || [];
    if (!ids.length) return;
    const command = d.PlayCommand || 'PlayNow';
    if ((command === 'PlayNext' || command === 'PlayLast') && nowPlaying()) {
        toast(t('remote.queue_unsupported'), 'error');
        return;
    }
    const items = (await Promise.all(ids.slice(0, 300).map(id => API.get('/Items/' + encodeURIComponent(id)).catch(e => {
        console.warn('Could not load an item to play:', e);
        return null;
    })))).filter(Boolean);
    if (items.length === 1 && items[0].IsFolder) {
        playContainer(items[0].Id, { shuffle: command === 'PlayShuffle' });
        return;
    }
    const playable = items.filter(i => !i.IsFolder);
    if (playable.length === 1) {
        const item = playable[0];
        playItem(item.Id, queueStreamType(item), item.Name || '', {
            selection: {
                startTicks: Number(d.StartPositionTicks) || 0,
                mediaSourceId: d.MediaSourceId,
                audioIndex: d.AudioStreamIndex,
                subtitleIndex: d.SubtitleStreamIndex,
            },
        });
    } else if (playable.length) {
        playQueueAt(playable, Math.min(Math.max(Number(d.StartIndex) || 0, 0), playable.length - 1));
    }
}

function remotePlaystate(d) {
    const run = PLAYSTATE[d?.Command];
    if (run && nowPlaying()) run(d);
}

function remoteCommand(d) {
    const run = COMMANDS[d?.Name];
    if (run) run(d.Arguments || {});
}

function handleMessage(msg) {
    switch (msg.MessageType) {
    case 'ForceKeepAlive':
        // Jellyfin's clients send a KeepAlive at half the interval the server asks for
        clearInterval(keepAlive);
        keepAlive = setInterval(() => sendSocket('KeepAlive'), Math.max(Number(msg.Data) || 60, 10) * 500);
        break;
    case 'Play': remotePlay(msg.Data); break;
    case 'Playstate': remotePlaystate(msg.Data); break;
    case 'GeneralCommand': remoteCommand(msg.Data); break;
    }
    for (const fn of listeners.get(msg.MessageType) || []) fn(msg.Data, msg);
}

// Calls fn with the Data of each message of type, and the whole message (some events carry their
// fields at the top, without Data); 'SocketOpen' runs on each (re)connect. Returns the unsubscriber.
function onSocketMessage(type, fn) {
    if (!listeners.has(type)) listeners.set(type, new Set());
    listeners.get(type).add(fn);
    return () => listeners.get(type).delete(fn);
}

function sendSocket(type, data) {
    if (socket?.readyState !== WebSocket.OPEN) return false;
    socket.send(JSON.stringify(data === undefined ? { MessageType: type } : { MessageType: type, Data: data }));
    return true;
}

function reportCapabilities() {
    return API.post('/Sessions/Capabilities/Full', {
        PlayableMediaTypes: ['Audio', 'Video'],
        SupportedCommands: Object.keys(COMMANDS),
        SupportsMediaControl: true,
        SupportsPersistentIdentifier: false,
    }).catch(e => console.warn('Could not report the remote control capabilities:', e));
}

// Opens the socket when signed in, and again after it drops (1 s, doubling up to 30 s); signing out closes it
function connectRemote() {
    if (!API.token) {
        if (socket) socket.close();
        return;
    }
    if (socket) return;
    clearTimeout(retryTimer);
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const ws = new WebSocket(`${proto}//${location.host}/socket?ApiKey=${encodeURIComponent(API.token)}`);
    socket = ws;
    ws.onopen = () => {
        retries = 0;
        reportCapabilities();
        for (const fn of listeners.get('SocketOpen') || []) fn();
    };
    ws.onmessage = e => {
        let msg;
        try { msg = JSON.parse(e.data); } catch { return; }
        handleMessage(msg);
    };
    ws.onclose = () => {
        if (socket !== ws) return;
        socket = null;
        clearInterval(keepAlive);
        keepAlive = null;
        if (API.token) retryTimer = setTimeout(connectRemote, Math.min(30000, 1000 * 2 ** retries++));
    };
}

window.addEventListener('hashchange', connectRemote);
connectRemote();

export { onSocketMessage, sendSocket, connectRemote, COMMANDS, PLAYSTATE };
