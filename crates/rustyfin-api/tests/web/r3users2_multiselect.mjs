// R3 users 2: multi-select in the library grid, as Jellyfin Web's multiSelect: a card's checkbox, a long press
// or the x key picks cards, a click picks while selecting, and the bar adds to a playlist or collection, marks
// played or unplayed, favourites, and for administrators refreshes and deletes. Runs modules/multiselect.js in a
// vm sandbox with a small fake DOM. Usage: node r3users2_multiselect.mjs (run by api_tests.rs r3users2_multiselect).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = source => source.replace(/^import [^;]*;$/gms, '').replace(/^export \{[^}]*\};$/gms, '');
const plain = value => JSON.parse(JSON.stringify(value));

function classList() {
    const set = new Set();
    return {
        add: c => set.add(c), remove: c => set.delete(c), contains: c => set.has(c),
        toggle: (c, on) => { if (on ?? !set.has(c)) set.add(c); else set.delete(c); },
    };
}

function fakeNode(tag = 'div') {
    const attrs = {};
    const node = {
        tagName: tag, nodeType: 1, hidden: false, innerHTML: '', className: '', children: [], classList: classList(), dataset: {},
        setAttribute: (k, v) => { attrs[k] = String(v); }, getAttribute: k => attrs[k] ?? null,
        appendChild: child => { node.children.push(child); child.parent = node; return child; },
        addEventListener() {}, querySelectorAll: () => [], querySelector: () => null, closest: () => null, matches: () => false,
        attrs,
    };
    return node;
}

// A grid (data-multiselect) of cards `ids`, and a card outside any such grid
function grid(ids) {
    const root = fakeNode();
    root.dataset.multiselect = '';
    const cards = ids.map(id => {
        const card = fakeNode();
        Object.assign(card.dataset, { id, type: 'Movie' });
        const poster = fakeNode();
        const title = { getAttribute: () => `Film ${id}` };
        card.querySelector = sel => (sel.includes('jf-card-select') ? poster.children[0] || null
            : sel === '[title]' ? title : sel.includes('jf-poster') ? poster : null);
        card.closest = sel => (sel === '[data-multiselect]' ? root : sel.includes('.jf-card[data-id]') ? card : null);
        card.matches = sel => sel.includes('.jf-card[data-id]');
        card.poster = poster;
        return card;
    });
    root.querySelectorAll = () => cards;
    return { root, cards };
}

function sandbox({ isAdmin = false, confirmAnswer = true } = {}) {
    const log = [];
    const keys = new Set();
    const listeners = {};
    const body = fakeNode('body');
    const context = vm.createContext({
        t: (key, opts) => { keys.add(key); return opts ? `${key}|${JSON.stringify(opts)}` : key; },
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'),
        toast: (msg, kind) => log.push(['toast', msg, kind || 'ok']),
        confirm: text => { log.push(['confirm', text]); return confirmAnswer; },
        userDataChanged: (id, data) => log.push(['userdata', id, data]),
        openAddToIds: (ids, kind, done) => log.push(['add-to', ids, kind, typeof done]),
        openRefreshDialog: refresh => log.push(['refresh-dialog', refresh]),
        HashChangeEvent: class { constructor(type) { this.type = type; } },
        MutationObserver: class { observe() {} disconnect() {} },
        window: { addEventListener() {}, dispatchEvent: e => log.push(['dispatch', e.type]) },
        document: {
            body,
            createElement: fakeNode,
            getElementById: id => body.children.find(c => c.id === id) || null,
            querySelector: () => null,
            addEventListener: (type, fn) => { listeners[type] = fn; },
        },
        API: {
            isAdmin,
            markPlayed: async id => log.push(['played', id]),
            markUnplayed: async id => log.push(['unplayed', id]),
            setFavorite: async id => log.push(['fav', id]),
            unsetFavorite: async id => { if (id === 'b') throw new Error('nope'); log.push(['unfav', id]); },
            deleteItem: async id => log.push(['delete', id]),
            refreshItem: async (id, options) => log.push(['refresh', id, options]),
            invalidateCache: () => {},
        },
    });
    vm.runInContext(`${strip(read('modules/multiselect.js'))}
        this.fns = { toggleCard, selectAll, endSelecting, selectedIds, barCommands, runBarAction, multiSelectClick,
            multiSelectKey, multiSelectLongPress, ensureCheckbox };`, context);
    const bar = () => body.children.find(c => c.id === 'multiselect-bar');
    return { fns: context.fns, log, keys, listeners, bar };
}

const event = (target, props = {}) => {
    const e = {
        target, defaultPrevented: false, stopped: false, ctrlKey: false, metaKey: false, altKey: false, ...props,
        preventDefault() { e.defaultPrevented = true; }, stopPropagation() { e.stopped = true; },
    };
    return e;
};

