// Collections and playlists (Jellyfin's BoxSets and Playlists): their lists (the home tiles, a movie
// library's Collections tab), a collection's or playlist's own items on its page, making one,
// removing an item from one and reordering a playlist by dragging
import { $, icon, toast, escapeHtml } from './util.js';
import { createCardElement, itemListRow } from './cards.js';
import { playQueueAt } from './queue.js';
import { navigate } from './router.js';

const KINDS = {
    collections: { type: 'BoxSet', title: 'collections.collections', create: 'collections.new_collection', empty: 'collections.no_collections', icon: 'Library' },
    playlists: { type: 'Playlist', title: 'collections.playlists', create: 'collections.new_playlist', empty: 'collections.no_playlists', icon: 'ListMusic' },
};
const LIST_LIMIT = 300;
const GRID_CLASS = 'grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4';

// Drops a draw for a page that was left meanwhile
let drawGen = 0;
// The collection or playlist whose page is open: { item, items, canEdit }
let openContainer = null;

// The request for a kind's list: every collection or playlist the user sees, or under a library
// those holding its items (the server's library-scoped listing), by name
function containerListUrl(kind, libId = '') {
    const q = new URLSearchParams({
        IncludeItemTypes: KINDS[kind].type, Recursive: 'true', SortBy: 'SortName', SortOrder: 'Ascending',
        Fields: 'ChildCount,PrimaryImageAspectRatio', Limit: String(LIST_LIMIT),
    });
    if (libId) q.set('ParentId', libId);
    return `/Items?${q}`;
}

// Only administrators make collections, as Jellyfin's default CollectionManagement; anyone makes playlists
const canCreate = kind => kind === 'playlists' || API.isAdmin;

function newButtonHtml(kind) {
    if (!canCreate(kind)) return '';
    return `
    <button type="button" class="jf-btn-ghost !py-2 !px-4 text-sm" data-action="new-container" data-kind="${kind}">
        ${icon('Plus', { size: 16 })} ${escapeHtml(t(KINDS[kind].create))}
    </button>`;
}

