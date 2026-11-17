// Multi-select in the library grid, as Jellyfin Web's multiSelect: a long press on a touch screen or a card's
// checkbox starts it, a click then picks cards, and a bar acts on all of them (add to a playlist or collection,
// mark played or unplayed, favourite, and for administrators refresh metadata and delete). The grid opts in with
// `data-multiselect`. Keys: x picks the focused card, Space or Enter too while selecting, Ctrl+A picks every card
// shown, Escape ends it.
import { icon, toast, escapeHtml } from './util.js';
import { openAddToIds } from './cardmenu.js';
import { userDataChanged } from './userdata.js';
import { openRefreshDialog } from './metadata-admin.js';

// The picked items by id: { id, name, type }
const selected = new Map();
// The grid being selected in, while anything is picked
let scope = null;
let observer = null;

const cardOf = el => el?.closest?.('[data-multiselect] .jf-card[data-id]') || null;
const cardsIn = root => [...root.querySelectorAll('.jf-card[data-id]')];

function cardInfo(card) {
    const name = card.querySelector('[title]')?.getAttribute('title') || card.querySelector('img')?.alt || '';
    return { id: card.dataset.id, name, type: card.dataset.type || '' };
}

// The card's checkbox, added the first time the card is hovered, focused or shown while selecting
function ensureCheckbox(card) {
    let box = card.querySelector(':scope .jf-card-select');
    if (!box) {
        box = document.createElement('button');
        box.type = 'button';
        box.className = 'jf-card-select';
        box.setAttribute('role', 'checkbox');
        box.setAttribute('aria-label', t('multiselect.select'));
        box.innerHTML = icon('Check', { size: 16 });
        const holder = card.querySelector('.jf-poster, .jf-poster-backdrop, .jf-poster-banner, .jf-list-thumb') || card;
        holder.appendChild(box);
    }
    return box;
}

// Draws a card picked or not
function markCard(card) {
    const on = selected.has(card.dataset.id);
    card.classList.toggle('is-selected', on);
    if (scope || on) ensureCheckbox(card).setAttribute('aria-checked', String(on));
    else card.querySelector('.jf-card-select')?.setAttribute('aria-checked', 'false');
}

function startSelecting(root) {
    if (scope === root) return;
    if (scope) endSelecting();
    scope = root;
    root.classList.add('is-selecting');
    cardsIn(root).forEach(markCard);
    // The virtual grid draws cards as it scrolls: each new one shows its checkbox and its state
    observer = new MutationObserver(records => {
        for (const r of records) {
            for (const node of r.addedNodes) {
                if (node.nodeType !== 1) continue;
                const cards = node.matches('.jf-card[data-id]') ? [node] : cardsIn(node);
                cards.forEach(markCard);
            }
        }
    });
    observer.observe(root, { childList: true, subtree: true });
    drawBar();
}

function endSelecting() {
    selected.clear();
    observer?.disconnect();
    observer = null;
    if (scope) {
        scope.classList.remove('is-selecting');
        cardsIn(scope).forEach(markCard);
    }
    scope = null;
    const bar = document.getElementById('multiselect-bar');
    if (bar) bar.hidden = true;
}

// Picks or drops a card; the last one dropped ends the selection, as in Jellyfin Web
function toggleCard(card, on = !selected.has(card.dataset.id)) {
    const root = card.closest('[data-multiselect]');
    if (!root) return;
    if (on) {
        startSelecting(root);
        selected.set(card.dataset.id, cardInfo(card));
    } else {
        selected.delete(card.dataset.id);
    }
    for (const same of cardsIn(root).filter(c => c.dataset.id === card.dataset.id)) markCard(same);
    if (selected.size === 0) endSelecting();
    else drawBar();
}

function selectAll() {
    if (!scope) return;
    for (const card of cardsIn(scope)) {
        selected.set(card.dataset.id, cardInfo(card));
        markCard(card);
    }
    drawBar();
}

