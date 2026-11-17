// R3 cards: a card's badges (played check, unplayed count, progress, favourite), its played,
// favourite and menu buttons, the banner and list shapes, the context menu's commands per item and
// user, likes and dislikes, and a mark redrawn on every card of the item, as Jellyfin Web's cards.
// Runs modules/cards.js, modules/userdata.js and modules/cardmenu.js in a vm sandbox, and api.js's setRating.
// Usage: node r3cards_cards.mjs (run by api_tests.rs r3cards_cards).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = source => source.replace(/^import [^;]*;$/gms, '').replace(/^export \{[^}]*\};$/gms, '');

const escapeHtml = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

function sandbox({ isAdmin = false, item = null } = {}) {
    const calls = [];
    const toasts = [];
    const events = [];
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml,
        blurhashTint: () => null,
        canDownload: () => false,
        toast: (msg, type) => toasts.push([msg, type]),
        playQueueAt: (items, index) => calls.push(['queue', items.map(i => i.Id), index]),
        CSS: { escape: s => s },
        CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init.detail; } },
        document: {
            querySelectorAll: () => [],
            addEventListener: () => {},
            dispatchEvent: e => events.push(e),
        },
        API: {
            isAdmin,
            get: async url => { calls.push(['get', url]); return item; },
            markPlayed: async id => { calls.push(['played', id]); return { Played: true, PlayCount: 1 }; },
            markUnplayed: async id => { calls.push(['unplayed', id]); return {}; },
            setFavorite: async id => calls.push(['fav', id]),
            unsetFavorite: async id => calls.push(['unfav', id]),
            setRating: async (id, likes) => calls.push(['rating', id, likes]),
            invalidateCache: prefix => calls.push(['invalidate', prefix]),
        },
    });
    vm.runInContext(`${strip(read('modules/cards.js'))}\n${strip(read('modules/userdata.js'))}\n${strip(read('modules/cardmenu.js'))}
        this.fns = { itemCard, cardState, cardMarksHtml, applyCardUserData, cardMenuCommands, cardMenuHtml,
            toggleCardPlayed, toggleCardFavorite, setCardLikes, playCardItem, openCardMenu, openAddTo, cardPointerDown,
        cardPointerMove, cardContextMenu, cardClickAfterLongPress, LONG_PRESS_MS };`, context);
    return { fns: context.fns, calls, toasts, events, context };
}

const movie = (userData = {}, extra = {}) => ({
    Id: 'm1', Name: 'Alpha', Type: 'Movie', ProductionYear: 2001, ImageTags: { Primary: 'p1' }, UserData: userData, ...extra,
});

test('a played item shows the check, and its played button is pressed', () => {
    const { fns } = sandbox();
    const html = fns.itemCard(movie({ Played: true, PlayedPercentage: 40 }));
    assert.match(html, /class="jf-card-check"/);
    assert.match(html, /data-played="true"/);
    assert.match(html, /data-action="card-played" data-id="m1" aria-label="detail.mark_unplayed"[^>]*aria-pressed="true"/);
    // Played wins over a stale progress
    assert.doesNotMatch(html, /jf-progress-bar/);
});

test('a started item shows its progress; a favourite its heart and pressed button', () => {
    const { fns } = sandbox();
    const html = fns.itemCard(movie({ PlayedPercentage: 40, IsFavorite: true }));
    assert.match(html, /class="jf-progress-bar" style="width:40%"/);
    assert.doesNotMatch(html, /jf-card-check/);
    assert.match(html, /data-icon="Heart"/);
    assert.match(html, /data-action="card-favorite"[^>]*aria-pressed="true"/);
    assert.match(html, /data-action="card-menu"[^>]*aria-haspopup="menu"/);
});

test('a series counts its unplayed episodes; played, it shows the check instead', () => {
    const { fns } = sandbox();
    const series = played => ({ Id: 's1', Name: 'Show', Type: 'Series', IsFolder: true, UserData: { UnplayedItemCount: played ? 0 : 7, Played: played } });
    assert.match(fns.itemCard(series(false)), /<span class="jf-card-count" title="cards.unplayed_count\|\{&quot;count&quot;:7\}">7<\/span>/);
    const done = fns.itemCard(series(true));
    assert.match(done, /jf-card-check/);
    assert.doesNotMatch(done, /jf-card-count/);
    // A movie never shows a count
    assert.doesNotMatch(fns.itemCard(movie({ UnplayedItemCount: 3 })), /jf-card-count/);
    assert.match(fns.itemCard({ ...series(false), UserData: { UnplayedItemCount: 250 } }), />99\+</);
});

