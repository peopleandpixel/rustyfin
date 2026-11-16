// A card's played, favourite and like marks, and its context menu (the ⋮ button, or a right click or
// long press on the card), as Jellyfin Web's card overlay buttons and item context menu
import { icon, toast, escapeHtml } from './util.js';
import { applyCardUserData } from './cards.js';
import { playQueueAt } from './queue.js';
import { openPhotoViewer } from './photos.js';

const PLAYABLE = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video', 'AudioBook', 'Trailer'];
const CONTAINERS = ['Series', 'Season', 'MusicAlbum', 'MusicArtist', 'BoxSet', 'Playlist', 'Folder', 'CollectionFolder'];
// Jellyfin Web's itemHelper.supportsAddingToPlaylist and supportsAddingToCollection, for the types Rustyfin has
const PLAYLIST_TYPES = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video', 'Series', 'Season', 'MusicAlbum', 'MusicArtist', 'BoxSet'];
const COLLECTION_TYPES = ['Movie', 'Series', 'Season', 'Episode', 'MusicAlbum', 'MusicArtist', 'MusicVideo', 'Video'];
const NO_USER_DATA = ['Person', 'Genre', 'MusicGenre', 'Studio', 'CollectionFolder', 'UserView'];

// The item a context menu was opened for, by id (the menu's play and add commands need its type)
const menuItems = new Map();
// Counts menu opens: an item answer for an earlier open is dropped
let menuSeq = 0;
const currentRoute = () => globalThis.location?.hash ?? '';

// The menu's commands for an item, in Jellyfin Web's order: [{ action, label, icon, ...data }] with
// 'separator' entries between the groups. Actions other than the card-* ones are the router's.
// `shownIn` is the collection or playlist the card is shown in ({ id, type, canEdit, entryId }),
// whose editors get Remove from it.
function cardMenuCommands(item, isAdmin, shownIn = null) {
    const type = item.Type || '';
    const data = item.UserData || {};
    const container = !!item.IsFolder || CONTAINERS.includes(type);
    const name = item.Name || '';
    const commands = [];
    if (PLAYABLE.includes(type)) commands.push({ action: 'card-play', label: t('common.play'), icon: 'Play' });
    else if (container) commands.push({ action: 'play-all', label: t('common.play'), icon: 'Play' });
    if (container) commands.push({ action: 'shuffle', label: t('player.shuffle'), icon: 'Shuffle' });
    commands.push('separator');
    if (!NO_USER_DATA.includes(type)) {
        commands.push({
            action: 'card-played', label: data.Played ? t('detail.mark_unplayed') : t('detail.mark_played'), icon: 'Check',
            played: !!data.Played,
        });
        commands.push({
            action: 'card-favorite', label: data.IsFavorite ? t('cards.remove_favorite') : t('detail.add_favorite'), icon: 'Heart',
            favorite: !!data.IsFavorite,
        });
        commands.push({ action: 'card-like', label: t('cards.like'), icon: 'ThumbsUp', checked: data.Likes === true });
        commands.push({ action: 'card-dislike', label: t('cards.dislike'), icon: 'ThumbsDown', checked: data.Likes === false });
    }
    commands.push('separator');
    if (PLAYLIST_TYPES.includes(type)) commands.push({ action: 'card-add-playlist', label: t('cards.add_to_playlist'), icon: 'ListPlus' });
    if (isAdmin && COLLECTION_TYPES.includes(type)) commands.push({ action: 'card-add-collection', label: t('cards.add_to_collection'), icon: 'FolderPlus' });
    if (shownIn?.canEdit && shownIn.id !== item.Id) {
        const playlist = shownIn.type === 'Playlist';
        commands.push({
            action: 'card-remove-from', icon: 'Trash2',
            label: t(playlist ? 'collections.remove_from_playlist' : 'collections.remove_from_collection'),
            containerId: shownIn.id, containerType: shownIn.type, entryId: shownIn.entryId || undefined,
        });
    }
    commands.push('separator');
    if ((type === 'Episode' || type === 'Season') && item.SeriesId) {
        commands.push({ action: 'navigate', label: t('cards.go_to_series'), icon: 'Tv', page: 'detail', target: item.SeriesId });
    }
    if (type === 'Episode' && item.SeasonId) {
        commands.push({ action: 'navigate', label: t('cards.go_to_season'), icon: 'Folder', page: 'detail', target: item.SeasonId });
    }
    if (item.AlbumId && type !== 'MusicAlbum') {
        commands.push({ action: 'navigate', label: t('cards.go_to_album'), icon: 'Disc3', page: 'detail', target: item.AlbumId });
    }
    if (isAdmin) {
        commands.push('separator');
        commands.push({ action: 'refresh-item', label: t('detail.refresh_metadata'), icon: 'RefreshCw' });
        // The item page's delete: it asks first, and goes to the season, series or album afterwards
        const parent = item.SeasonId || item.SeriesId || item.AlbumId || '';
        commands.push({
            action: 'delete-item', label: t('detail.delete_item'), icon: 'Trash2',
            name, type, parentId: parent === item.Id ? '' : parent,
        });
    }
    // No separator first, last or twice in a row
    return commands.filter((c, i, all) => c !== 'separator'
        || (i > 0 && all[i - 1] !== 'separator' && all.slice(i + 1).some(n => n !== 'separator')));
}