test('administrators get every bulk command; others not refresh, delete or add to collection', () => {
    const { fns } = sandbox();
    const actions = isAdmin => plain(fns.barCommands(isAdmin).map(c => c[0]));
    assert.deepEqual(actions(true), ['add-playlist', 'add-collection', 'played', 'unplayed', 'favorite', 'unfavorite', 'refresh', 'delete']);
    assert.deepEqual(actions(false), ['add-playlist', 'played', 'unplayed', 'favorite', 'unfavorite']);
});

test('picking a card shows its checkbox checked and the bar; dropping the last ends the selection', () => {
    const box = sandbox();
    const { root, cards } = grid(['a', 'b']);
    box.fns.toggleCard(cards[0]);
    assert.deepEqual([...box.fns.selectedIds()], ['a']);
    assert.ok(root.classList.contains('is-selecting'));
    assert.ok(cards[0].classList.contains('is-selected'));
    assert.equal(cards[0].poster.children[0].getAttribute('aria-checked'), 'true');
    assert.equal(cards[0].poster.children[0].getAttribute('role'), 'checkbox');
    assert.equal(cards[1].poster.children[0].getAttribute('aria-checked'), 'false', 'every card shows its checkbox while selecting');
    assert.equal(box.bar().hidden, false);
    assert.equal(box.bar().getAttribute('role'), 'toolbar');
    assert.match(box.bar().innerHTML, /multiselect\.count\|\{&quot;count&quot;:1\}/);
    box.fns.toggleCard(cards[0]);
    assert.equal(box.fns.selectedIds().length, 0);
    assert.ok(!root.classList.contains('is-selecting'));
    assert.equal(box.bar().hidden, true);
});

test('while selecting, a click on a card picks it instead of opening it; a long-press click is ignored', () => {
    const box = sandbox();
    const { cards } = grid(['a', 'b']);
    const idle = event(cards[0]);
    box.fns.multiSelectClick(idle);
    assert.equal(idle.stopped, false, 'not selecting: the click opens the item');
    box.fns.toggleCard(cards[0]);
    const click = event(cards[1]);
    box.fns.multiSelectClick(click);
    assert.ok(click.stopped && click.defaultPrevented);
    assert.deepEqual([...box.fns.selectedIds()], ['a', 'b']);
    const afterPress = event(cards[1], { defaultPrevented: true });
    box.fns.multiSelectClick(afterPress);
    assert.deepEqual([...box.fns.selectedIds()], ['a', 'b'], 'the click ending a long press is the press’s own');
});

