// Photo libraries: their folders browsed in place (library/<id>/folder/<path>), the full-screen
// photo viewer (zoom, pan, swipe, the EXIF panel) and the slideshow. Rustyfin keeps a photo
// library's files without folder items, so its folders come from the items' paths.
import { $, icon, toast, escapeHtml } from './util.js';
import { playItem } from './player.js';

// The folder tree of one library, built once per visit of the library
let photoTree = null; // { libId, session, promise }
// The selection mode of the open folder: the ids picked for a slideshow
let photoSelection = null;

// The item types a photo library lists: photos, and home videos that play in the player
const PHOTO_LIBRARY_TYPES = 'Photo,Video';
const PHOTO_PAGE = 500;

// The folder that holds `path`, relative to the library path it is under ('' at the top); null
// when it is under none of `roots`
function relativeFolder(path, roots) {
    const file = String(path || '').replace(/\\/g, '/');
    for (const root of roots) {
        const base = String(root).replace(/\\/g, '/').replace(/\/+$/, '');
        if (base && file.startsWith(base + '/')) {
            const rest = file.slice(base.length + 1);
            const cut = rest.lastIndexOf('/');
            return cut < 0 ? '' : rest.slice(0, cut);
        }
    }
    return null;
}

// The library paths: an administrator's library lists them; otherwise the deepest folder that
// holds every item (one folder holding everything then shows as the top)
function libraryRoots(lib, items) {
    const paths = lib?.paths || lib?.Paths || lib?.Locations || [];
    if (paths.length) return paths;
    const dirs = items.map(i => String(i.Path || '').replace(/\\/g, '/').split('/').slice(0, -1));
    if (!dirs.length) return [];
    let common = dirs[0];
    for (const dir of dirs) {
        let n = 0;
        while (n < common.length && n < dir.length && common[n] === dir[n]) n++;
        common = common.slice(0, n);
    }
    return [common.join('/')];
}

// Jellyfin's SortName order, with numbers compared as numbers (its AlphanumericComparator)
const byName = (a, b) => a.localeCompare(b, undefined, { numeric: true, sensitivity: 'base' });
const itemSortName = item => item.SortName || item.Name || '';

// Each folder's subfolders and items: Map(folder path -> { folders: [name], items: [item] }),
// each list in Jellyfin's IsFolder,SortName order (folders first, then by sort name)
function photoFolderTree(items, roots) {
    const tree = new Map();
    const node = path => {
        if (!tree.has(path)) tree.set(path, { folders: new Set(), items: [] });
        return tree.get(path);
    };
    node('');
    for (const item of items) {
        const folder = relativeFolder(item.Path, roots);
        if (folder == null) continue;
        node(folder).items.push(item);
        const parts = folder ? folder.split('/') : [];
        for (let i = 0; i < parts.length; i++) {
            node(parts.slice(0, i).join('/')).folders.add(parts[i]);
        }
    }
    for (const entry of tree.values()) {
        entry.folders = [...entry.folders].sort(byName);
        entry.items.sort((a, b) => byName(itemSortName(a), itemSortName(b)));
    }
    return tree;
}

// The photos of a folder and every folder under it, in the tree's order
function photosUnder(tree, folder) {
    const entry = tree.get(folder);
    if (!entry) return [];
    const own = entry.items.filter(i => i.Type === 'Photo' || i.MediaType === 'Photo');
    return own.concat(...entry.folders.map(name => photosUnder(tree, folder ? `${folder}/${name}` : name)));
}

async function fetchLibraryItems(libId) {
    const items = [];
    for (let start = 0; ; start += PHOTO_PAGE) {
        const q = new URLSearchParams({
            ParentId: libId, Recursive: 'true', IncludeItemTypes: PHOTO_LIBRARY_TYPES,
            Fields: 'Path,PrimaryImageAspectRatio,SortName', SortBy: 'SortName', StartIndex: String(start), Limit: String(PHOTO_PAGE),
        });
        const page = await API.get(`/Items?${q}`);
        const batch = page.Items || [];
        items.push(...batch);
        if (batch.length < PHOTO_PAGE || items.length >= (page.TotalRecordCount ?? 0)) return items;
    }
}