function cardMenuHtml(item, commands) {
    const id = item.Id;
    return commands.map(c => {
        if (c === 'separator') return '<hr>';
        const data = [
            ['action', c.action], ['id', c.target || id], ['page', c.page], ['played', c.played], ['favorite', c.favorite],
            ['name', c.name], ['type', c.type], ['parent-id', c.parentId], ['container-id', c.containerId],
            ['container-type', c.containerType], ['entry-id', c.entryId],
        ].filter(([, v]) => v !== undefined).map(([k, v]) => ` data-${k}="${escapeHtml(String(v))}"`).join('');
        const role = c.checked === undefined ? 'menuitem' : 'menuitemcheckbox';
        const checked = c.checked === undefined ? '' : ` aria-checked="${c.checked}"`;
        return `<button type="button" role="${role}"${checked}${data}>${icon(c.icon, { size: 16, class: c.checked ? 'text-[#00a4dc]' : '' })}<span>${escapeHtml(c.label)}</span></button>`;
    }).join('');
}

function menuElement() {
    let menu = document.getElementById('card-menu');
    if (!menu) {
        menu = document.createElement('div');
        menu.id = 'card-menu';
        menu.className = 'jf-card-menu';
        menu.setAttribute('popover', 'auto');
        menu.setAttribute('role', 'menu');
        // A command closes the menu; the router's document listener runs it after
        menu.addEventListener('click', e => {
            if (e.target.closest('[data-action]')) menu.hidePopover();
        });
        menu.addEventListener('keydown', e => {
            if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return;
            e.preventDefault();
            const items = [...menu.querySelectorAll('[data-action]')];
            const at = items.indexOf(document.activeElement);
            items[(at + (e.key === 'ArrowDown' ? 1 : items.length - 1)) % items.length]?.focus();
        });
        document.body.appendChild(menu);
    }
    return menu;
}

// Opens the menu for the card's item at `point` ({ x, y }), else under the button. The item is
// fetched first, as Jellyfin Web's card menu does, so its marks and links are current.
async function openCardMenu(el, point = null) {
    const id = el.dataset.id || el.closest('.jf-card')?.dataset.id;
    if (!id) return;
    const anchor = el.getBoundingClientRect();
    const seq = ++menuSeq;
    const route = currentRoute();
    // Only the latest open, on the page it was made on, shows a menu
    const stale = () => seq !== menuSeq || currentRoute() !== route || el.isConnected === false;
    let item;
    try {
        item = await API.get(`/Items/${encodeURIComponent(id)}`);
    } catch (e) {
        if (!stale()) toast(e.message, 'error');
        return;
    }
    if (stale()) return;
    menuItems.clear();
    menuItems.set(item.Id, item);
    const menu = menuElement();
    menu.innerHTML = cardMenuHtml(item, cardMenuCommands(item, API.isAdmin, menuContainer(el)));
    if (menu.matches(':popover-open')) menu.hidePopover();
    menu.style.left = '0px';
    menu.style.top = '0px';
    menu.showPopover();
    const box = menu.getBoundingClientRect();
    const x = point ? point.x : anchor.right - box.width;
    const y = point ? point.y : anchor.bottom + 4;
    menu.style.left = `${Math.max(8, Math.min(x, innerWidth - box.width - 8))}px`;
    menu.style.top = `${Math.max(8, y + box.height > innerHeight - 8 ? y - box.height - (point ? 0 : anchor.height + 8) : y)}px`;
    menu.querySelector('[data-action]')?.focus();
}