test('banner and list shapes', () => {
    const { fns } = sandbox();
    const banner = fns.itemCard(movie({}, { ImageTags: { Primary: 'p1', Banner: 'b1' } }), { banner: true });
    assert.match(banner, /class="jf-poster-banner"/);
    assert.match(banner, /data-img-type="Banner" data-img-tag="b1"/);
    // No banner: the landscape picks, here the Primary letterboxed
    assert.match(fns.itemCard(movie(), { banner: true }), /data-img-type="Primary"/);
    const row = fns.itemCard(movie({ Played: true }, { RunTimeTicks: 72000000000 }), { list: true });
    assert.match(row, /class="jf-card jf-list-row group"/);
    assert.match(row, /2001 · Movie · cards.minutes\|\{&quot;count&quot;:120\}/);
    assert.match(row, /jf-card-check/);
    assert.match(row, /data-action="card-menu"/);
});

// Values from the sandbox, as plain values of this realm
const plain = value => JSON.parse(JSON.stringify(value));
const actions = commands => plain(commands.map(c => (c === 'separator' ? '|' : c.action)));

test('the menu: a movie for a user, an episode for an administrator, a series', () => {
    const { fns } = sandbox();
    assert.deepEqual(actions(fns.cardMenuCommands(movie(), false)),
        ['card-play', '|', 'card-played', 'card-favorite', 'card-like', 'card-dislike', '|', 'card-add-playlist']);
    const episode = { Id: 'e1', Name: 'Pilot', Type: 'Episode', SeriesId: 's1', SeasonId: 'n1', UserData: { Played: true, Likes: true } };
    const commands = fns.cardMenuCommands(episode, true);
    assert.deepEqual(actions(commands), [
        'card-play', '|', 'card-played', 'card-favorite', 'card-like', 'card-dislike', '|', 'card-add-playlist',
        'card-add-collection', '|', 'navigate', 'navigate', '|', 'refresh-item', 'delete-item',
    ]);
    assert.equal(commands[2].label, 'detail.mark_unplayed');
    assert.equal(commands[4].checked, true);
    assert.equal(commands[5].checked, false);
    assert.deepEqual(plain(commands.filter(c => c.action === 'navigate').map(c => c.target)), ['s1', 'n1']);
    // Delete is the item page's flow: it goes to the season afterwards
    const del = commands.find(c => c.action === 'delete-item');
    assert.deepEqual(plain([del.name, del.type, del.parentId]), ['Pilot', 'Episode', 'n1']);
    const html = fns.cardMenuHtml(episode, commands);
    assert.match(html, /role="menuitemcheckbox" aria-checked="true" data-action="card-like" data-id="e1"/);
    assert.match(html, /data-action="navigate" data-id="s1" data-page="detail"/);
    assert.match(html, /data-action="delete-item" data-id="e1" data-name="Pilot" data-type="Episode" data-parent-id="n1"/);
    const series = actions(fns.cardMenuCommands({ Id: 's1', Type: 'Series', IsFolder: true, UserData: {} }, false));
    assert.deepEqual(series.slice(0, 2), ['play-all', 'shuffle']);
    const track = fns.cardMenuCommands({ Id: 't1', Type: 'Audio', AlbumId: 'a1', UserData: {} }, false);
    assert.equal(track.find(c => c.action === 'navigate').target, 'a1');
});

test('play from the menu goes through the queue', async () => {
    const item = movie();
    const s = sandbox({ item });
    const menu = { innerHTML: '', style: {}, matches: () => false, showPopover() {}, hidePopover() {}, setAttribute() {},
        addEventListener() {}, getBoundingClientRect: () => ({ width: 100, height: 100 }), querySelector: () => null };
    s.context.document.getElementById = () => menu;
    s.context.innerWidth = 1000;
    s.context.innerHeight = 800;
    await s.fns.openCardMenu({ dataset: { id: 'm1' }, getBoundingClientRect: () => ({ right: 500, bottom: 300, height: 30 }) });
    assert.deepEqual(s.calls[0], ['get', '/Items/m1']);
    assert.match(menu.innerHTML, /data-action="card-play"/);
    s.fns.playCardItem({ dataset: { id: 'm1' } });
    assert.deepEqual(plain(s.calls.at(-1)), ['queue', ['m1'], 0]);
});