// Draws a kind's cards into `holder`
async function drawContainerGrid(holder, kind, libId, gen) {
    const session = API.token;
    try {
        const data = await API.get(containerListUrl(kind, libId));
        if (gen !== drawGen || API.token !== session || !holder.isConnected) return;
        const items = data.Items || [];
        if (!items.length) {
            holder.outerHTML = `<div class="text-center py-20 text-white/40">${escapeHtml(t(KINDS[kind].empty))}</div>`;
            return;
        }
        const frag = document.createDocumentFragment();
        for (const it of items) {
            const card = createCardElement(it);
            if (card) frag.appendChild(card);
        }
        holder.replaceChildren(frag);
    } catch (e) {
        if (gen !== drawGen || !holder.isConnected) return;
        holder.outerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

// The Collections or Playlists page (library/collections, library/playlists), as Jellyfin's
// views of those names
function renderContainerPage(kind) {
    const gen = ++drawGen;
    const page = $('page-library');
    page.classList.remove('jf-with-rail');
    page.innerHTML = `
    <div class="jf-page">
        <div class="flex flex-wrap items-center justify-between gap-4 mb-6 pb-4 border-b border-white/10">
            <div class="flex items-center gap-2">
                <span class="inline-flex">${icon(KINDS[kind].icon, { size: 28 })}</span>
                <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(t(KINDS[kind].title))}</h1>
            </div>
            ${newButtonHtml(kind)}
        </div>
        <div id="container-grid" class="${GRID_CLASS}"></div>
    </div>`;
    return drawContainerGrid($('container-grid'), kind, '', gen);
}

// A movie library's Collections tab: the collections holding its movies, under the tab bar
function renderCollectionsTab(libId, libName, tabBar) {
    const gen = ++drawGen;
    const page = $('page-library');
    page.classList.remove('jf-with-rail');
    page.innerHTML = `
    <div class="jf-page">
        <div class="flex flex-wrap items-center justify-between gap-4 mb-4 pb-4 border-b border-white/10">
            <div class="flex items-center gap-2">
                <span class="inline-flex">${icon('Film', { size: 28 })}</span>
                <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
            </div>
            ${newButtonHtml('collections')}
        </div>
        ${tabBar}
        <div id="container-grid" class="${GRID_CLASS}"></div>
    </div>`;
    return drawContainerGrid($('container-grid'), 'collections', libId, gen);
}

// The home page's Collections and Playlists tiles, after the libraries, as Jellyfin's views. The
// Collections one shows once a collection exists (revealContainerTiles), as Jellyfin makes that
// view with the first collection.
function containerTilesHtml() {
    return Object.entries(KINDS).map(([kind, k]) => `
    <div class="jf-card p-5 bg-gradient-to-br from-sky-600/30 to-slate-500/10 border-sky-500/30 flex flex-col justify-between min-h-[120px]${kind === 'collections' ? ' hidden' : ''}" data-container-tile="${kind}" data-action="navigate" data-page="library" data-id="${kind}">
        <span class="inline-flex">${icon(k.icon, { size: 28 })}</span>
        <p class="font-bold text-white text-base truncate">${escapeHtml(t(k.title))}</p>
    </div>`).join('');
}

async function revealContainerTiles() {
    try {
        const tile = document.querySelector('[data-container-tile="collections"]');
        if (!tile) return;
        const data = await API.cachedGet('/Items?IncludeItemTypes=BoxSet&Recursive=true&Limit=0');
        if (data?.TotalRecordCount > 0) tile.classList.remove('hidden');
    } catch { /* no tile */ }
}

// --- A collection's or playlist's page ---

// The section under a collection's or playlist's details: a collection's items as cards, a
// playlist's as rows in its order that its editors drag to reorder; Play, Shuffle and the card
// menu's Remove come from the page and the menu
function containerSectionHtml(item) {
    if (item.Type !== 'BoxSet' && item.Type !== 'Playlist') return '';
    return `<div id="container-items" class="mb-10" data-container-id="${escapeHtml(item.Id)}" data-container-type="${escapeHtml(item.Type)}"></div>`;
}

// The request for a container's items in their saved order: the Jellyfin endpoint for a playlist
// (its entries carry PlaylistItemId), /Items with ParentId for a collection
function containerItemsUrl(item) {
    if (item.Type === 'Playlist') return `/Playlists/${encodeURIComponent(item.Id)}/Items?Fields=PrimaryImageAspectRatio`;
    return `/Items?ParentId=${encodeURIComponent(item.Id)}&Fields=PrimaryImageAspectRatio&Limit=${LIST_LIMIT}`;
}

async function renderContainerItems(item) {
    const holder = $('container-items');
    if (!holder) return;
    const session = API.token;
    try {
        const [data, playlist] = await Promise.all([
            API.get(containerItemsUrl(item), { cache: 'no-store' }),
            item.Type === 'Playlist' ? API.getPlaylist(item.Id).catch(() => null) : null,
        ]);
        if (API.token !== session || !holder.isConnected) return;
        const items = data.Items || [];
        const canEdit = item.Type === 'Playlist' ? !!playlist?.CanEdit : API.isAdmin;
        openContainer = { item, items, canEdit };
        holder.dataset.canEdit = String(canEdit);
        drawContainerItems(holder);
    } catch (e) {
        if (API.token !== session || !holder.isConnected) return;
        holder.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

function drawContainerItems(holder) {
    const { item, items, canEdit } = openContainer;
    const heading = `<h2 class="text-xl font-bold mb-4 text-white">${escapeHtml(t('collections.items_count', { count: items.length }))}</h2>`;
    if (!items.length) {
        holder.innerHTML = `${heading}<p class="text-sm text-white/40">${escapeHtml(t(item.Type === 'Playlist' ? 'collections.playlist_empty' : 'collections.collection_empty'))}</p>`;
        return;
    }
    if (item.Type === 'BoxSet') {
        holder.innerHTML = `${heading}<div class="${GRID_CLASS}"></div>`;
        const grid = holder.lastElementChild;
        for (const it of items) {
            const card = createCardElement(it);
            if (card) grid.appendChild(card);
        }
        return;
    }
    holder.innerHTML = `${heading}<ol class="flex flex-col gap-1" aria-label="${escapeHtml(item.Name || '')}">${items.map((it, index) => playlistEntryHtml(it, index, canEdit)).join('')}</ol>`;
    if (canEdit) wireDragReorder(holder.querySelector('ol'));
}

function playlistEntryHtml(it, index, canEdit) {
    const name = it.Name || '';
    return `
    <li class="jf-playlist-entry flex items-center gap-2" data-entry-id="${escapeHtml(it.PlaylistItemId || '')}" data-index="${index}"${canEdit ? ' draggable="true"' : ''}>
        ${canEdit ? `<span class="jf-drag-handle text-white/30 cursor-grab" title="${escapeHtml(t('collections.drag_to_reorder'))}" aria-hidden="true">${icon('GripVertical', { size: 16 })}</span>` : ''}
        <button type="button" class="jf-icon-btn flex-shrink-0" data-action="playlist-play-from" data-index="${index}" title="${escapeHtml(t('common.play'))}" aria-label="${escapeHtml(`${t('common.play')}: ${name}`)}">
            ${icon('Play', { size: 16, class: 'fill-current' })}
        </button>
        <div class="min-w-0 flex-1">${itemListRow(it)}</div>
        ${canEdit ? `
        <button type="button" class="jf-icon-btn flex-shrink-0 hover:!text-rose-400" data-action="card-remove-from" data-id="${escapeHtml(it.Id)}" data-entry-id="${escapeHtml(it.PlaylistItemId || '')}" title="${escapeHtml(t('collections.remove_from_playlist'))}" aria-label="${escapeHtml(`${t('collections.remove_from_playlist')}: ${name}`)}">
            ${icon('Trash2', { size: 16 })}
        </button>` : ''}
    </li>`;
}

// Plays the open playlist (or collection) through the queue from one of its entries, as a click on
// a playlist entry's play button does in Jellyfin Web
function playContainerFrom(el) {
    if (!openContainer) return;
    const playable = openContainer.items.filter(i => !i.IsFolder);
    const at = playable.indexOf(openContainer.items[Number(el.dataset.index)]);
    playQueueAt(playable, Math.max(0, at));
}

// The index an entry lands at when dropped on `target`: its place, or after it in its lower half
function dropIndex(list, target, clientY) {
    const rows = [...list.children];
    const box = target.getBoundingClientRect();
    const index = rows.indexOf(target);
    return clientY > box.top + box.height / 2 ? index + 1 : index;
}

// Native drag and drop on the playlist's rows: the row moves in the list at once, and Jellyfin's
// Move call saves the new place; a failed save draws the saved order again
function wireDragReorder(list) {
    let dragged = null;
    list.addEventListener('dragstart', e => {
        dragged = e.target.closest('li[data-entry-id]');
        if (!dragged) return;
        e.dataTransfer.effectAllowed = 'move';
        e.dataTransfer.setData('text/plain', dragged.dataset.entryId);
        dragged.classList.add('opacity-50');
    });
    list.addEventListener('dragend', () => {
        dragged?.classList.remove('opacity-50');
        dragged = null;
    });
    list.addEventListener('dragover', e => {
        if (dragged) e.preventDefault();
    });
    list.addEventListener('drop', e => {
        const target = e.target.closest('li[data-entry-id]');
        if (!dragged || !target) return;
        e.preventDefault();
        const from = [...list.children].indexOf(dragged);
        let to = dropIndex(list, target, e.clientY);
        if (to > from) to -= 1;
        if (to === from) return;
        movePlaylistEntry(from, to);
    });
}

// Moves the open playlist's entry from one index to another, on the page and on the server
async function movePlaylistEntry(from, to) {
    if (!openContainer) return;
    const { item, items } = openContainer;
    const [entry] = items.splice(from, 1);
    items.splice(to, 0, entry);
    const holder = $('container-items');
    if (holder) drawContainerItems(holder);
    try {
        await API.movePlaylistItem(item.Id, entry.PlaylistItemId, to);
        API.invalidateCache('/Items');
    } catch (e) {
        toast(e.message, 'error');
        renderContainerItems(item);
    }
}

// Removes an item from a collection or playlist: the card menu's Remove, or a playlist entry's
// button. The container is the menu's (data-container-id) or the open page's.
async function removeFromContainer(el) {
    const holder = el.dataset.containerId ? null : el.closest('[data-container-id]');
    const containerId = el.dataset.containerId || holder?.dataset.containerId || openContainer?.item.Id;
    const containerType = el.dataset.containerType || holder?.dataset.containerType || openContainer?.item.Type;
    const { id, entryId } = el.dataset;
    if (!containerId || !id) return;
    try {
        if (containerType === 'Playlist') await API.removeFromPlaylist(containerId, [entryId]);
        else await API.removeFromCollection(containerId, [id]);
        API.invalidateCache('/Items');
        toast(t(containerType === 'Playlist' ? 'collections.removed_from_playlist' : 'collections.removed_from_collection'));
    } catch (e) {
        toast(e.message, 'error');
        return;
    }
    if (openContainer?.item.Id === containerId) {
        openContainer.items = openContainer.items.filter(i => (containerType === 'Playlist' ? i.PlaylistItemId !== entryId : i.Id !== id));
        const section = $('container-items');
        if (section) drawContainerItems(section);
    }
}

// A dialog for a new collection or playlist: its name and, for a playlist, whether every user may
// see it (Jellyfin's public playlist). It opens the new one's page.
function openNewContainer(el) {
    const kind = el.dataset.kind === 'collections' ? 'collections' : 'playlists';
    let dialog = document.getElementById('new-container-dialog');
    if (!dialog) {
        dialog = document.createElement('dialog');
        dialog.id = 'new-container-dialog';
        dialog.className = 'jf-filter-menu';
        document.body.appendChild(dialog);
    }
    dialog.innerHTML = `
        <form method="dialog" class="flex flex-col gap-3">
            <h2 class="text-base font-bold">${escapeHtml(t(KINDS[kind].create))}</h2>
            <input name="name" class="jf-input" required placeholder="${escapeHtml(t('cards.name'))}" aria-label="${escapeHtml(t('cards.name'))}">
            ${kind === 'playlists' ? `<label class="flex items-center gap-2 text-sm"><input type="checkbox" name="public"> ${escapeHtml(t('collections.public_playlist'))}</label>` : ''}
            <div class="flex justify-end gap-2">
                <button type="submit" value="cancel" formnovalidate class="jf-btn-ghost !py-1.5 !px-3 text-xs">${escapeHtml(t('common.cancel'))}</button>
                <button type="submit" value="ok" class="jf-btn-primary !py-1.5 !px-3 text-xs">${escapeHtml(t('collections.create'))}</button>
            </div>
        </form>`;
    const form = dialog.querySelector('form');
    dialog.onclose = () => {
        if (dialog.returnValue !== 'ok') return;
        const name = form.elements.name.value.trim();
        createContainer(kind, name, !!form.elements.public?.checked);
    };
    dialog.returnValue = '';
    dialog.showModal();
    form.elements.name.focus();
}

async function createContainer(kind, name, isPublic) {
    if (!name) {
        toast(t('cards.name_required'), 'error');
        return;
    }
    try {
        const made = kind === 'collections'
            ? await API.post('/Collections', { Name: name, Ids: [] })
            : await API.post('/Playlists', { Name: name, Ids: [], UserId: API.userId, IsPublic: isPublic });
        API.invalidateCache('/Items');
        navigate('detail', made.Id);
    } catch (e) {
        toast(e.message, 'error');
    }
}

export {
    KINDS, containerListUrl, renderContainerPage, renderCollectionsTab, containerTilesHtml, revealContainerTiles,
    containerSectionHtml, containerItemsUrl, renderContainerItems, playlistEntryHtml, playContainerFrom, dropIndex,
    movePlaylistEntry, removeFromContainer, openNewContainer, createContainer,
};