// The bar's commands: [action, label key, icon, administrators only]
const BAR_COMMANDS = [
    ['add-playlist', 'cards.add_to_playlist', 'ListPlus', false],
    ['add-collection', 'cards.add_to_collection', 'FolderPlus', true],
    ['played', 'detail.mark_played', 'Check', false],
    ['unplayed', 'detail.mark_unplayed', 'CircleDashed', false],
    ['favorite', 'detail.add_favorite', 'Heart', false],
    ['unfavorite', 'cards.remove_favorite', 'HeartOff', false],
    ['refresh', 'detail.refresh_metadata', 'RefreshCw', true],
    ['delete', 'detail.delete_item', 'Trash2', true],
];

function barCommands(isAdmin) {
    return BAR_COMMANDS.filter(([, , , admin]) => isAdmin || !admin);
}

function barHtml(count, isAdmin) {
    const button = (action, label, iconName, extra = '') => `<button type="button" class="jf-multiselect-btn${extra}" data-multiselect-action="${action}" title="${escapeHtml(label)}" aria-label="${escapeHtml(label)}">${icon(iconName, { size: 18 })}</button>`;
    return `
        ${button('close', t('multiselect.cancel'), 'X')}
        <span class="text-sm font-semibold mr-auto" aria-live="polite">${escapeHtml(t('multiselect.count', { count }))}</span>
        ${button('select-all', t('multiselect.select_all'), 'SquareCheckBig')}
        ${barCommands(isAdmin).map(([action, key, iconName]) => button(action, t(key), iconName, action === 'delete' ? ' text-red-400' : '')).join('')}`;
}

function drawBar() {
    let bar = document.getElementById('multiselect-bar');
    if (!bar) {
        bar = document.createElement('div');
        bar.id = 'multiselect-bar';
        bar.className = 'jf-multiselect-bar';
        bar.setAttribute('role', 'toolbar');
        bar.addEventListener('click', e => {
            const action = e.target.closest('[data-multiselect-action]')?.dataset.multiselectAction;
            if (action) runBarAction(action);
        });
        // Left and right move along the bar, as in a toolbar
        bar.addEventListener('keydown', e => {
            if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
            e.preventDefault();
            const buttons = [...bar.querySelectorAll('button')];
            const at = buttons.indexOf(document.activeElement);
            buttons[(at + (e.key === 'ArrowRight' ? 1 : buttons.length - 1)) % buttons.length]?.focus();
        });
        document.body.appendChild(bar);
    }
    bar.setAttribute('aria-label', t('multiselect.toolbar'));
    bar.innerHTML = barHtml(selected.size, API.isAdmin);
    bar.hidden = false;
}

// Runs `call(id)` for every picked item; returns the ids it worked for and toasts the failures
async function eachSelected(ids, call) {
    const results = await Promise.allSettled(ids.map(id => call(id)));
    const done = ids.filter((_, i) => results[i].status === 'fulfilled');
    const failed = results.find(r => r.status === 'rejected');
    if (failed) toast(t('multiselect.some_failed', { count: ids.length - done.length, message: failed.reason?.message || '' }), 'error');
    return done;
}

const USER_DATA_ACTIONS = {
    played: { call: id => API.markPlayed(id), data: { Played: true }, done: 'multiselect.marked_played' },
    unplayed: { call: id => API.markUnplayed(id), data: { Played: false }, done: 'multiselect.marked_unplayed' },
    favorite: { call: id => API.setFavorite(id), data: { IsFavorite: true }, done: 'multiselect.favorited' },
    unfavorite: { call: id => API.unsetFavorite(id), data: { IsFavorite: false }, done: 'multiselect.unfavorited' },
};

