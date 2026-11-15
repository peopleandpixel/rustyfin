// Shared helpers: DOM shortcuts, icons, toasts, escaping, scan rings, time formats; the library list
import { scanStatuses } from './library-admin.js';
let libraries = [];

// --- DOM & Helper Utilities ---
function $(id) { return document.getElementById(id); }
function html(id, content) { const el = $(id); if (el) el.innerHTML = content; }
function show(id) {
    const el = $(id);
    if (!el) return;
    el.classList.remove('hidden');
    el.style.removeProperty('display');
}
function hide(id) {
    const el = $(id);
    if (!el) return;
    el.classList.add('hidden');
    el.style.setProperty('display', 'none', 'important');
}

function initLazyImages(root = document) {
    // Hover-based full-res image loading: swap thumbnail for full-res on card hover
    root.querySelectorAll('img.jf-thumb-img[data-full-src]').forEach(img => {
        if (img.dataset.lazyBound) return;
        const card = img.closest('.jf-card');
        if (!card) return;
        img.dataset.lazyBound = '1';

        card.addEventListener('mouseenter', () => {
            if (img.dataset.fullLoaded) return;
            img.dataset.fullLoaded = '1';
            const fullSrc = img.dataset.fullSrc;
            const fullImg = new Image();
            fullImg.decode().then(() => {
                img.src = fullSrc;
                img.style.opacity = '1';
                img.classList.remove('blur-sm', 'scale-105');
            }).catch(() => {
                // Full-res failed; keep thumbnail
                delete img.dataset.fullLoaded;
            });
            fullImg.src = fullSrc;
        }, { once: true });
    });
}

// Lucide icon as an SVG string. name is a key of window.lucideIcons (src/icons.js); an unknown name gives ''.
function icon(name, { size = 16, class: cls = '', label } = {}) {
    const nodes = window.lucideIcons?.[name];
    if (!nodes) return '';
    const children = nodes.map(([tag, attrs]) => `<${tag} ${Object.entries(attrs).map(([k, v]) => `${k}="${escapeHtml(v)}"`).join(' ')}/>`).join('');
    const a11y = label ? `role="img" aria-label="${escapeHtml(label)}"` : 'aria-hidden="true"';
    return `<svg xmlns="http://www.w3.org/2000/svg" width="${size}" height="${size}" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"${cls ? ` class="${escapeHtml(cls)}"` : ''} ${a11y}>${children}</svg>`;
}

function toast(msg, type = 'success') {
    const c = $('toast-container');
    if (!c) return;
    const div = document.createElement('div');
    const isError = type === 'error';
    div.className = 'jf-toast pointer-events-auto flex items-center gap-3 px-4 py-3 rounded-xl shadow-2xl text-xs font-semibold backdrop-blur-xl border ' +
        (isError ? 'bg-red-500/90 text-white border-red-400/40' : 'bg-[#0f172a]/95 text-white border-[#00a4dc]/40');
    div.innerHTML = `
        <span class="w-2 h-2 rounded-full ${isError ? 'bg-red-300' : 'bg-[#00a4dc]'} shadow-sm"></span>
        <span class="leading-snug">${escapeHtml(msg)}</span>
    `;
    c.appendChild(div);
    setTimeout(() => {
        div.classList.add('removing');
        setTimeout(() => div.remove(), 250);
    }, 3200);
}

function escapeHtml(str) {
    if (!str) return '';
    return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// Inner markup of a scan ring: nothing when the library is not scanning, else a progressbar with a track and an arc
function scanRingInner(libId, name) {
    const s = scanStatuses[libId];
    if (s === undefined) return '';
    const p = s.percent;
    const known = p > 0;
    const label = escapeHtml(s.queued ? t('library.scan_queued', { name }) : known ? t('library.scanning_percent', { name, percent: p }) : t('library.scanning', { name }));
    const valueNow = known && !s.queued ? ` aria-valuenow="${p}"` : '';
    const spin = known || s.queued ? '' : ' motion-safe:animate-spin';
    const arc = known ? p : 25;
    // A queued scan has no arc: only the track, drawn dashed
    const track = s.queued
        ? '<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2.5" opacity="0.25" pathLength="100" stroke-dasharray="6.25 6.25"/>'
        : '<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2.5" opacity="0.25"/>';
    const arcCircle = s.queued ? '' : `<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" pathLength="100" stroke-dasharray="${arc} 100" transform="rotate(-90 8 8)"/>`;
    return `<span role="progressbar" class="block w-full h-full text-[#00a4dc]" title="${label}" aria-label="${label}" aria-valuemin="0" aria-valuemax="100"${valueNow}>`
        + `<svg aria-hidden="true" viewBox="0 0 16 16" class="w-full h-full${spin}">`
        + track + arcCircle
        + '</svg></span>';
}

// A ring next to a library name, sized by `size`. The wrapper is hidden by empty:hidden while the library is not scanning.
function scanRing(libId, name, size = 'w-4 h-4') {
    return `<span data-scan-ring="${escapeHtml(libId)}" data-name="${escapeHtml(name)}" class="inline-flex flex-shrink-0 ${size} empty:hidden">${scanRingInner(libId, name)}</span>`;
}

// Redraws the rings of one library, or of every library when called without an id
function updateScanRings(libId) {
    const selector = libId === undefined ? '[data-scan-ring]' : `[data-scan-ring="${CSS.escape(libId)}"]`;
    document.querySelectorAll(selector).forEach(el => {
        el.innerHTML = scanRingInner(el.dataset.scanRing, el.dataset.name);
    });
}

function formatRuntime(ticks) {
    if (!ticks) return '';
    const totalMinutes = Math.round(ticks / 600000000);
    if (totalMinutes < 60) return t('common.minutes_short', { count: totalMinutes });
    const hours = Math.floor(totalMinutes / 60);
    const mins = totalMinutes % 60;
    const hoursText = t('common.hours_short', { count: hours });
    if (mins === 0) return hoursText;
    return t('common.hours_minutes_short', { hours: hoursText, minutes: t('common.minutes_short', { count: mins }) });
}

function formatDurationSeconds(secs) {
    if (isNaN(secs) || secs === null) return '0:00';
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s < 10 ? '0' : ''}${s}`;
}

export {
    $, html, show, hide, initLazyImages, icon, toast, escapeHtml, scanRing, updateScanRings, formatRuntime,
    formatDurationSeconds,
};

// Shared state that inline handlers or other modules assign: reached through window
Object.defineProperties(window, {
    libraries: { get: () => libraries, set: value => { libraries = value; } },
});