// The collection or playlist a card is shown in, from its page's section (collections.js)
function menuContainer(el) {
    const holder = el.closest?.('[data-container-id]');
    if (!holder) return null;
    return {
        id: holder.dataset.containerId,
        type: holder.dataset.containerType,
        canEdit: holder.dataset.canEdit === 'true',
        entryId: el.closest('[data-entry-id]')?.dataset.entryId || '',
    };
}

// User data changed: every card of the item redraws its marks, the cached lists go, and open
// lists (the library grid) hear of it
function userDataChanged(id, data) {
    applyCardUserData(id, data);
    API.invalidateCache('/Items');
    API.invalidateCache('/Shows');
    document.dispatchEvent(new CustomEvent('jf-userdata', { detail: { id, data } }));
}

// A mark's state from the element: its own data-*, else its card's
const currentFlag = (el, key) => (el.dataset[key] ?? el.closest('.jf-card')?.dataset[key]) === 'true';

async function toggleCardPlayed(el) {
    const id = el.dataset.id;
    const current = currentFlag(el, 'played');
    try {
        const answer = current ? await API.markUnplayed(id) : await API.markPlayed(id);
        userDataChanged(id, { ...(answer && answer.Played !== undefined ? answer : {}), Played: !current });
        toast(current ? t('detail.marked_unplayed') : t('detail.marked_played'));
    } catch (e) {
        toast(e.message, 'error');
    }
}

async function toggleCardFavorite(el) {
    const id = el.dataset.id;
    const current = currentFlag(el, 'favorite');
    try {
        if (current) await API.unsetFavorite(id);
        else await API.setFavorite(id);
        userDataChanged(id, { IsFavorite: !current });
        toast(current ? t('detail.favorite_removed') : t('detail.favorite_added'));
    } catch (e) {
        toast(e.message, 'error');
    }
}

// Like or dislike; choosing the one already set clears it, as Jellyfin's like and dislike buttons do
async function setCardLikes(el, likes) {
    const id = el.dataset.id;
    const value = el.getAttribute('aria-checked') === 'true' ? null : likes;
    try {
        await API.setRating(id, value);
        userDataChanged(id, { Likes: value });
        toast(value === null ? t('cards.rating_cleared') : value ? t('cards.liked') : t('cards.disliked'));
    } catch (e) {
        toast(e.message, 'error');
    }
}

// Play from the menu: the item alone, through the queue (resume or start over, as any Play asks)
function playCardItem(el) {
    const item = menuItems.get(el.dataset.id);
    if (item?.Type === 'Photo') openPhotoViewer([item], 0);
    else if (item) playQueueAt([item], 0);
}

// Add to playlist or collection: a dialog with the existing ones and a new one by name
function addToDialog(kind) {
    let dialog = document.getElementById('card-add-dialog');
    if (!dialog) {
        dialog = document.createElement('dialog');
        dialog.id = 'card-add-dialog';
        dialog.className = 'jf-filter-menu';
        document.body.appendChild(dialog);
    }
    dialog.dataset.kind = kind;
    return dialog;
}

async function openAddTo(el, kind) {
    const id = el.dataset.id;
    let lists = [];
    try {
        const answer = kind === 'playlist' ? await API.getPlaylists() : await API.getCollections();
        lists = Array.isArray(answer) ? answer : answer.Items || [];
        // Only the playlists the user may change: their own and those shared with them to edit
        if (kind === 'playlist') lists = lists.filter(l => l.CanEdit !== false);
    } catch (e) {
        toast(e.message, 'error');
        return;
    }
    const dialog = addToDialog(kind);
    const title = kind === 'playlist' ? t('cards.add_to_playlist') : t('cards.add_to_collection');
    dialog.innerHTML = `
        <form method="dialog" class="flex flex-col gap-3">
            <h2 class="text-base font-bold">${escapeHtml(title)}</h2>
            <select name="target" class="jf-input">
                <option value="">${escapeHtml(kind === 'playlist' ? t('cards.new_playlist') : t('cards.new_collection'))}</option>
                ${lists.map(l => `<option value="${escapeHtml(l.Id)}">${escapeHtml(l.Name || '')}</option>`).join('')}
            </select>
            <input name="name" class="jf-input" placeholder="${escapeHtml(t('cards.name'))}" aria-label="${escapeHtml(t('cards.name'))}">
            <div class="flex justify-end gap-2">
                <button type="submit" value="cancel" class="jf-btn-ghost !py-1.5 !px-3 text-xs">${escapeHtml(t('common.cancel'))}</button>
                <button type="submit" value="ok" class="jf-btn-primary !py-1.5 !px-3 text-xs">${escapeHtml(t('cards.add'))}</button>
            </div>
        </form>`;
    const select = dialog.querySelector('select');
    const name = dialog.querySelector('input');
    select.addEventListener('change', () => { name.hidden = !!select.value; });
    dialog.onclose = () => {
        if (dialog.returnValue === 'ok') addTo(kind, id, select.value, name.value.trim());
    };
    // A reused dialog keeps the last returnValue; Escape must not read as the last OK
    dialog.returnValue = '';
    dialog.showModal();
    (lists.length ? select : name).focus();
}