function loadPhotoTree(lib, libId) {
    if (photoTree?.libId === libId && photoTree.session === API.token) return photoTree.promise;
    const promise = fetchLibraryItems(libId).then(items => {
        const roots = libraryRoots(lib, items);
        return { tree: photoFolderTree(items, roots) };
    });
    photoTree = { libId, session: API.token, promise };
    promise.catch(() => { if (photoTree?.promise === promise) photoTree = null; });
    return promise;
}

// A folder's route: library/<id>/folder/<each part encoded>
function folderRoute(libId, folder) {
    return folder ? `${libId}/folder/${folder.split('/').map(encodeURIComponent).join('/')}` : libId;
}

function folderFromRoute(parts) {
    return parts.map(p => { try { return decodeURIComponent(p); } catch { return p; } }).join('/');
}

function breadcrumbsHtml(libId, libName, folder) {
    const parts = folder ? folder.split('/') : [];
    const crumbs = [{ name: libName, route: folderRoute(libId, '') }]
        .concat(parts.map((name, i) => ({ name, route: folderRoute(libId, parts.slice(0, i + 1).join('/')) })));
    return crumbs.map((c, i) => i === crumbs.length - 1
        ? `<span class="text-white font-extrabold" aria-current="page">${escapeHtml(c.name)}</span>`
        : `<button type="button" class="text-white/60 hover:text-white" data-action="navigate" data-page="library" data-id="${escapeHtml(c.route)}">${escapeHtml(c.name)}</button><span class="text-white/30">/</span>`).join('');
}

function folderCardHtml(libId, folder, name, cover) {
    const img = cover ? `<img alt="" loading="lazy" decoding="async" class="absolute inset-0 w-full h-full object-cover opacity-60" src="${escapeHtml(API.imageUrl(cover.Id, 'Primary', { maxHeight: 360, tag: cover.ImageTags?.Primary }))}">` : '';
    const path = folder ? `${folder}/${name}` : name;
    return `
    <div class="jf-card group rf-photo-card" data-action="navigate" data-page="library" data-id="${escapeHtml(folderRoute(libId, path))}" data-type="Folder" tabindex="0" role="link">
        <div class="jf-poster-backdrop relative bg-white/5">${img}
            <div class="absolute inset-0 flex items-center justify-center text-white/80">${icon('Folder', { size: 40 })}</div>
        </div>
        <div class="p-3"><p class="text-sm font-semibold truncate text-white" title="${escapeHtml(name)}">${escapeHtml(name)}</p></div>
    </div>`;
}

function photoCardHtml(item, index) {
    const isVideo = item.Type === 'Video';
    const tag = item.ImageTags?.Primary;
    const img = tag ? `<img alt="${escapeHtml(item.Name || '')}" loading="lazy" decoding="async" class="absolute inset-0 w-full h-full object-cover" src="${escapeHtml(API.imageUrl(item.Id, 'Primary', { maxHeight: 360, tag }))}">` : '';
    const selected = photoSelection?.has(item.Id);
    return `
    <div class="jf-card group rf-photo-card${selected ? ' ring-2 ring-[#00a4dc]' : ''}" data-action="photo-open" data-id="${escapeHtml(item.Id)}" data-type="${escapeHtml(item.Type)}" data-index="${index}" tabindex="0" role="button" aria-pressed="${photoSelection ? String(!!selected) : 'false'}">
        <div class="jf-poster-backdrop relative bg-white/5">${img}
            ${!img ? `<div class="absolute inset-0 flex items-center justify-center text-white/20">${icon(isVideo ? 'Film' : 'Image', { size: 40 })}</div>` : ''}
            ${isVideo ? `<div class="absolute inset-0 flex items-center justify-center"><span class="jf-play-btn-circle">${icon('Play', { size: 24, class: 'text-white ml-1 fill-current' })}</span></div>` : ''}
            ${photoSelection ? `<span class="absolute top-2 left-2 w-6 h-6 rounded-full border-2 border-white flex items-center justify-center ${selected ? 'bg-[#00a4dc]' : 'bg-black/40'}">${selected ? icon('Check', { size: 14 }) : ''}</span>` : ''}
        </div>
        <div class="p-3"><p class="text-sm font-semibold truncate text-white" title="${escapeHtml(item.Name || '')}">${escapeHtml(item.Name || '')}</p></div>
    </div>`;
}

