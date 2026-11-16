// Jellyfin Web's backdrop screensaver: after the user's idle time with no input, the backdrops of random movies,
// shows and artists fade one into the next until any input. Off by default (the user's Display setting
// `screensaver`, kept on the device as Jellyfin Web keeps it), and never while anything plays.
import { escapeHtml } from './util.js';
import { userSetting, SCREENSAVER_BACKDROP, DEFAULT_IDLE_SECONDS } from './user-settings.js';
import { nowPlaying } from './player.js';

const SLIDE_MS = 15000;
const CHECK_MS = 5000;

let lastInput = Date.now();
let shown = null; // { el, items, index, timer }
let opening = false;
// The click that follows the press that ended the screensaver is dropped until then
let dropClickUntil = 0;

function idleSeconds(get = userSetting) {
    const value = Number(get('screensaverTime', String(DEFAULT_IDLE_SECONDS)));
    return Number.isFinite(value) && value > 0 ? value : DEFAULT_IDLE_SECONDS;
}

// Whether anything plays: the player's session (video or music, paused too), the player dialog, or any media element
function isPlaying() {
    if (nowPlaying()) return true;
    if (document.getElementById('player-modal')?.open) return true;
    return [...document.querySelectorAll('video, audio')].some(m => !m.paused && !m.ended);
}

// Whether the screensaver may start now
function screensaverDue(now = Date.now(), get = userSetting) {
    if (!API.isLoggedIn() || get('screensaver', 'none') !== SCREENSAVER_BACKDROP) return false;
    if (document.visibilityState === 'hidden' || isPlaying()) return false;
    return now - lastInput >= idleSeconds(get) * 1000;
}

function backdropItemsPath() {
    const q = new URLSearchParams({
        ImageTypes: 'Backdrop', EnableImageTypes: 'Backdrop', IncludeItemTypes: 'Movie,Series,MusicArtist',
        SortBy: 'Random', Recursive: 'true', ImageTypeLimit: '1', Limit: '200',
    });
    return `/Users/${encodeURIComponent(API.userId)}/Items?${q}`;
}

function slideHtml(item) {
    const tag = item.BackdropImageTags?.[0];
    const src = API.imageUrl(item.Id, 'Backdrop/0', { tag, maxWidth: 1920, quality: 90 });
    return `<img class="jf-screensaver-img" alt="" src="${escapeHtml(src)}">
        <p class="jf-screensaver-name">${escapeHtml(item.Name || '')}</p>`;
}

function showSlide() {
    if (!shown) return;
    const item = shown.items[shown.index % shown.items.length];
    shown.index += 1;
    const slide = document.createElement('div');
    slide.className = 'jf-screensaver-slide';
    slide.innerHTML = slideHtml(item);
    const img = slide.querySelector('img');
    // Fades in once loaded; the slide before it goes when the new one is shown
    img.addEventListener('load', () => {
        slide.classList.add('is-shown');
        const old = [...shown?.el.querySelectorAll('.jf-screensaver-slide') || []].filter(s => s !== slide);
        setTimeout(() => old.forEach(s => s.remove()), 1200);
    }, { once: true });
    img.addEventListener('error', () => slide.remove(), { once: true });
    shown.el.appendChild(slide);
}

async function showScreensaver() {
    if (shown || opening) return;
    opening = true;
    let items = [];
    try {
        const answer = await API.get(backdropItemsPath());
        items = (answer.Items || []).filter(i => i.BackdropImageTags?.length);
    } catch {
        items = [];
    } finally {
        opening = false;
    }
    // Input or playback while the list loaded keeps it closed
    if (!items.length || shown || !screensaverDue()) return;
    const el = document.createElement('div');
    el.id = 'screensaver';
    el.className = 'jf-screensaver';
    el.setAttribute('aria-hidden', 'true');
    document.body.appendChild(el);
    shown = { el, items, index: 0, timer: setInterval(showSlide, SLIDE_MS) };
    showSlide();
}

function hideScreensaver() {
    if (!shown) return;
    clearInterval(shown.timer);
    shown.el.remove();
    shown = null;
}

// Any input wakes the page. The press or key that ends the screensaver does nothing else.
function onInput(e) {
    lastInput = Date.now();
    if (!shown) return;
    hideScreensaver();
    if (e.type === 'keydown' || e.type === 'pointerdown') {
        e.preventDefault();
        e.stopPropagation();
        if (e.type === 'pointerdown') dropClickUntil = Date.now() + 600;
    }
}

for (const type of ['pointermove', 'pointerdown', 'keydown', 'wheel', 'touchstart']) {
    document.addEventListener(type, onInput, { capture: true, passive: type !== 'keydown' && type !== 'pointerdown' });
}
document.addEventListener('click', e => {
    if (Date.now() >= dropClickUntil) return;
    dropClickUntil = 0;
    e.preventDefault();
    e.stopPropagation();
}, true);

setInterval(() => {
    if (shown && (isPlaying() || !API.isLoggedIn())) hideScreensaver();
    else if (!shown && screensaverDue()) showScreensaver();
}, CHECK_MS);

export {
    idleSeconds, isPlaying, screensaverDue,
    showScreensaver, hideScreensaver, onInput, backdropItemsPath,
};