test('a long press in the grid picks the card instead of opening its menu', () => {
    const box = sandbox();
    const { cards } = grid(['a']);
    const press = event(null, { detail: { card: cards[0] } });
    box.fns.multiSelectLongPress(press);
    assert.ok(press.defaultPrevented, 'the card menu stays closed');
    assert.deepEqual([...box.fns.selectedIds()], ['a']);
    const outside = { closest: () => null };
    const other = event(null, { detail: { card: outside } });
    box.fns.multiSelectLongPress(other);
    assert.equal(other.defaultPrevented, false, 'outside a multi-select grid the menu opens');
    // cardmenu.js asks first, with a cancelable event
    assert.match(read('modules/cardmenu.js'), /new CustomEvent\('jf-card-longpress', \{ cancelable: true/);
    assert.match(read('modules/cardmenu.js'), /if \(!claim\.defaultPrevented\) openCardMenu/);
});

test('keys: x picks the focused card, Space toggles while selecting, Ctrl+A picks all shown, Escape ends', () => {
    const box = sandbox();
    const { cards } = grid(['a', 'b', 'c']);
    const x = event(cards[0], { key: 'x' });
    box.fns.multiSelectKey(x);
    assert.ok(x.defaultPrevented);
    assert.deepEqual([...box.fns.selectedIds()], ['a']);
    box.fns.multiSelectKey(event(cards[1], { key: ' ' }));
    assert.deepEqual([...box.fns.selectedIds()], ['a', 'b']);
    box.fns.multiSelectKey(event(cards[2], { key: 'a', ctrlKey: true }));
    assert.deepEqual([...box.fns.selectedIds()], ['a', 'b', 'c']);
    const esc = event(cards[0], { key: 'Escape' });
    box.fns.multiSelectKey(esc);
    assert.equal(box.fns.selectedIds().length, 0);
    const space = event(cards[0], { key: ' ' });
    box.fns.multiSelectKey(space);
    assert.equal(space.defaultPrevented, false, 'not selecting: Space opens the card as before');
});

test('Mark played acts on every picked item, tells the cards, and ends the selection', async () => {
    const box = sandbox();
    const { cards } = grid(['a', 'b']);
    box.fns.toggleCard(cards[0]);
    box.fns.toggleCard(cards[1]);
    await box.fns.runBarAction('played');
    assert.deepEqual(box.log.filter(e => e[0] === 'played').map(e => e[1]), ['a', 'b']);
    assert.deepEqual(box.log.filter(e => e[0] === 'userdata').map(e => [e[1], e[2].Played]), [['a', true], ['b', true]]);
    assert.ok(box.log.some(e => e[0] === 'toast' && e[1] === 'multiselect.marked_played|{"count":2}'));
    assert.equal(box.fns.selectedIds().length, 0);
});

test('a failure is reported, and only the items that worked change', async () => {
    const box = sandbox();
    const { cards } = grid(['a', 'b']);
    box.fns.toggleCard(cards[0]);
    box.fns.toggleCard(cards[1]);
    await box.fns.runBarAction('unfavorite');
    assert.deepEqual(box.log.filter(e => e[0] === 'userdata').map(e => e[1]), ['a']);
    assert.ok(box.log.some(e => e[0] === 'toast' && e[2] === 'error' && e[1].startsWith('multiselect.some_failed')));
});

test('Add to playlist opens the add dialog with every picked id', async () => {
    const box = sandbox();
    const { cards } = grid(['a', 'b']);
    box.fns.toggleCard(cards[0]);
    box.fns.toggleCard(cards[1]);
    await box.fns.runBarAction('add-playlist');
    assert.deepEqual(plain(box.log.find(e => e[0] === 'add-to').slice(1)), [['a', 'b'], 'playlist', 'function']);
});

test('Delete asks once, then removes each picked item and draws the page again', async () => {
    const no = sandbox({ isAdmin: true, confirmAnswer: false });
    const g1 = grid(['a', 'b']);
    no.fns.toggleCard(g1.cards[0]);
    await no.fns.runBarAction('delete');
    assert.ok(!no.log.some(e => e[0] === 'delete'));

    const yes = sandbox({ isAdmin: true });
    const g2 = grid(['a', 'b']);
    yes.fns.toggleCard(g2.cards[0]);
    yes.fns.toggleCard(g2.cards[1]);
    await yes.fns.runBarAction('delete');
    assert.equal(yes.log.filter(e => e[0] === 'confirm').length, 1);
    assert.deepEqual(yes.log.filter(e => e[0] === 'delete').map(e => e[1]), ['a', 'b']);
    assert.ok(yes.log.some(e => e[0] === 'dispatch' && e[1] === 'hashchange'));
});

test('Refresh metadata uses the refresh dialog for all picked items', async () => {
    const box = sandbox({ isAdmin: true });
    const { cards } = grid(['a', 'b']);
    box.fns.toggleCard(cards[0]);
    box.fns.toggleCard(cards[1]);
    await box.fns.runBarAction('refresh');
    const refresh = box.log.find(e => e[0] === 'refresh-dialog')[1];
    await refresh({ MetadataRefreshMode: 'Default' });
    assert.deepEqual(box.log.filter(e => e[0] === 'refresh').map(e => e[1]), ['a', 'b']);
});

test('the library grid opts in, and every string is in English and German', async () => {
    assert.match(read('modules/library.js'), /id="lib-grid-container" data-multiselect/);
    assert.match(read('app.js'), /import '\.\/modules\/multiselect\.js';/);
    const box = sandbox({ isAdmin: true });
    const { cards } = grid(['a']);
    box.fns.toggleCard(cards[0]);
    await box.fns.runBarAction('played');
    for (const extra of ['multiselect.some_failed', 'multiselect.marked_unplayed', 'multiselect.favorited', 'multiselect.unfavorited',
        'multiselect.refresh_started', 'multiselect.refreshed', 'multiselect.delete_confirm', 'multiselect.deleted']) {
        box.keys.add(extra);
    }
    const plural = new Set(['multiselect.count', 'multiselect.some_failed', 'multiselect.marked_played', 'multiselect.marked_unplayed',
        'multiselect.favorited', 'multiselect.unfavorited', 'multiselect.refresh_started', 'multiselect.refreshed',
        'multiselect.delete_confirm', 'multiselect.deleted']);
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`));
        for (const key of box.keys) {
            const forms = plural.has(key) ? [`${key}_one`, `${key}_other`] : [key];
            for (const form of forms) {
                assert.equal(typeof form.split('.').reduce((o, k) => o?.[k], strings), 'string', `${lang}: ${form}`);
            }
        }
    }
});
