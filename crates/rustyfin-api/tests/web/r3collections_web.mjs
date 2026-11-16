// R3 collections: the web client's collections and playlists. Runs modules/collections.js and
// cardmenu.js's menu builder in a vm sandbox with the DOM and API stubbed, and checks music.js and
// the translations as text. Usage: node r3collections_web.mjs (run by api_tests.rs r3collections_web).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = src => src.replace(/^import .*$/gm, '').replace(/^export \{[^}]*\};$/gms, '');
const plain = value => JSON.parse(JSON.stringify(value));

function sandbox({ isAdmin = false } = {}) {
    const log = [];
    const elements = {};
    const context = vm.createContext({
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'),
        $: id => elements[id] || null,
        toast: (msg, kind) => log.push(['toast', msg, kind || 'ok']),
        itemListRow: it => `<row ${it.Id}>`,
        createCardElement: it => ({ html: `<card ${it.Id}>` }),
        playQueueAt: (items, index) => log.push(['queue', items.map(i => i.Id), index]),
        navigate: (page, id) => log.push(['navigate', page, id]),
        applyCardUserData: () => {},
        document: { addEventListener() {}, getElementById: () => null },
        API: {
            token: 'tok',
            userId: 'u1',
            isAdmin,
            invalidateCache: () => {},
            async movePlaylistItem(...args) { log.push(['move', ...args]); },
            async removeFromPlaylist(...args) { log.push(['remove-playlist', ...args]); },
            async removeFromCollection(...args) { log.push(['remove-collection', ...args]); },
            async post(path, body) { log.push(['post', path, body]); return { Id: 'new1' }; },
        },
    });
    vm.runInContext(`${strip(read('modules/collections.js'))}
        this.fns = { KINDS, containerListUrl, containerItemsUrl, playlistEntryHtml, dropIndex, movePlaylistEntry,
            removeFromContainer, playContainerFrom, createContainer, containerSectionHtml,
            setOpen: value => { openContainer = value; }, getOpen: () => openContainer };`, context);
    vm.runInContext(`${strip(read('modules/cardmenu.js'))}
        this.menu = { cardMenuCommands };`, context);
    return { fns: context.fns, menu: context.menu, log, elements };
}

test('the lists ask for every collection or playlist, or under a library those holding its items', () => {
    const { fns } = sandbox();
    const all = new URL(fns.containerListUrl('collections'), 'http://x');
    assert.equal(all.pathname, '/Items');
    assert.equal(all.searchParams.get('IncludeItemTypes'), 'BoxSet');
    assert.equal(all.searchParams.get('ParentId'), null);
    const under = new URL(fns.containerListUrl('playlists', 'lib1'), 'http://x');
    assert.equal(under.searchParams.get('IncludeItemTypes'), 'Playlist');
    assert.equal(under.searchParams.get('ParentId'), 'lib1');
    assert.equal(under.searchParams.get('SortBy'), 'SortName');
});

test("a playlist's items come with their PlaylistItemIds; a collection's from /Items under it", () => {
    const { fns } = sandbox();
    assert.match(fns.containerItemsUrl({ Id: 'p1', Type: 'Playlist' }), /^\/Playlists\/p1\/Items/);
    assert.match(fns.containerItemsUrl({ Id: 'c1', Type: 'BoxSet' }), /^\/Items\?ParentId=c1/);
    assert.equal(fns.containerSectionHtml({ Id: 'm', Type: 'Movie' }), '');
    assert.match(fns.containerSectionHtml({ Id: 'p1', Type: 'Playlist' }), /data-container-id="p1" data-container-type="Playlist"/);
});

test("an editor's playlist rows drag and have Remove; a reader's don't", () => {
    const { fns } = sandbox();
    const entry = { Id: 'a1', Name: 'Song', PlaylistItemId: 'e1' };
    const editable = fns.playlistEntryHtml(entry, 2, true);
    assert.match(editable, /draggable="true"/);
    assert.match(editable, /data-action="card-remove-from" data-id="a1" data-entry-id="e1"/);
    assert.match(editable, /data-action="playlist-play-from" data-index="2"/);
    const readOnly = fns.playlistEntryHtml(entry, 2, false);
    assert.doesNotMatch(readOnly, /draggable|card-remove-from|GripVertical/);
    assert.match(readOnly, /playlist-play-from/);
});

test('a drop lands before a row in its upper half and after it in its lower half', () => {
    const { fns } = sandbox();
    const row = top => ({ getBoundingClientRect: () => ({ top, height: 40 }) });
    const rows = [row(0), row(40), row(80)];
    const list = { children: rows };
    assert.equal(fns.dropIndex(list, rows[1], 45), 1);
    assert.equal(fns.dropIndex(list, rows[1], 75), 2);
});

