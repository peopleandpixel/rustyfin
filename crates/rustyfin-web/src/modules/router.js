// Hash routing, page dispatch, the user menu, click actions and start-up
import { $, toast } from './util.js';
import { checkSetupWizard, wizAddManualLibrary } from './wizard.js';
import { startOidcSignIn, startOidcLink, unlinkOidc, renderLogin, updateAuthUI } from './auth.js';
import { renderHome, renderFavorites } from './home.js';
import {
    renderLibrary, stopLibraryLoad, libraryRemember, startGridDiag, jumpToLetter, clearLibraryFilters,
} from './library.js';
import { currentDetailId, renderDetail, renderPerson, toggleFavorite, togglePlayed } from './item.js';
import { renderSearch } from './search.js';
import { playWithResumeChoice, stopPlayback, playItem, playerAutoHide, onPlayerClosed } from './player.js';
import { playContainer } from './queue.js';
import { renderSettings } from './settings.js';
import {
    renderAdmin, showAdminTab, setUserPolicy, removeOidcLinkAsAdmin, copyOidcCallbackUrl, saveOidcAdminSettings,
    startTaskAction, stopTaskAction,
} from './admin.js';
import {
    loadSidebar, deleteLibraryFromAdmin, toggleRealtimeMonitor, openFolderBrowser, folderBrowserLoad, folderBrowserUp,
    folderBrowserSelect, closeFolderBrowser, addLibraryPathRow, removeLibraryPathRow, initScanProgressWebSocket,
} from './library-admin.js';
import { openPluginConfig, openPluginPriority } from './plugins.js';
let currentPage = 'home';

// --- Navigation & Routing ---
// The hash navigate() set last (see handleHash)
let navigatedHash = null;

