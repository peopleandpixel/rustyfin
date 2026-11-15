// Library management: the sidebar list, adding and removing libraries, the folder browser, scan progress
import { $, show, hide, icon, toast, escapeHtml, scanRing, updateScanRings } from './util.js';
import { currentPage, navigate } from './router.js';
import { renderHome } from './home.js';
import { currentLibraryId, currentLibraryRoute, renderLibrary, libraryRefreshInPlace } from './library.js';
import { currentDetailId, renderDetail } from './item.js';
import { renderAdmin, refreshTasks } from './admin.js';
// Library id -> { percent, queued } while it scans (percent 0 = not known yet, so its ring spins; queued = waiting its turn). Absent when not scanning.
let scanStatuses = {};

// --- Sidebar Loader ---
async function loadSidebar() {
    if (!API.isLoggedIn()) {
        hide('sidebar');
        return;
    }
    show('sidebar');
    if (API.isAdmin) {
        show('sidebar-add-lib');
        show('sidebar-admin-section');
    } else {
        hide('sidebar-add-lib');
        hide('sidebar-admin-section');
    }
    let libs;
    try {
        libs = await API.getLibraries();
    } catch (err) {
        toast(t('library.list_failed', { message: err.message }), 'error');
        return;
    }
    libraries = libs;
    const list = $('sidebar-lib-list');
    if (!list) return;
    list.innerHTML = '';
    const icons = {
        movies: 'Film', tvshows: 'Tv', music: 'Music', photos: 'Image', mixed: 'Folder',
        Movies: 'Film', TvShows: 'Tv', Music: 'Music', Photos: 'Image', Mixed: 'Folder'
    };
    libraries.forEach(lib => {
        const id = lib.id || lib.Id;
        const name = lib.name || lib.Name;
        const type = lib.content_type || lib.ContentType || 'Mixed';
        list.innerHTML += `
            <div class="flex items-center gap-1 group">
                <a data-action="navigate" data-page="library" data-id="${escapeHtml(id)}" class="jf-drawer-item ${currentLibraryId === id && currentPage === 'library' ? 'active' : ''} flex-1">
                    <span class="inline-flex">${icon(icons[type] || 'Folder', { size: 16 })}</span>
                    <span class="truncate">${escapeHtml(name)}</span>
                    ${scanRing(id, name)}
                </a>
                <button type="button" class="jf-icon-btn !w-6 !h-6 text-white/20 hover:text-white opacity-0 group-hover:opacity-100 transition flex-shrink-0" data-action="scan-library" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}" title="${escapeHtml(t('library.scan_library'))}">
                    ${icon('RefreshCw', { size: 12 })}
                </button>
            </div>`;
    });
}

// --- Add Library Modal Handlers ---
function openAddLibrary() {
    if (!API.isLoggedIn()) {
        toast(t('login.sign_in_first'), 'error');
        navigate('login');
        return;
    }
    const dlg = $('add-library-modal');
    if (!dlg) return;
    const form = $('add-library-form');
    if (form) form.reset();
    dlg.classList.add('modal-open');
    if (typeof dlg.showModal === 'function') {
        try { dlg.showModal(); } catch { dlg.setAttribute('open', ''); }
    } else {
        dlg.setAttribute('open', '');
    }
}

function closeAddLibrary() {
    const dlg = $('add-library-modal');
    if (dlg) {
        dlg.classList.remove('modal-open');
        if (typeof dlg.close === 'function') {
            try { dlg.close(); } catch { dlg.removeAttribute('open'); }
        } else {
            dlg.removeAttribute('open');
        }
    }
}

async function deleteLibraryFromAdmin(id) {
    if (!confirm(t('admin.remove_library_confirm'))) return;
    try {
        await API.deleteLibrary(id);
    } catch (err) {
        toast(t('library.delete_failed', { message: err.message }), 'error');
        return;
    }
    await loadSidebar();
    renderAdmin();
}

// Saves the checkbox's new state; a failure puts the checkbox back and shows a toast
async function toggleRealtimeMonitor(el) {
    const enabled = el.checked;
    el.disabled = true;
    try {
        const options = await API.getLibraryOptions(el.dataset.id);
        await API.saveLibraryOptions(el.dataset.id, { ...options, EnableRealtimeMonitor: enabled });
    } catch (err) {
        el.checked = !enabled;
        toast(t('admin.realtime_monitor_failed', { message: err.message }), 'error');
    } finally {
        el.disabled = false;
    }
}

// Shows a message inside the add-library dialog; an empty message hides the box
function setAddLibraryError(message) {
    const el = $('add-lib-error');
    if (!el) return;
    el.textContent = message || '';
    el.classList.toggle('hidden', !message);
}