// What the open folder shows, for its click handlers
let openFolder = null; // { libId, folder, entry, tree }

// Draws a photo library's folder: its subfolders, then its photos and videos
async function renderPhotoLibrary(lib, libId, folderParts = []) {
    const session = API.token;
    const page = $('page-library');
    const libName = lib?.name || lib?.Name || t('common.library');
    const folder = folderFromRoute(folderParts);
    if (openFolder?.libId !== libId || openFolder?.folder !== folder) photoSelection = null;
    page.innerHTML = `<div class="jf-page"><div class="flex justify-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div></div>`;
    let tree;
    try {
        ({ tree } = await loadPhotoTree(lib, libId));
    } catch (err) {
        if (API.token !== session) return;
        page.innerHTML = `<div class="p-10 text-center text-red-400 jf-page">${escapeHtml(err.message)}</div>`;
        return;
    }
    if (API.token !== session || !location.hash.includes(libId)) return;
    const entry = tree.get(folder) || { folders: [], items: [] };
    openFolder = { libId, folder, entry, tree };
    const hasPhotos = photosUnder(tree, folder).length > 0;
    const folderCards = entry.folders.map(name => {
        const sub = folder ? `${folder}/${name}` : name;
        return folderCardHtml(libId, folder, name, photosUnder(tree, sub)[0]);
    }).join('');
    const itemCards = entry.items.map((item, i) => photoCardHtml(item, i)).join('');
    const btn = (action, iconName, label, extra = '') => `<button type="button" class="jf-btn-secondary !py-2 inline-flex items-center gap-2" data-action="${action}" ${extra}>${icon(iconName, { size: 16 })}<span>${label}</span></button>`;
    page.innerHTML = `
    <div class="jf-page">
        <div class="flex flex-col md:flex-row md:items-center justify-between gap-4 mb-6 pb-4 border-b border-white/10">
            <div>
                <nav class="flex flex-wrap items-center gap-2 text-xl lg:text-2xl" aria-label="${escapeHtml(t('photos.breadcrumbs'))}">
                    <span class="inline-flex">${icon('Images', { size: 26 })}</span>${breadcrumbsHtml(libId, libName, folder)}
                </nav>
                <p class="text-xs text-white/40 mt-1">${t('photos.counts', { folders: entry.folders.length, items: entry.items.length })}</p>
            </div>
            ${hasPhotos ? `<div class="flex flex-wrap gap-2">
                ${btn('photo-slideshow', 'Play', t('photos.slideshow'))}
                ${btn('photo-slideshow', 'Shuffle', t('photos.shuffle'), 'data-shuffle="true"')}
                ${btn('photo-select', 'Check', photoSelection ? t('photos.select_done', { n: photoSelection.size }) : t('photos.select'), `aria-pressed="${!!photoSelection}"`)}
            </div>` : ''}
        </div>
        ${folderCards || itemCards ? `<div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-6 gap-4" id="photo-grid">${folderCards}${itemCards}</div>`
            : `<p class="text-white/50 text-center py-16">${t('photos.empty')}</p>`}
    </div>`;
}

// A photo card: picks it in selection mode, plays a video, else opens the viewer at it
function openPhotoCard(el) {
    if (!openFolder) return;
    const item = openFolder.entry.items[Number(el.dataset.index)];
    if (!item) return;
    if (photoSelection) {
        if (photoSelection.has(item.Id)) photoSelection.delete(item.Id);
        else photoSelection.add(item.Id);
        el.outerHTML = photoCardHtml(item, Number(el.dataset.index));
        const button = document.querySelector('[data-action="photo-select"] span');
        if (button) button.textContent = t('photos.select_done', { n: photoSelection.size });
        return;
    }
    if (item.Type === 'Video') {
        playItem(item.Id, 'Videos', item.Name || '');
        return;
    }
    const photos = openFolder.entry.items.filter(i => i.Type === 'Photo');
    openPhotoViewer(photos, Math.max(0, photos.indexOf(item)));
}

function togglePhotoSelection() {
    photoSelection = photoSelection ? null : new Set();
    if (openFolder) navigateRedraw();
}

function navigateRedraw() {
    const lib = (window.libraries || []).find(l => (l.id || l.Id) === openFolder.libId);
    const parts = openFolder.folder ? openFolder.folder.split('/') : [];
    renderPhotoLibrary(lib, openFolder.libId, parts.map(encodeURIComponent));
}

// The slideshow of the picked photos, else of every photo in and under the open folder
function startFolderSlideshow(el) {
    if (!openFolder) return;
    const all = photosUnder(openFolder.tree, openFolder.folder);
    const picked = photoSelection?.size ? all.filter(i => photoSelection.has(i.Id)) : all;
    if (!picked.length) {
        toast(t('photos.empty'), 'error');
        return;
    }
    openPhotoViewer(picked, 0, { play: true, shuffle: el.dataset.shuffle === 'true' });
}

// A photo shown outside a photo library (search, home rows, a detail page): the viewer, with the
// other photo cards beside it on the page
async function openPhotoItem(id, el = null) {
    const cards = el?.closest('.grid, .jf-row, [class*="overflow-x"]')?.querySelectorAll('[data-type="Photo"][data-id]');
    const ids = cards?.length ? [...new Set([...cards].map(c => c.dataset.id))] : [id];
    try {
        const items = await Promise.all(ids.map(i => API.getItem(i)));
        openPhotoViewer(items, Math.max(0, ids.indexOf(id)));
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- The viewer ---

const SLIDESHOW_INTERVALS = [3, 5, 10, 20, 30];
const INTERVAL_KEY = 'rustyfin.slideshowInterval';
const MAX_ZOOM = 6;

let viewer = null; // { items, order, pos, playing, shuffle, timer, zoom, x, y, info, el }

function slideshowSeconds() {
    let saved = null;
    try { saved = Number(localStorage.getItem(INTERVAL_KEY)); } catch {}
    return SLIDESHOW_INTERVALS.includes(saved) ? saved : 5;
}

// The viewing order: the list's, or a random one (Fisher-Yates) that starts at `first`
function viewingOrder(count, shuffle, first = 0, random = Math.random) {
    const order = [...Array(count).keys()];
    if (!shuffle) return order;
    for (let i = count - 1; i > 0; i--) {
        const j = Math.floor(random() * (i + 1));
        [order[i], order[j]] = [order[j], order[i]];
    }
    const at = order.indexOf(first);
    if (at > 0) [order[0], order[at]] = [order[at], order[0]];
    return order;
}

function viewerImageUrl(item) {
    const width = Math.round(Math.min(window.screen?.width || 1920, 3840) * (window.devicePixelRatio || 1));
    return API.imageUrl(item.Id, 'Primary', { maxWidth: Math.min(width, 7680), quality: 90, tag: item.ImageTags?.Primary });
}

function openPhotoViewer(items, index = 0, { play = false, shuffle = false } = {}) {
    if (!items?.length) return;
    closePhotoViewer();
    const el = document.createElement('div');
    el.id = 'photo-viewer';
    el.className = 'fixed inset-0 z-[200] bg-black text-white select-none';
    el.setAttribute('role', 'dialog');
    el.setAttribute('aria-modal', 'true');
    el.setAttribute('aria-label', t('photos.viewer'));
    el.tabIndex = -1;
    const ctl = (action, iconName, label, extra = '') => `<button type="button" class="p-2 rounded-full bg-black/40 hover:bg-white/20" data-action="${action}" aria-label="${escapeHtml(label)}" title="${escapeHtml(label)}" ${extra}>${icon(iconName, { size: 22 })}</button>`;
    el.innerHTML = `
        <div class="absolute inset-0 overflow-hidden touch-none" data-photo-stage>
            <img class="absolute inset-0 m-auto max-w-full max-h-full object-contain origin-center will-change-transform" alt="" draggable="false" data-photo-img>
        </div>
        <div class="absolute top-0 inset-x-0 flex items-center gap-2 p-3 bg-gradient-to-b from-black/70 to-transparent" data-photo-bar>
            ${ctl('photo-close', 'X', t('common.close'))}
            <p class="flex-1 min-w-0 truncate text-sm font-semibold" data-photo-title></p>
            <span class="text-xs text-white/60" data-photo-count></span>
            ${ctl('photo-play', 'Play', t('photos.play'), 'data-photo-play')}
            ${ctl('photo-shuffle', 'Shuffle', t('photos.shuffle'), 'aria-pressed="false" data-photo-shuffle')}
            <label class="text-xs text-white/70 flex items-center gap-1">${t('photos.interval')}
                <select class="bg-black/60 border border-white/20 rounded px-1 py-0.5" data-photo-interval aria-label="${escapeHtml(t('photos.interval'))}">
                    ${SLIDESHOW_INTERVALS.map(s => `<option value="${s}">${t('photos.seconds', { n: s })}</option>`).join('')}
                </select>
            </label>
            ${ctl('photo-info', 'Info', t('photos.info'), 'aria-pressed="false" data-photo-info-btn')}
        </div>
        ${ctl('photo-prev', 'ChevronLeft', t('photos.previous'), 'style="position:absolute;left:12px;top:50%;transform:translateY(-50%)"')}
        ${ctl('photo-next', 'ChevronRight', t('photos.next'), 'style="position:absolute;right:12px;top:50%;transform:translateY(-50%)"')}
        <aside class="absolute top-16 right-3 bottom-3 w-72 max-w-[85vw] overflow-y-auto rounded-xl bg-black/80 border border-white/10 p-4 text-sm hidden" data-photo-info aria-live="polite"></aside>`;
    document.body.appendChild(el);
    viewer = {
        items, order: viewingOrder(items.length, shuffle, index), pos: 0, playing: false, shuffle, timer: 0,
        zoom: 1, x: 0, y: 0, info: false, el, pointers: new Map(), pinch: null, swipe: null, details: new Map(),
        returnFocus: document.activeElement,
    };
    if (!shuffle) viewer.pos = index;
    el.querySelector('[data-photo-interval]').value = String(slideshowSeconds());
    el.querySelector('[data-photo-interval]').addEventListener('change', e => {
        try { localStorage.setItem(INTERVAL_KEY, e.target.value); } catch {}
        if (viewer?.playing) scheduleNext();
    });
    bindViewerGestures(el);
    document.addEventListener('keydown', viewerKeys, true);
    setShuffle(shuffle, { keepOrder: true });
    showPhoto();
    if (play) setPlaying(true);
    el.focus();
}

function closePhotoViewer() {
    if (!viewer) return;
    clearTimeout(viewer.timer);
    document.removeEventListener('keydown', viewerKeys, true);
    if (document.fullscreenElement === viewer.el) document.exitFullscreen?.().catch(() => {});
    viewer.el.remove();
    const back = viewer.returnFocus;
    viewer = null;
    back?.focus?.();
}

function currentPhoto() {
    return viewer.items[viewer.order[viewer.pos]];
}

function showPhoto() {
    const item = currentPhoto();
    const img = viewer.el.querySelector('[data-photo-img]');
    resetZoom();
    img.src = viewerImageUrl(item);
    img.alt = item.Name || '';
    viewer.el.querySelector('[data-photo-title]').textContent = item.Name || '';
    viewer.el.querySelector('[data-photo-count]').textContent = `${viewer.pos + 1} / ${viewer.items.length}`;
    // The next photo loads meanwhile, so the slideshow doesn't wait for it
    const next = viewer.items[viewer.order[(viewer.pos + 1) % viewer.items.length]];
    if (next && next !== item) new Image().src = viewerImageUrl(next);
    if (viewer.info) renderInfo();
    if (viewer.playing) scheduleNext();
}

function stepPhoto(delta) {
    if (!viewer) return;
    const n = viewer.items.length;
    viewer.pos = (viewer.pos + delta + n) % n;
    showPhoto();
}

function scheduleNext() {
    clearTimeout(viewer.timer);
    viewer.timer = setTimeout(() => stepPhoto(1), slideshowSeconds() * 1000);
}

function setPlaying(on) {
    viewer.playing = on;
    clearTimeout(viewer.timer);
    if (on) scheduleNext();
    const button = viewer.el.querySelector('[data-photo-play]');
    const label = on ? t('photos.pause') : t('photos.play');
    button.innerHTML = icon(on ? 'Pause' : 'Play', { size: 22 });
    button.setAttribute('aria-label', label);
    button.title = label;
}

// Shuffle on: a random order from the photo shown; off: the list's order, at the same photo
function setShuffle(on, { keepOrder = false } = {}) {
    viewer.shuffle = on;
    if (!keepOrder) {
        const at = viewer.order[viewer.pos];
        viewer.order = viewingOrder(viewer.items.length, on, at);
        viewer.pos = on ? 0 : at;
        viewer.el.querySelector('[data-photo-count]').textContent = `${viewer.pos + 1} / ${viewer.items.length}`;
    }
    viewer.el.querySelector('[data-photo-shuffle]').setAttribute('aria-pressed', String(on));
    viewer.el.querySelector('[data-photo-shuffle]').classList.toggle('!bg-[#00a4dc]', on);
}

// --- Zoom and pan ---

function applyTransform() {
    const img = viewer.el.querySelector('[data-photo-img]');
    img.style.transform = `translate(${viewer.x}px, ${viewer.y}px) scale(${viewer.zoom})`;
    img.style.cursor = viewer.zoom > 1 ? 'grab' : '';
}

function resetZoom() {
    viewer.zoom = 1;
    viewer.x = 0;
    viewer.y = 0;
    applyTransform();
}

// Zooms to `zoom`, keeping the point (cx, cy) of the stage (from its centre) where it is
function zoomTo(zoom, cx = 0, cy = 0) {
    const next = Math.min(MAX_ZOOM, Math.max(1, zoom));
    const k = next / viewer.zoom;
    viewer.x = cx - (cx - viewer.x) * k;
    viewer.y = cy - (cy - viewer.y) * k;
    viewer.zoom = next;
    if (next === 1) { viewer.x = 0; viewer.y = 0; }
    if (viewer.playing && next > 1) setPlaying(false);
    applyTransform();
}

function stagePoint(stage, e) {
    const r = stage.getBoundingClientRect();
    return [e.clientX - r.left - r.width / 2, e.clientY - r.top - r.height / 2];
}

function bindViewerGestures(el) {
    const stage = el.querySelector('[data-photo-stage]');
    stage.addEventListener('wheel', e => {
        e.preventDefault();
        zoomTo(viewer.zoom * (e.deltaY < 0 ? 1.2 : 1 / 1.2), ...stagePoint(stage, e));
    }, { passive: false });
    stage.addEventListener('dblclick', e => zoomTo(viewer.zoom > 1 ? 1 : 2.5, ...stagePoint(stage, e)));
    stage.addEventListener('pointerdown', e => {
        stage.setPointerCapture?.(e.pointerId);
        viewer.pointers.set(e.pointerId, [e.clientX, e.clientY]);
        if (viewer.pointers.size === 2) {
            const [a, b] = [...viewer.pointers.values()];
            viewer.pinch = { dist: Math.hypot(a[0] - b[0], a[1] - b[1]), zoom: viewer.zoom };
            viewer.swipe = null;
        } else {
            viewer.swipe = { x: e.clientX, y: e.clientY, panX: viewer.x, panY: viewer.y, t: Date.now() };
        }
    });
    stage.addEventListener('pointermove', e => {
        if (!viewer.pointers.has(e.pointerId)) return;
        viewer.pointers.set(e.pointerId, [e.clientX, e.clientY]);
        if (viewer.pinch && viewer.pointers.size === 2) {
            const [a, b] = [...viewer.pointers.values()];
            const r = stage.getBoundingClientRect();
            zoomTo(viewer.pinch.zoom * Math.hypot(a[0] - b[0], a[1] - b[1]) / viewer.pinch.dist,
                (a[0] + b[0]) / 2 - r.left - r.width / 2, (a[1] + b[1]) / 2 - r.top - r.height / 2);
        } else if (viewer.swipe && viewer.zoom > 1) {
            viewer.x = viewer.swipe.panX + e.clientX - viewer.swipe.x;
            viewer.y = viewer.swipe.panY + e.clientY - viewer.swipe.y;
            applyTransform();
        }
    });
    const end = e => {
        viewer?.pointers.delete(e.pointerId);
        if (!viewer) return;
        if (viewer.pointers.size < 2) viewer.pinch = null;
        const swipe = viewer.swipe;
        viewer.swipe = null;
        if (!swipe || viewer.zoom > 1 || e.type === 'pointercancel') return;
        const dx = e.clientX - swipe.x;
        const dy = e.clientY - swipe.y;
        if (Math.abs(dx) > 50 && Math.abs(dx) > Math.abs(dy) * 1.5) stepPhoto(dx < 0 ? 1 : -1);
    };
    stage.addEventListener('pointerup', end);
    stage.addEventListener('pointercancel', end);
}

// --- The EXIF panel ---

// The panel's rows from a photo's item: what the scan read from its EXIF block
function photoInfoRows(item) {
    const rows = [];
    const add = (key, value) => { if (value != null && value !== '') rows.push([t(`photos.exif_${key}`), String(value)]); };
    if (item.PremiereDate) add('taken', new Date(item.PremiereDate).toLocaleString());
    if (item.Width && item.Height) add('size', `${item.Width} × ${item.Height}`);
    add('camera', [item.CameraMake, item.CameraModel].filter(Boolean).join(' '));
    add('software', item.Software);
    if (item.ExposureTime) add('exposure', item.ExposureTime >= 1 ? `${item.ExposureTime} s` : `1/${Math.round(1 / item.ExposureTime)} s`);
    if (item.Aperture) add('aperture', `ƒ/${Number(item.Aperture).toFixed(1)}`);
    if (item.FocalLength) add('focal', `${Number(item.FocalLength)} mm`);
    add('iso', item.IsoSpeedRating);
    add('orientation', item.ImageOrientation);
    if (item.Latitude != null && item.Longitude != null) {
        add('location', `${Number(item.Latitude).toFixed(5)}, ${Number(item.Longitude).toFixed(5)}`);
    }
    if (item.Altitude != null) add('altitude', `${Math.round(item.Altitude)} m`);
    add('path', item.Path);
    return rows;
}

async function renderInfo() {
    const panel = viewer.el.querySelector('[data-photo-info]');
    const item = currentPhoto();
    let details = viewer.details.get(item.Id);
    if (!details) {
        panel.innerHTML = `<p class="text-white/50">${t('common.loading')}</p>`;
        try {
            details = await API.getItem(item.Id);
        } catch {
            details = item;
        }
        if (!viewer) return;
        viewer.details.set(item.Id, details);
        if (currentPhoto() !== item) return;
    }
    const rows = photoInfoRows(details);
    panel.innerHTML = `<h2 class="font-bold mb-3">${escapeHtml(details.Name || '')}</h2>` + (rows.length
        ? `<dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5">${rows.map(([k, v]) => `<dt class="text-white/50">${escapeHtml(k)}</dt><dd class="break-words">${escapeHtml(v)}</dd>`).join('')}</dl>`
        : `<p class="text-white/50">${t('photos.exif_none')}</p>`);
}

