// Remote control from this browser: the dashboard's active sessions (now playing, progress, play method, and
// pause, stop and message controls), kept current by the socket's Sessions messages, and "Play on", which hands
// the playing item over to another session at the same point.
import { $, icon, toast, escapeHtml, formatDurationSeconds } from './util.js';
import { nowPlaying, closePlayer } from './player.js';
import { onSocketMessage, sendSocket } from './remote.js';

// Jellyfin's dashboard lists the sessions active in the last 16 minutes
const ACTIVE_WITHIN_SECONDS = 960;
let unfollow = [];

const ownDeviceId = () => 'web-' + (localStorage.getItem('rustyfin_device_id') || '');

function isRecent(s) {
    const at = Date.parse(s.LastActivityDate);
    return !Number.isFinite(at) || Date.now() - at <= ACTIVE_WITHIN_SECONDS * 1000;
}

function nowPlayingTitle(item) {
    const parts = [item.SeriesName, item.Name].filter(Boolean);
    return parts.join(' – ') || item.Name || '';
}

function lastSeen(date) {
    const secs = Math.round((Date.parse(date) - Date.now()) / 1000);
    if (!Number.isFinite(secs)) return '–';
    const rtf = new Intl.RelativeTimeFormat(document.documentElement.lang || undefined, { numeric: 'auto' });
    if (secs > -60) return rtf.format(secs, 'second');
    if (secs > -3600) return rtf.format(Math.round(secs / 60), 'minute');
    return rtf.format(Math.round(secs / 3600), 'hour');
}

function controlButton(id, action, label, iconName) {
    return `<button type="button" class="jf-icon-btn !w-8 !h-8" title="${escapeHtml(label)}" aria-label="${escapeHtml(label)}"
        onclick="sessionControl('${escapeHtml(id)}', '${action}')">${icon(iconName, { size: 16, class: 'w-4 h-4' })}</button>`;
}

// One session: who and where, what plays and how far, and the controls the session supports
function sessionCard(s) {
    const item = s.NowPlayingItem;
    const ps = s.PlayState || {};
    const own = s.DeviceId === ownDeviceId();
    let playing = `<p class="text-white/40 text-xs">${t('remote.idle')}</p>`;
    if (item) {
        const total = (item.RunTimeTicks || 0) / 1e7;
        const pos = (ps.PositionTicks || 0) / 1e7;
        const pct = total > 0 ? Math.min(100, pos / total * 100) : 0;
        const method = ps.PlayMethod ? t(ps.PlayMethod === 'DirectPlay' ? 'remote.direct_play' : ps.PlayMethod === 'DirectStream' ? 'remote.direct_stream' : 'remote.transcode') : '';
        playing = `
            <p class="text-white text-sm font-semibold truncate">${ps.IsPaused ? icon('Pause', { size: 12, class: 'inline w-3 h-3 mr-1' }) : ''}${escapeHtml(nowPlayingTitle(item))}</p>
            <div class="h-1 rounded bg-white/10 my-1.5" role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow="${Math.round(pct)}">
                <div class="h-1 rounded bg-[#00a4dc]" style="width:${pct.toFixed(1)}%"></div>
            </div>
            <p class="text-white/50 text-[11px]">${formatDurationSeconds(pos)}${total > 0 ? ' / ' + formatDurationSeconds(total) : ''}${method ? ' · ' + escapeHtml(method) : ''}</p>`;
    }
    const controls = [];
    if (item && s.SupportsMediaControl) {
        controls.push(ps.IsPaused
            ? controlButton(s.Id, 'Unpause', t('remote.resume'), 'Play')
            : controlButton(s.Id, 'Pause', t('remote.pause'), 'Pause'));
        controls.push(controlButton(s.Id, 'Stop', t('remote.stop'), 'Square'));
    }
    if ((s.SupportedCommands || []).includes('DisplayMessage')) {
        controls.push(controlButton(s.Id, 'Message', t('remote.send_message'), 'MessageSquare'));
    }
    return `
        <div class="rounded-xl bg-white/5 border border-white/10 p-4 flex flex-col gap-2" data-session-id="${escapeHtml(s.Id)}">
            <div class="flex items-start justify-between gap-2">
                <div class="min-w-0">
                    <p class="text-white font-bold text-sm truncate">${escapeHtml(s.UserName || '–')}</p>
                    <p class="text-white/50 text-xs truncate">${escapeHtml(s.DeviceName || t('admin.browser'))} · ${escapeHtml(s.Client || '')}${own ? ' · ' + t('remote.this_device') : ''}</p>
                </div>
                <span class="text-white/40 text-[11px] whitespace-nowrap">${escapeHtml(lastSeen(s.LastActivityDate))}</span>
            </div>
            <div class="min-h-[2.5rem]">${playing}</div>
            ${controls.length ? `<div class="flex gap-2">${controls.join('')}</div>` : ''}
        </div>`;
}