test('marks from a card toggle on the server and tell the open lists', async () => {
    const s = sandbox();
    const card = { dataset: { played: 'false', favorite: 'true' } };
    const button = id => ({ dataset: { id }, closest: () => card, getAttribute: () => null });
    await s.fns.toggleCardPlayed(button('m1'));
    await s.fns.toggleCardFavorite(button('m1'));
    assert.deepEqual(s.calls.filter(c => c[0] !== 'invalidate'), [['played', 'm1'], ['unfav', 'm1']]);
    assert.deepEqual(s.events.map(e => [e.type, e.detail.id, JSON.stringify(e.detail.data)]), [
        ['jf-userdata', 'm1', '{"Played":true,"PlayCount":1}'],
        ['jf-userdata', 'm1', '{"IsFavorite":false}'],
    ]);
    assert.ok(s.calls.some(c => c[0] === 'invalidate' && c[1] === '/Items'));
    assert.deepEqual(s.toasts, [['detail.marked_played', undefined], ['detail.favorite_removed', undefined]]);
});

test('like and dislike: the one already chosen clears the rating', async () => {
    const s = sandbox();
    const item = checked => ({ dataset: { id: 'm1' }, getAttribute: () => String(checked) });
    await s.fns.setCardLikes(item(false), true);
    await s.fns.setCardLikes(item(false), false);
    await s.fns.setCardLikes(item(true), true);
    assert.deepEqual(s.calls.filter(c => c[0] === 'rating'), [['rating', 'm1', true], ['rating', 'm1', false], ['rating', 'm1', null]]);
    assert.deepEqual(s.toasts.map(([m]) => m), ['cards.liked', 'cards.disliked', 'cards.rating_cleared']);
});

test('a mark redraws the badges and buttons of every card of the item', () => {
    const { fns, context } = sandbox();
    const make = () => {
        const parts = {};
        return {
            dataset: { played: 'false', favorite: 'false', progress: '30', unplayed: '4', likes: 'null', folder: 'true', rating: '' },
            querySelector: sel => (parts[sel] ??= { outerHTML: '' }),
            parts,
        };
    };
    const cards = [make(), make()];
    context.document.querySelectorAll = sel => {
        assert.equal(sel, '.jf-card[data-id="s1"]');
        return cards;
    };
    fns.applyCardUserData('s1', { Played: true });
    for (const card of cards) {
        assert.equal(card.dataset.played, 'true');
        assert.equal(card.dataset.progress, '0');
        assert.equal(card.dataset.unplayed, '0');
        assert.match(card.parts['.jf-card-marks'].outerHTML, /jf-card-check/);
        assert.match(card.parts['.jf-card-actions'].outerHTML, /data-action="card-played"[^>]*aria-pressed="true"/);
    }
    fns.applyCardUserData('s1', { IsFavorite: true, Likes: false });
    assert.equal(cards[0].dataset.favorite, 'true');
    assert.equal(cards[0].dataset.likes, 'false');
});

// A popover menu element as openCardMenu uses it, counting its opens
function fakeMenu() {
    return {
        innerHTML: '', style: {}, shown: 0, matches: () => false, showPopover() { this.shown++; }, hidePopover() {},
        setAttribute() {}, addEventListener() {}, getBoundingClientRect: () => ({ width: 100, height: 100 }), querySelector: () => null,
    };
}

function menuSandbox() {
    const s = sandbox();
    const menu = fakeMenu();
    const pending = [];
    s.context.document.getElementById = () => menu;
    s.context.innerWidth = 1000;
    s.context.innerHeight = 800;
    s.context.location = { hash: '#library/films' };
    // Each item answer waits until the test resolves it
    s.context.API.get = url => new Promise(resolve => pending.push({ url, resolve }));
    const anchor = id => ({ dataset: { id }, getBoundingClientRect: () => ({ right: 500, bottom: 300, height: 30 }) });
    return { ...s, menu, pending, anchor };
}

test('a slow item answer for an earlier open, or after leaving the page, opens no menu', async () => {
    const s = menuSandbox();
    const first = s.fns.openCardMenu(s.anchor('a'));
    const second = s.fns.openCardMenu(s.anchor('b'));
    s.pending[1].resolve(movie({}, { Id: 'b', Name: 'Bravo' }));
    await second;
    s.pending[0].resolve(movie({}, { Id: 'a', Name: 'Alpha' }));
    await first;
    assert.equal(s.menu.shown, 1);
    assert.match(s.menu.innerHTML, /data-id="b"/);
    assert.doesNotMatch(s.menu.innerHTML, /data-id="a"/);
    const third = s.fns.openCardMenu(s.anchor('c'));
    s.context.location.hash = '#home';
    s.pending[2].resolve(movie({}, { Id: 'c' }));
    await third;
    assert.equal(s.menu.shown, 1);
});