test('a move reorders the page and saves through Jellyfin\'s Move with the entry id', async () => {
    const { fns, log } = sandbox();
    const items = ['a', 'b', 'c'].map(id => ({ Id: id, PlaylistItemId: `e${id}` }));
    fns.setOpen({ item: { Id: 'p1', Type: 'Playlist' }, items, canEdit: true });
    await fns.movePlaylistEntry(2, 0);
    assert.deepEqual(plain(fns.getOpen().items.map(i => i.Id)), ['c', 'a', 'b']);
    assert.deepEqual(plain(log.find(l => l[0] === 'move')), ['move', 'p1', 'ec', 0]);
});

test('Remove takes a playlist entry by its PlaylistItemId and a collection item by its id', async () => {
    const { fns, log } = sandbox();
    const items = [{ Id: 'a', PlaylistItemId: 'ea' }, { Id: 'b', PlaylistItemId: 'eb' }];
    fns.setOpen({ item: { Id: 'p1', Type: 'Playlist' }, items, canEdit: true });
    await fns.removeFromContainer({ dataset: { id: 'a', entryId: 'ea', containerId: 'p1', containerType: 'Playlist' }, closest: () => null });
    assert.deepEqual(plain(log.find(l => l[0] === 'remove-playlist')), ['remove-playlist', 'p1', ['ea']]);
    assert.deepEqual(plain(fns.getOpen().items.map(i => i.Id)), ['b']);
    await fns.removeFromContainer({ dataset: { id: 'm1', containerId: 'c1', containerType: 'BoxSet' }, closest: () => null });
    assert.deepEqual(plain(log.find(l => l[0] === 'remove-collection')), ['remove-collection', 'c1', ['m1']]);
});

test('playing from an entry queues the playlist from that entry', () => {
    const { fns, log } = sandbox();
    const items = [{ Id: 'a' }, { Id: 'b' }, { Id: 'c' }];
    fns.setOpen({ item: { Id: 'p1', Type: 'Playlist' }, items, canEdit: false });
    fns.playContainerFrom({ dataset: { index: '1' } });
    assert.deepEqual(plain(log.at(-1)), ['queue', ['a', 'b', 'c'], 1]);
});

test('a new playlist is made for the user, public when asked, and opens', async () => {
    const { fns, log } = sandbox();
    await fns.createContainer('playlists', 'Road trip', true);
    assert.deepEqual(plain(log[0]), ['post', '/Playlists', { Name: 'Road trip', Ids: [], UserId: 'u1', IsPublic: true }]);
    assert.deepEqual(plain(log[1]), ['navigate', 'detail', 'new1']);
    await fns.createContainer('collections', '', false);
    assert.equal(log.at(-1)[1], 'cards.name_required');
});

test("the card menu offers Remove from the collection or playlist the card is in, to its editors only", () => {
    const { menu } = sandbox({ isAdmin: true });
    const movie = { Id: 'm1', Type: 'Movie', UserData: {} };
    const actions = cmds => cmds.filter(c => c !== 'separator').map(c => c.action);
    assert.ok(!actions(menu.cardMenuCommands(movie, true)).includes('card-remove-from'));
    const inSet = menu.cardMenuCommands(movie, true, { id: 'c1', type: 'BoxSet', canEdit: true });
    const remove = inSet.find(c => c.action === 'card-remove-from');
    assert.equal(remove.containerId, 'c1');
    assert.equal(remove.label, 'collections.remove_from_collection');
    const reader = menu.cardMenuCommands(movie, false, { id: 'p1', type: 'Playlist', canEdit: false });
    assert.ok(!actions(reader).includes('card-remove-from'));
    const inList = menu.cardMenuCommands(movie, false, { id: 'p1', type: 'Playlist', canEdit: true, entryId: 'e9' });
    assert.equal(inList.find(c => c.action === 'card-remove-from').entryId, 'e9');
});

test('the music library has a Playlists tab, and the strings are in English and German', () => {
    const music = read('modules/music.js');
    assert.match(music, /\['playlists', 'music\.tab_playlists'\]/);
    assert.match(music, /case 'playlists':[\s\S]*?IncludeItemTypes: 'Playlist'/);
    const router = read('modules/router.js');
    for (const action of ['card-remove-from', 'playlist-play-from', 'new-container']) assert.match(router, new RegExp(`'${action}'`));
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`));
        for (const key of ['collections', 'playlists', 'new_collection', 'new_playlist', 'remove_from_playlist', 'remove_from_collection', 'items_count_other']) {
            assert.ok(strings.collections[key], `${lang} collections.${key}`);
        }
        assert.ok(strings.music.tab_playlists, `${lang} music.tab_playlists`);
    }
});