function navigate(page, data, { fromHistory = false } = {}) {
    // The library list remembers where it was, and stops loading once the page is left; it comes
    // back to that place only through history
    libraryRemember();
    if (page !== 'library') stopLibraryLoad();
    libraryRestore = fromHistory && page === 'library';
    document.querySelectorAll('[id^="page-"]').forEach(el => el.classList.add('hidden'));
    const el = $('page-' + page);
    if (el) {
        el.classList.remove('hidden');
        el.classList.add('jf-page');
    }
    currentPage = page;
    navigatedHash = data ? `${page}/${data}` : page;
    window.location.hash = navigatedHash;

    // Update sidebar active states
    document.querySelectorAll('.jf-drawer-item').forEach(item => item.classList.remove('active'));
    if (page === 'home') $('side-nav-home')?.classList.add('active');
    if (page === 'favorites') $('side-nav-fav')?.classList.add('active');
    if (page === 'admin') $('side-nav-admin')?.classList.add('active');
    if (page === 'settings') $('side-nav-settings')?.classList.add('active');

    const pages = {
        home: renderHome,
        favorites: renderFavorites,
        login: () => renderLogin(data),
        library: () => renderLibrary(data),
        detail: () => renderDetail(data),
        search: () => renderSearch(data),
        person: () => renderPerson(data),
        settings: () => renderSettings(data),
        admin: () => renderAdmin(data)
    };

    if (pages[page]) pages[page]();
    closeUserMenu();
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

// --- UI Navigation Helpers ---
function toggleUserMenu() { $('user-dropdown')?.classList.toggle('hidden'); }
function closeUserMenu() { $('user-dropdown')?.classList.add('hidden'); }
document.addEventListener('click', e => {
    if (!e.target.closest('#nav-user-menu')) closeUserMenu();
});

// Toasts that a scan started, once. Refresh answers only when the scan ends, so the toast shows 300 ms after
// sending, or when the request ends if sooner; a request that fails before then, such as a 404 for a deleted
// library, shows only the error.
function startScanWithToast(request, startedMessage) {
    let started = false;
    const showStarted = () => {
        if (started) return;
        started = true;
        toast(startedMessage);
    };
    const timer = setTimeout(showStarted, 300);
    return request.then(showStarted, err => {
        clearTimeout(timer);
        toast(t(started ? 'library.scan_failed' : 'library.scan_start_failed', { message: err.message }), 'error');
    }).finally(() => clearTimeout(timer));
}

// The refresh answers only when the server has finished, so the button stays busy until then, and the open page is re-rendered from fresh data.
async function refreshItemMetadata(button, id) {
    if (button.disabled) return;
    const busy = t('detail.metadata_refreshing');
    button.disabled = true;
    button.setAttribute('aria-busy', 'true');
    button.classList.add('is-busy');
    button.setAttribute('aria-label', busy);
    button.setAttribute('title', busy);
    try {
        await API.refreshItem(id);
        API.invalidateCache('/Items');
        API.invalidateCache('/Shows');
        toast(t('detail.metadata_refreshed'));
        if (currentPage === 'detail' && currentDetailId === id) await renderDetail(id);
    } catch (err) {
        if (err.name === 'TimeoutError') toast(t('detail.metadata_refresh_timeout'), 'error');
        else toast(t('detail.metadata_refresh_failed', { message: err.message }), 'error');
    } finally {
        button.disabled = false;
        button.removeAttribute('aria-busy');
        button.classList.remove('is-busy');
        button.setAttribute('aria-label', t('detail.refresh_metadata'));
        button.setAttribute('title', t('detail.refresh_metadata'));
    }
}

// Clicks on rendered markup are dispatched here from data-action and data-* attributes, so server values never end up inside inline JavaScript.
const clickActions = {
    navigate: el => navigate(el.dataset.page, el.dataset.id),
    // A video with a resume point asks first; the detail page's Resume and Play from beginning buttons are that choice
    play: el => {
        const { id, streamType, title } = el.dataset;
        if (el.dataset.resume === 'true') playItem(id, streamType, title);
        else if (el.dataset.fromStart === 'true') playItem(id, streamType, title, { fromStart: true });
        else playWithResumeChoice(id, streamType, title);
    },
    // Play all and Shuffle on a series, season, album, artist, playlist, collection or library
    'play-all': el => playContainer(el.dataset.id),
    shuffle: el => playContainer(el.dataset.id, { shuffle: true }),
    'scan-library': el => startScanWithToast(API.scanLibrary(el.dataset.id), t('library.scan_started_for', { name: el.dataset.name })),
    'scan-library-page': el => startScanWithToast(API.scanLibrary(el.dataset.id), t('library.media_scan_started_for', { name: el.dataset.name })),
    'scan-library-admin': el => startScanWithToast(API.scanLibrary(el.dataset.id), t('admin.scan_started')),
    'scan-all': () => startScanWithToast(API.scanAllLibraries(), t('admin.scanning_all')),
    'jump-letter': el => jumpToLetter(el.dataset.id, el.dataset.letter),
    'clear-library-filters': el => clearLibraryFilters(el.dataset.id),
    'toggle-favorite': el => toggleFavorite(el.dataset.id, el.dataset.favorite === 'true'),
    'toggle-played': el => togglePlayed(el.dataset.id, el.dataset.played === 'true'),
    'refresh-item': el => refreshItemMetadata(el, el.dataset.id),
    'admin-tab': el => showAdminTab(el, el.dataset.tab),
    'delete-user': el => { if (confirm(t('admin.delete_user_confirm'))) API.deleteUser(el.dataset.id).then(() => renderAdmin()); },
    'set-user-admin': el => setUserPolicy(el, { IsAdministrator: el.checked }),
    'set-user-disabled': el => setUserPolicy(el, { IsDisabled: el.checked }),
    'delete-library': el => deleteLibraryFromAdmin(el.dataset.id),
    'toggle-realtime-monitor': el => toggleRealtimeMonitor(el),
    'start-task': el => startTaskAction(el.dataset.id, el.dataset.name),
    'stop-task': el => stopTaskAction(el.dataset.id, el.dataset.name),
    'browse-path': el => openFolderBrowser(el.closest('.path-field').querySelector('[data-path-field]')),
    'add-lib-path': el => addLibraryPathRow(el.closest('.wiz-man-lib')?.querySelector('.wiz-man-paths') || $('add-lib-paths')),
    'remove-lib-path': el => removeLibraryPathRow(el),
    'wiz-add-lib': () => wizAddManualLibrary(),
    'wiz-remove-lib': el => { el.closest('.wiz-man-lib').remove(); document.querySelector('[data-action="wiz-add-lib"]').focus(); },
    'folder-open': el => folderBrowserLoad(el.dataset.path),
    'folder-up': () => folderBrowserUp(),
    'folder-cancel': () => closeFolderBrowser(),
    'folder-select': () => folderBrowserSelect(),
    'oidc-sign-in': () => startOidcSignIn(),
    'oidc-link': () => startOidcLink(),
    'oidc-unlink': el => unlinkOidc(el.dataset.id),
    'oidc-remove-link': el => removeOidcLinkAsAdmin(el),
    'oidc-copy-callback': () => copyOidcCallbackUrl(),
    'oidc-save-settings': () => saveOidcAdminSettings(),
    'plugin-edit-config': el => openPluginConfig(el.dataset.pluginId),
    'plugin-edit-priority': el => openPluginPriority(el.dataset.pluginId),
};
document.addEventListener('click', e => {
    const el = e.target.closest('[data-action]');
    if (el) clickActions[el.dataset.action]?.(el, e);
});

function toggleSidebar() {
    const m = $('sidebar-mobile'), o = $('sidebar-overlay');
    if (!m || !o) return;
    if (m.classList.contains('hidden')) {
        m.classList.remove('hidden');
        o.classList.remove('hidden');
        setTimeout(() => m.style.transform = 'translateX(0)', 10);
        const content = $('sidebar')?.innerHTML || '';
        $('sidebar-mobile-content').innerHTML = content;
    } else {
        m.style.transform = 'translateX(-100%)';
        o.classList.add('hidden');
        setTimeout(() => m.classList.add('hidden'), 200);
    }
}

// --- Hash Router ---
// A hash route (location.hash without its #) as { page, param, query }: a leading slash is
// allowed (#/library/<id>), and a ?query after the route (#library/<id>?debug=grid) is split off,
// so it never becomes part of the page's parameter (nor reaches the server in it)
function hashRoute(hash) {
    const at = hash.indexOf('?');
    const path = (at < 0 ? hash : hash.slice(0, at)).replace(/^\/+/, '');
    const query = new URLSearchParams(at < 0 ? '' : hash.slice(at + 1));
    const [page, ...rest] = path.split('/');
    return { page, param: rest.join('/') || undefined, query };
}

function handleHash() {
    const hash = window.location.hash.slice(1);
    // navigate() sets the hash itself; a change to any other hash came from history (Back/Forward)
    const fromHistory = hash !== navigatedHash;
    const { page, param, query } = hashRoute(hash);
    // debug=grid is kept for the tab (sessionStorage), so a reload keeps the panel: the hash
    // itself is cleaned just below
    if (query.get('debug') === 'grid') {
        try { sessionStorage.setItem('rustyfin.gridDebug', '1'); } catch {}
        startGridDiag(true);
    }
    if (!page) {
        navigate('home');
        return;
    }
    // A hash typed in another form (a leading slash, a ?query) becomes the canonical one in place:
    // navigate() then sets the hash it already has, so no history entry is added and no second
    // hashchange draws the page again (Back would land on the typed form and come straight back)
    const canonical = param ? `${page}/${param}` : page;
    if (hash !== canonical) history.replaceState(null, '', `#${canonical}`);
    navigate(page, param, { fromHistory });
}

// --- Bootstrap & Lifecycle ---
window.addEventListener('hashchange', handleHash);

document.addEventListener('DOMContentLoaded', async () => {
    // Every way of closing the player goes through here, so the session is stopped and its position reported once
    if ($('player-modal') && $('video-player')) playerBars = playerAutoHide($('player-modal'), $('video-player'));
    $('player-modal')?.addEventListener('close', onPlayerClosed);
    window.addEventListener('pagehide', () => stopPlayback());
    updateAuthUI();
    const wizardActive = await checkSetupWizard();
    if (!wizardActive) {
        await loadSidebar();
        handleHash();
    }
    // Initialize WebSocket for scan progress notifications
    if (API.isLoggedIn()) {
        initScanProgressWebSocket();
    }
});

export { currentPage, navigate, toggleUserMenu, closeUserMenu, toggleSidebar, hashRoute };