async function submitAddLibrary(e) {
    e.preventDefault();
    const f = e.target;
    const name = f.name.value.trim();
    const type = f.type.value;
    const paths = [...new Set([...f.querySelectorAll('[name="paths"]')].map(i => i.value.trim()).filter(Boolean))];
    const submitBtn = $('add-lib-submit');

    setAddLibraryError('');
    if (!name || !paths.length) {
        setAddLibraryError(t('library.name_and_path_required'));
        return;
    }
    let lib;
    if (submitBtn) submitBtn.disabled = true;
    try {
        lib = await API.createLibrary(name, type, paths, { EnableRealtimeMonitor: f.realtimeMonitor.checked });
    } catch (err) {
        setAddLibraryError(t('library.create_failed', { message: err.message }));
        return;
    } finally {
        if (submitBtn) submitBtn.disabled = false;
    }
    toast(t('library.created', { name }));
    closeAddLibrary();
    // Auto-start scan after creating library
    API.scanLibrary(lib.id || lib.Id).catch(err => toast(t('library.scan_start_failed', { message: err.message }), 'error'));
    // The library exists at this point, so a reload error is not a creation failure: toast it as before
    try {
        await loadSidebar();
        if (currentPage === 'admin') renderAdmin();
        else if (currentPage === 'home') renderHome();
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- Server path fields: autofill and Browse ---
// The parent of an absolute path ("/" for top-level folders and for "/" itself)
function folderParent(path) {
    const trimmed = path.replace(/\/+$/, '');
    return trimmed.slice(0, trimmed.lastIndexOf('/')) || '/';
}

// Suggestions: one shared timer, so only the last field typed in gets its suggestions
let pathSuggestTimer = null;
document.addEventListener('input', e => {
    if (!e.target.matches?.('[data-path-field]')) return;
    const input = e.target;
    clearTimeout(pathSuggestTimer);
    pathSuggestTimer = setTimeout(() => suggestPaths(input), 200);
});

// Per field: busy while a suggestion request is in flight, again when the field changed meanwhile
const pathSuggestStates = new WeakMap();

// At most one request per field: a change during a request queues one more run after it
async function suggestPaths(input) {
    const state = pathSuggestStates.get(input) || { busy: false, again: false };
    pathSuggestStates.set(input, state);
    if (state.busy) {
        state.again = true;
        return;
    }
    state.busy = true;
    try {
        await loadPathSuggestions(input);
    } finally {
        state.busy = false;
    }
    if (state.again) {
        state.again = false;
        suggestPaths(input);
    }
}

async function loadPathSuggestions(input) {
    const list = $('path-suggestions');
    if (!list) return;
    const path = input.value.trim();
    const raw = input.value;
    if (!path.startsWith('/')) {
        list.replaceChildren();
        return;
    }
    const cut = path.lastIndexOf('/');
    let entries;
    try {
        entries = await API.directoryContents(cut === 0 ? '/' : path.slice(0, cut), path.slice(cut + 1));
    } catch {
        entries = [];
    }
    if (input.value !== raw) return; // the field changed while the request ran
    list.replaceChildren(...entries.slice(0, 50).map(entry => {
        const option = document.createElement('option');
        option.value = entry.Path;
        return option;
    }));
}

// The folder browser dialog fills the field it was opened from
let folderBrowserInput = null;
// The folder the dialog lists; '' until a listing succeeds
let folderBrowserPath = '';
// Numbers each listing request; only the latest may touch the dialog
let folderBrowserSeq = 0;

async function openFolderBrowser(input) {
    const dlg = $('folder-browser');
    if (!dlg || !input) return;
    folderBrowserInput = input;
    folderBrowserPath = '';
    dlg.classList.add('modal-open');
    if (typeof dlg.showModal === 'function') {
        try { dlg.showModal(); } catch { dlg.setAttribute('open', ''); }
    } else {
        dlg.setAttribute('open', '');
    }
    // Start at the typed folder, else its parent, else the root; stops when the dialog closes or another load starts
    let start = input.value.trim();
    if (!start.startsWith('/')) start = '/';
    for (const path of new Set([start, folderParent(start), '/'])) {
        const load = folderBrowserLoad(path);
        const seq = folderBrowserSeq; // folderBrowserLoad takes its number synchronously
        if (await load) return;
        if (seq !== folderBrowserSeq || !dlg.open) return;
    }
}

// Lists the folders in path; false if it cannot be read (the error is shown in the list) or a newer load superseded it
async function folderBrowserLoad(path) {
    const list = $('folder-browser-list');
    const dlg = $('folder-browser');
    if (!list || !dlg) return false;
    const seq = ++folderBrowserSeq;
    const isCurrent = () => seq === folderBrowserSeq && dlg.open;
    const upBtn = dlg.querySelector('[data-action="folder-up"]');
    const selectBtn = dlg.querySelector('[data-action="folder-select"]');
    list.innerHTML = '<div class="p-3 text-center text-white/40 text-xs"><div class="w-4 h-4 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin mx-auto"></div></div>';
    let entries;
    try {
        entries = await API.directoryContents(path);
    } catch (e) {
        if (!isCurrent()) return false;
        // The failed path becomes the dialog's path, so Up goes to its parent and Select stays off
        folderBrowserPath = path;
        $('folder-browser-path').textContent = path;
        upBtn.disabled = path === '/';
        selectBtn.disabled = true;
        list.innerHTML = `<div class="p-3 text-center text-red-400 text-xs">${escapeHtml(e.message)}</div>`;
        (upBtn.disabled ? dlg.querySelector('[data-action="folder-cancel"]') : upBtn).focus();
        return false;
    }
    if (!isCurrent()) return false;
    folderBrowserPath = path;
    $('folder-browser-path').textContent = path;
    upBtn.disabled = path === '/';
    selectBtn.disabled = false;
    if (!entries.length) {
        list.innerHTML = `<div class="p-3 text-center text-white/30 text-xs">${t('library.no_subfolders')}</div>`;
    } else {
        list.innerHTML = entries.map(entry => `<button type="button" class="w-full text-left px-3 py-2 text-xs text-white hover:bg-white/5 flex items-center gap-2 rounded" data-action="folder-open" data-path="${escapeHtml(entry.Path)}">${icon('Folder', { size: 14, class: 'inline-block align-[-2px] mr-1.5' })}<span class="truncate min-w-0">${escapeHtml(entry.Name)}</span></button>`).join('');
    }
    const first = list.querySelector('[data-action="folder-open"]');
    (first || $('folder-browser').querySelector('[data-action="folder-up"]')).focus();
    return true;
}

function folderBrowserUp() {
    if (folderBrowserPath) folderBrowserLoad(folderParent(folderBrowserPath));
}

// Puts the listed folder into the field and closes the dialog
function folderBrowserSelect() {
    const input = folderBrowserInput;
    if (input && folderBrowserPath) {
        input.value = folderBrowserPath;
        input.dispatchEvent(new Event('input', { bubbles: true }));
    }
    closeFolderBrowser();
}

function closeFolderBrowser() {
    const dlg = $('folder-browser');
    if (dlg?.open) dlg.close();
}

// However the dialog closes (Cancel, Select or Escape), focus returns to the field it filled
document.addEventListener('close', e => {
    if (e.target.id !== 'folder-browser') return;
    e.target.classList.remove('modal-open');
    folderBrowserInput?.focus();
}, true);

// Adds a path row to Add Library: a copy of the first row (its Browse button included) with a remove button
function addLibraryPathRow(list) {
    const first = list?.querySelector('.path-field');
    if (!first) return;
    const row = first.cloneNode(true);
    const input = row.querySelector('input');
    input.value = '';
    input.removeAttribute('id');
    input.removeAttribute('aria-invalid');
    input.classList.remove('ring-1', 'ring-red-500');
    const remove = document.createElement('button');
    remove.type = 'button';
    remove.className = 'jf-btn-ghost !px-3 !py-2 text-xs';
    remove.dataset.action = 'remove-lib-path';
    remove.setAttribute('aria-label', t('library.remove_path'));
    remove.textContent = '×';
    row.appendChild(remove);
    list.appendChild(row);
    input.focus();
}

function removeLibraryPathRow(el) {
    el.closest('.path-field')?.remove();
}

// Leaves only the first path row, empty
function resetAddLibraryPaths() {
    const rows = $('add-lib-paths')?.querySelectorAll('.path-field') || [];
    rows.forEach((row, i) => {
        if (i) row.remove();
        else row.querySelector('input').value = '';
    });
}

// Override openAddLibrary to reset state
const _origOpenAddLibrary = openAddLibrary;
openAddLibrary = function() {
    setAddLibraryError('');
    const submitBtn = $('add-lib-submit');
    if (submitBtn) submitBtn.disabled = false;
    resetAddLibraryPaths();
    _origOpenAddLibrary();
};

// --- WebSocket for scan progress ---
// The scan progress socket while it connects or is open, or null
let scanSocket = null;
// The pending retry timer, or null when no retry is scheduled
let scanSocketRetry = null;
// Delay before the next retry in ms; doubles up to 30 s and returns to 1 s when a socket opens
let scanSocketDelay = 1000;
// True once the current outage has been logged, so the warning is printed only once
let scanSocketWarned = false;

// A library change re-renders the open detail, library or home page from fresh data. The lists are dropped from the
// 30 s cache first, so the re-render is not served from it. Other pages get no re-render.
function refreshViewAfterLibraryChange() {
    const page = currentPage;
    API.invalidateCache('/Items');
    API.invalidateCache('/Shows');
    if (page !== 'detail' && page !== 'library' && page !== 'home') return;
    setTimeout(() => {
        // Only if the user is still on that page
        if (currentPage !== page) return;
        if (page === 'detail' && currentDetailId) renderDetail(currentDetailId);
        else if (page === 'library' && currentLibraryId) {
            // An open list is brought up to date where it stands; only a library not shown yet is drawn again
            if (!libraryRefreshInPlace()) renderLibrary(currentLibraryRoute);
        }
        else if (page === 'home') renderHome();
    }, 500);
}

function initScanProgressWebSocket() {
    if (typeof WebSocket === 'undefined') return;
    if (!API.isLoggedIn() || scanSocket || scanSocketRetry) return;

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/socket?api_key=${encodeURIComponent(API.token)}`;

    let ws;
    try {
        ws = new WebSocket(wsUrl);
    } catch (e) {
        console.error('Opening the scan progress socket failed:', e);
        return;
    }
    scanSocket = ws;

    ws.onopen = () => {
        scanSocketDelay = 1000;
        scanSocketWarned = false;
        // The server then sends one frame per scan in progress, so the map is rebuilt from empty
        scanStatuses = {};
        updateScanRings();
    };

    ws.onmessage = (event) => {
        try {
            const data = JSON.parse(event.data);
            if (data.MessageType === 'ScheduledTaskStarted') {
                // Refresh tasks tab if visible
                if (currentPage === 'admin') refreshTasks();
            } else if (data.MessageType === 'ScheduledTaskCompleted') {
                const libId = data.ItemId;
                if (data.State === 'Failed') {
                    toast(t('admin.task_failed', { name: data.Name || t('admin.task'), error: data.Error || t('common.unknown_error') }), 'error');
                } else if (data.State === 'Completed' && libId) {
                    // Name the library; the event's Name is an English server label
                    const lib = libraries.find(l => (l.id || l.Id) === libId);
                    toast(lib ? t('library.scan_finished_for', { name: lib.name || lib.Name }) : t('library.scan_finished'));
                }
                // Refresh tasks tab if visible
                if (currentPage === 'admin') refreshTasks();
            } else if (data.MessageType === 'RefreshProgress') {
                // Scan progress: State "Queued" while it waits its turn, "Active" with each whole percent, "Idle" at "100" when done
                const id = data.Data?.ItemId;
                const p = Number(data.Data?.Progress ?? NaN);
                const state = data.Data?.State;
                if (id && Number.isFinite(p)) {
                    // Taken as is, not maxed: a new run of the same library can start lower than the last one
                    if (state === 'Idle' || p >= 100) delete scanStatuses[id];
                    else scanStatuses[id] = { percent: p, queued: state === 'Queued' };
                    updateScanRings(id);
                }
            } else if (data.MessageType === 'LibraryChanged') {
                refreshViewAfterLibraryChange();
            }
        } catch (e) {
            console.error('Handling a server event failed:', e);
        }
    };

    ws.onclose = () => {
        // A socket stopped on purpose is no longer the current one
        if (scanSocket !== ws) return;
        scanSocket = null;
        if (!API.isLoggedIn()) return;
        if (!scanSocketWarned) {
            console.warn('Scan progress socket closed, retrying with backoff');
            scanSocketWarned = true;
        }
        // Reconnect with exponential backoff
        scanSocketRetry = setTimeout(() => {
            scanSocketRetry = null;
            initScanProgressWebSocket();
        }, scanSocketDelay);
        scanSocketDelay = Math.min(scanSocketDelay * 2, 30000);
    };
}

// Called on logout and before a login starts a new socket: closes the socket and cancels any pending retry
function stopScanProgressWebSocket() {
    clearTimeout(scanSocketRetry);
    scanSocketRetry = null;
    scanSocketDelay = 1000;
    scanSocketWarned = false;
    const ws = scanSocket;
    scanSocket = null;
    if (ws) ws.close();
}

export {
    scanStatuses, loadSidebar, openAddLibrary, closeAddLibrary, deleteLibraryFromAdmin, toggleRealtimeMonitor,
    submitAddLibrary, openFolderBrowser, folderBrowserLoad, folderBrowserUp, folderBrowserSelect, closeFolderBrowser,
    addLibraryPathRow, removeLibraryPathRow, initScanProgressWebSocket, stopScanProgressWebSocket,
};