async function addTo(kind, id, target, name) {
    try {
        let listName = name;
        if (target) {
            await (kind === 'playlist' ? API.addToPlaylist(target, [id]) : API.addToCollection(target, [id]));
            const option = document.querySelector(`#card-add-dialog option[value="${CSS.escape(target)}"]`);
            listName = option?.textContent || '';
        } else {
            if (!name) {
                toast(t('cards.name_required'), 'error');
                return;
            }
            await (kind === 'playlist' ? API.createPlaylist(name, [id]) : API.createCollection(name, [id]));
        }
        API.invalidateCache('/Items');
        toast(t('cards.added_to', { name: listName }));
    } catch (e) {
        toast(e.message, 'error');
    }
}

// A right click on a card opens its menu at the pointer. So does a long press on a touch screen:
// iOS Safari fires no contextmenu for touch, so a timer does it (LONG_PRESS_MS without moving), and
// the click that ends the press doesn't open the item.
const LONG_PRESS_MS = 500;
const LONG_PRESS_SLOP = 10;
let longPress = null;
let longPressAt = 0;

function cancelLongPress() {
    if (longPress) clearTimeout(longPress.timer);
    longPress = null;
}

function cardPointerDown(e) {
    cancelLongPress();
    if (e.pointerType !== 'touch') return;
    const card = e.target.closest?.('.jf-card[data-id]');
    if (!card || card.closest('#card-menu')) return;
    const press = { x: e.clientX, y: e.clientY };
    press.timer = setTimeout(() => {
        longPress = null;
        longPressAt = Date.now();
        openCardMenu(card, press);
    }, LONG_PRESS_MS);
    longPress = press;
}

function cardPointerMove(e) {
    if (longPress && Math.hypot(e.clientX - longPress.x, e.clientY - longPress.y) > LONG_PRESS_SLOP) cancelLongPress();
}

// The click (and Android's contextmenu) right after a long press opened the menu are its own
const justLongPressed = () => Date.now() - longPressAt < 800;

function cardContextMenu(e) {
    const card = e.target.closest?.('.jf-card[data-id]');
    if (!card || card.closest('#card-menu')) return;
    e.preventDefault();
    if (justLongPressed()) return;
    cancelLongPress();
    openCardMenu(card, { x: e.clientX, y: e.clientY });
}

function cardClickAfterLongPress(e) {
    if (!justLongPressed() || !e.target.closest?.('.jf-card[data-id]')) return;
    e.preventDefault();
    e.stopPropagation();
}

document.addEventListener('pointerdown', cardPointerDown, { passive: true });
document.addEventListener('pointermove', cardPointerMove, { passive: true });
for (const type of ['pointerup', 'pointercancel', 'scroll']) document.addEventListener(type, cancelLongPress, { passive: true, capture: true });
document.addEventListener('contextmenu', cardContextMenu);
document.addEventListener('click', cardClickAfterLongPress, true);

export {
    cardMenuCommands, cardMenuHtml, openCardMenu, toggleCardPlayed, toggleCardFavorite, setCardLikes, playCardItem,
    openAddTo, addTo, userDataChanged, cardPointerDown, cardPointerMove, cardContextMenu, cardClickAfterLongPress,
    cancelLongPress, LONG_PRESS_MS,
};