async function runBarAction(action) {
    const items = [...selected.values()];
    const ids = items.map(i => i.id);
    if (action === 'close') { endSelecting(); return; }
    if (action === 'select-all') { selectAll(); return; }
    if (ids.length === 0) return;
    const userData = USER_DATA_ACTIONS[action];
    if (userData) {
        const done = await eachSelected(ids, userData.call);
        for (const id of done) userDataChanged(id, userData.data);
        if (done.length) toast(t(userData.done, { count: done.length }));
        endSelecting();
        return;
    }
    if (action === 'add-playlist' || action === 'add-collection') {
        openAddToIds(ids, action === 'add-playlist' ? 'playlist' : 'collection', endSelecting);
        return;
    }
    if (action === 'refresh') {
        openRefreshDialog(async options => {
            endSelecting();
            toast(t('multiselect.refresh_started', { count: ids.length }));
            const done = await eachSelected(ids, id => API.refreshItem(id, options));
            API.invalidateCache('/Items');
            API.invalidateCache('/Shows');
            if (done.length) toast(t('multiselect.refreshed', { count: done.length }));
        });
        return;
    }
    if (action === 'delete') await deleteSelected(items);
}

// Administrators: removes the picked items after one confirmation, then draws the page again
async function deleteSelected(items) {
    if (!confirm(t('multiselect.delete_confirm', { count: items.length }))) return;
    const done = await eachSelected(items.map(i => i.id), id => API.deleteItem(id));
    API.invalidateCache('/Items');
    API.invalidateCache('/Shows');
    endSelecting();
    if (done.length) toast(t('multiselect.deleted', { count: done.length }));
    // The grid drops the removed items when its page is drawn again
    window.dispatchEvent(new HashChangeEvent('hashchange'));
}

// A click on a checkbox picks its card; while selecting, a click anywhere on a card does, not opening it
function multiSelectClick(e) {
    // The click that ends a long press is the press's own (cardmenu.js drops it)
    if (e.defaultPrevented) return;
    const box = e.target.closest?.('.jf-card-select');
    const card = cardOf(box || e.target);
    if (!card || (!box && !scope)) return;
    if (!box && card.closest('[data-multiselect]') !== scope) return;
    e.preventDefault();
    e.stopPropagation();
    toggleCard(card);
}

function multiSelectKey(e) {
    if (e.key === 'Escape' && scope) {
        if (document.querySelector('dialog[open]')) return;
        e.preventDefault();
        endSelecting();
        return;
    }
    if (e.target.closest?.('input, textarea, select, [contenteditable="true"]')) return;
    if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'a' && scope) {
        e.preventDefault();
        selectAll();
        return;
    }
    const card = e.target.matches?.('.jf-card[data-id]') ? cardOf(e.target) : null;
    if (!card || e.ctrlKey || e.metaKey || e.altKey) return;
    const picking = e.key === 'x' || (scope && (e.key === ' ' || e.key === 'Enter'));
    if (!picking) return;
    e.preventDefault();
    e.stopPropagation();
    toggleCard(card);
}

// The first hover or focus of a card adds its checkbox, so a mouse or a keyboard can start a selection
function revealCheckbox(e) {
    const card = cardOf(e.target);
    if (card) ensureCheckbox(card).setAttribute('aria-checked', String(selected.has(card.dataset.id)));
}

// A long press (cardmenu.js) on a card in a grid with multi-select picks it instead of opening its menu
function multiSelectLongPress(e) {
    const card = e.detail?.card;
    if (!card?.closest?.('[data-multiselect]')) return;
    e.preventDefault();
    toggleCard(card, true);
}

document.addEventListener('click', multiSelectClick, true);
document.addEventListener('keydown', multiSelectKey, true);
document.addEventListener('pointerover', revealCheckbox, { passive: true });
document.addEventListener('focusin', revealCheckbox);
document.addEventListener('jf-card-longpress', multiSelectLongPress);
// Another page, or the grid drawn again, ends the selection
window.addEventListener('hashchange', () => { if (scope) endSelecting(); });

function selectedIds() {
    return [...selected.keys()];
}

export {
    toggleCard, selectAll, endSelecting, selectedIds, barCommands, barHtml, runBarAction, multiSelectClick, multiSelectKey,
    multiSelectLongPress, ensureCheckbox,
};
