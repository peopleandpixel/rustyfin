// The navigation drawer on phones: #sidebar-mobile slides in over the page with a copy of the desktop
// sidebar's links. It closes on a tap on the overlay, on a link or item in it, on Escape and on navigation.
// No imports: util.js pulls in heavy modules, so this file reads the DOM directly.

const CLOSE_MS = 200;
let open = false;
let hideTimer = 0;

// The next frame, so the slide starts from the hidden position; a timer where there is no frame callback
function nextFrame(fn) {
    if (typeof requestAnimationFrame === 'function') requestAnimationFrame(fn);
    else setTimeout(fn, 0);
}

export function sidebarOpen() {
    return open;
}

export function openSidebar() {
    const m = document.getElementById('sidebar-mobile');
    const o = document.getElementById('sidebar-overlay');
    if (!m || !o || open) return;
    open = true;
    clearTimeout(hideTimer);
    const content = document.getElementById('sidebar-mobile-content');
    if (content) content.innerHTML = document.getElementById('sidebar')?.innerHTML || '';
    m.classList.remove('hidden');
    o.classList.remove('hidden');
    nextFrame(() => { if (open) m.classList.add('is-open'); });
    document.getElementById('sidebar-btn')?.setAttribute('aria-expanded', 'true');
}

export function closeSidebar() {
    const m = document.getElementById('sidebar-mobile');
    const o = document.getElementById('sidebar-overlay');
    if (!open || !m || !o) return;
    open = false;
    m.classList.remove('is-open');
    o.classList.add('hidden');
    document.getElementById('sidebar-btn')?.setAttribute('aria-expanded', 'false');
    clearTimeout(hideTimer);
    // Hidden once the slide-out ends, unless the drawer was opened again meanwhile
    hideTimer = setTimeout(() => { if (!open) m.classList.add('hidden'); }, CLOSE_MS);
}

export function toggleSidebar() {
    if (open) closeSidebar();
    else openSidebar();
}

document.addEventListener('keydown', e => {
    if (e.key === 'Escape') closeSidebar();
});

document.getElementById('sidebar-mobile')?.addEventListener('click', e => {
    if (e.target.closest('a,.jf-drawer-item')) closeSidebar();
});