function toggleInfo() {
    viewer.info = !viewer.info;
    const panel = viewer.el.querySelector('[data-photo-info]');
    panel.classList.toggle('hidden', !viewer.info);
    viewer.el.querySelector('[data-photo-info-btn]').setAttribute('aria-pressed', String(viewer.info));
    if (viewer.info) renderInfo();
}

function viewerKeys(e) {
    if (!viewer || e.target.closest?.('select')) return;
    const keys = {
        ArrowRight: () => stepPhoto(1),
        ArrowLeft: () => stepPhoto(-1),
        Escape: () => closePhotoViewer(),
        ' ': () => setPlaying(!viewer.playing),
        i: () => toggleInfo(),
        '+': () => zoomTo(viewer.zoom * 1.5),
        '=': () => zoomTo(viewer.zoom * 1.5),
        '-': () => zoomTo(viewer.zoom / 1.5),
        '0': () => resetZoom(),
    };
    const run = keys[e.key];
    if (!run || (e.key === ' ' && e.target.closest?.('button'))) return;
    e.preventDefault();
    e.stopPropagation();
    run();
}

const photoActions = {
    'photo-open': el => openPhotoCard(el),
    'photo-select': () => togglePhotoSelection(),
    'photo-slideshow': el => startFolderSlideshow(el),
    'photo-close': () => closePhotoViewer(),
    'photo-prev': () => stepPhoto(-1),
    'photo-next': () => stepPhoto(1),
    'photo-play': () => setPlaying(!viewer.playing),
    'photo-shuffle': () => setShuffle(!viewer.shuffle),
    'photo-info': () => toggleInfo(),
};

export {
    renderPhotoLibrary, openPhotoViewer, closePhotoViewer, openPhotoItem, photoActions, photoFolderTree, photosUnder,
    relativeFolder, libraryRoots, viewingOrder, photoInfoRows, folderRoute,
};