function sessionListHtml(sessions) {
    const list = (sessions || []).filter(isRecent);
    if (!list.length) return `<p class="text-white/40 text-xs">${t('remote.no_sessions')}</p>`;
    return `<div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">${list.map(sessionCard).join('')}</div>`;
}

// The dashboard's Sessions tab body, filled from the sessions the page loaded with
function sessionsPanelHtml(sessions) {
    return `<div id="admin-sessions-list">${sessionListHtml(sessions)}</div>`;
}

function renderSessions(sessions) {
    const el = $('admin-sessions-list');
    if (el) el.innerHTML = sessionListHtml(sessions);
}

// The Sessions tab follows the server's Sessions messages while it is shown, also after a reconnect
function followSessions() {
    unfollowSessions();
    const start = () => sendSocket('SessionsStart', '0,1500');
    unfollow = [onSocketMessage('Sessions', renderSessions), onSocketMessage('SocketOpen', start)];
    start();
    API.getSessions().then(renderSessions).catch(() => {});
}

function unfollowSessions() {
    if (!unfollow.length) return;
    unfollow.forEach(off => off());
    unfollow = [];
    sendSocket('SessionsStop');
}

async function sessionControl(id, action) {
    try {
        if (action === 'Message') {
            const text = prompt(t('remote.message_prompt'));
            if (!text) return;
            await API.post(`/Sessions/${encodeURIComponent(id)}/Message`, { Header: API.userName || '', Text: text, TimeoutMs: 5000 });
            toast(t('remote.message_sent'));
        } else {
            await API.post(`/Sessions/${encodeURIComponent(id)}/Playing/${action}`);
        }
    } catch (e) {
        toast(t('remote.command_failed'), 'error');
    }
}

// The "Play on" list: the other sessions that take remote control
function playOnDialog() {
    let dialog = $('play-on-dialog');
    if (!dialog) {
        dialog = document.createElement('dialog');
        dialog.id = 'play-on-dialog';
        dialog.className = 'modal';
        dialog.addEventListener('click', e => { if (e.target === dialog) dialog.close(); });
        $('player-modal')?.appendChild(dialog) || document.body.appendChild(dialog);
    }
    return dialog;
}

async function openPlayOn() {
    if (!nowPlaying()) return;
    let sessions = [];
    try {
        sessions = await API.get(`/Sessions?ControllableByUserId=${encodeURIComponent(API.userId)}`);
    } catch (e) {
        toast(t('remote.command_failed'), 'error');
        return;
    }
    const targets = (sessions || []).filter(s => s.DeviceId !== ownDeviceId() && s.SupportsMediaControl && isRecent(s));
    if (!targets.length) {
        toast(t('remote.no_targets'), 'error');
        return;
    }
    const dialog = playOnDialog();
    dialog.innerHTML = `
        <div class="modal-box bg-[#101827] border border-white/10 max-w-sm">
            <h3 class="font-bold text-white text-base mb-3">${t('remote.play_on')}</h3>
            <ul class="flex flex-col gap-1">
                ${targets.map(s => `<li><button type="button" class="w-full text-left rounded-lg px-3 py-2 hover:bg-white/10 text-white"
                    onclick="playOnSession('${escapeHtml(s.Id)}')">
                    <span class="block text-sm font-semibold">${escapeHtml(s.DeviceName || s.Client || '')}</span>
                    <span class="block text-xs text-white/50">${escapeHtml(s.Client || '')} · ${escapeHtml(s.UserName || '')}</span>
                </button></li>`).join('')}
            </ul>
            <div class="modal-action"><button type="button" class="btn btn-sm" onclick="this.closest('dialog').close()">${t('common.cancel')}</button></div>
        </div>`;
    dialog.showModal();
}

// Hands the playing item over: the target starts it where this player is, then this player closes
async function playOnSession(id) {
    const np = nowPlaying();
    $('play-on-dialog')?.close();
    if (!np) return;
    const q = new URLSearchParams({ playCommand: 'PlayNow', itemIds: np.itemId, startPositionTicks: String(np.positionTicks) });
    try {
        await API.post(`/Sessions/${encodeURIComponent(id)}/Playing?${q}`);
        closePlayer();
        toast(t('remote.sent'));
    } catch (e) {
        toast(t('remote.command_failed'), 'error');
    }
}

export { sessionsPanelHtml, renderSessions, followSessions, unfollowSessions, sessionControl, openPlayOn, playOnSession };