test('Escape in a reused add-to dialog adds nothing', async () => {
    const s = sandbox();
    const added = [];
    const dialog = {
        dataset: {}, innerHTML: '', returnValue: 'ok', seenOnOpen: null,
        querySelector: sel => (sel === 'select' ? dialog.select : dialog.input),
        select: { value: 'p1', addEventListener() {}, focus() {} },
        input: { value: '', hidden: false, focus() {} },
        showModal() { this.seenOnOpen = this.returnValue; },
    };
    s.context.document.getElementById = () => dialog;
    Object.assign(s.context.API, {
        getPlaylists: async () => [{ Id: 'p1', Name: 'Mix' }],
        addToPlaylist: async (id, ids) => added.push([id, ids]),
        createPlaylist: async (name, ids) => added.push([name, ids]),
    });
    // The last use of the dialog ended with OK; this one is closed with Escape (returnValue stays)
    await s.fns.openAddTo({ dataset: { id: 'm1' } }, 'playlist');
    assert.equal(dialog.seenOnOpen, '');
    dialog.onclose();
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(added, []);
    // OK still adds
    await s.fns.openAddTo({ dataset: { id: 'm1' } }, 'playlist');
    dialog.returnValue = 'ok';
    dialog.onclose();
    await new Promise(resolve => setImmediate(resolve));
    assert.deepEqual(plain(added), [['p1', ['m1']]]);
});

test('a long touch press opens the menu (iOS fires no contextmenu); a move cancels it', async () => {
    const s = menuSandbox();
    const timers = [];
    s.context.setTimeout = (f, ms) => { timers.push({ f, ms, live: true }); return timers.length; };
    s.context.clearTimeout = id => { if (timers[id - 1]) timers[id - 1].live = false; };
    const card = { dataset: { id: 'm1' }, closest: sel => (sel === '#card-menu' ? null : card), getBoundingClientRect: () => ({}) };
    const target = { closest: () => card };
    const down = (pointerType, x = 10, y = 10) => s.fns.cardPointerDown({ pointerType, target, clientX: x, clientY: y });
    const fire = () => timers.filter(t => t.live).forEach(t => { t.live = false; t.f(); });
    // A mouse press does nothing; right click is the mouse's way
    down('mouse');
    assert.equal(timers.length, 0);
    // A press that moves (a scroll) is no long press
    down('touch');
    assert.equal(timers[0].ms, s.fns.LONG_PRESS_MS);
    s.fns.cardPointerMove({ clientX: 10, clientY: 40 });
    fire();
    assert.equal(s.pending.length, 0);
    // A still press opens the menu at the finger
    down('touch');
    s.fns.cardPointerMove({ clientX: 12, clientY: 11 });
    fire();
    assert.deepEqual(s.pending.map(p => p.url), ['/Items/m1']);
    // The click that ends the press, and Android's contextmenu, don't open the item or a second menu
    const click = { target, prevented: false, stopped: false, preventDefault() { this.prevented = true; }, stopPropagation() { this.stopped = true; } };
    s.fns.cardClickAfterLongPress(click);
    assert.ok(click.prevented && click.stopped);
    s.fns.cardContextMenu({ target, clientX: 1, clientY: 1, preventDefault() {} });
    assert.equal(s.pending.length, 1);
});

test("api.js setRating is Jellyfin's likes call", async () => {
    const sent = [];
    const context = vm.createContext({
        window: { location: { origin: 'http://x' } },
        localStorage: { getItem: () => null },
    });
    vm.runInContext(`${read('api.js')}\nthis.API = API;`, context);
    const api = context.API;
    api.post = async path => sent.push(['POST', path]);
    api.del = async path => sent.push(['DELETE', path]);
    await api.setRating('i 1', true);
    await api.setRating('i1', false);
    await api.setRating('i1', null);
    assert.deepEqual(sent, [
        ['POST', '/UserItems/i%201/Rating?likes=true'],
        ['POST', '/UserItems/i1/Rating?likes=false'],
        ['DELETE', '/UserItems/i1/Rating'],
    ]);
});
