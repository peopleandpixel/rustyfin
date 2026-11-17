// Instant mix: the card menu offers it for songs, albums, artists, genres and playlists, its request is
// /Items/{id}/InstantMix with the user and a limit of 200, and it plays the result as the play queue. The item
// page's more button opens the same menu. Runs cardmenu.js's functions in a vm sandbox with stubs.
// Usage: node r3music2_instantmix.mjs (run by api_tests.rs r3music2_instantmix).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const cardmenu = read('modules/cardmenu.js');
const item = read('modules/item.js');
const router = read('modules/router.js');
const en = JSON.parse(read('translations/en.json'));

// The menu's constants and cardMenuCommands, and the Instant mix functions, from cardmenu.js
function between(source, head, end) {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `has ${head}`);
    const stop = source.indexOf(end, start);
    assert.ok(stop > start, `has ${end}`);
    return source.slice(start, stop);
}

function sandbox(api) {
    const calls = { queue: [], toast: [] };
    const context = vm.createContext({
        t: key => key,
        icon: name => `<i:${name}>`,
        escapeHtml: s => String(s),
        toast: (message, kind) => calls.toast.push([message, kind]),
        playQueueAt: (items, index) => calls.queue.push({ items, index }),
        canDownload: () => false,
        API: api,
        URLSearchParams,
        String,
        console,
    });
    vm.runInContext(
        `${between(cardmenu, 'const PLAYABLE', 'function cardMenuHtml(')}
         ${between(cardmenu, "// The request for an item's Instant mix", '// Add to playlist or collection')}
         this.fns = { cardMenuCommands, instantMixUrl, playInstantMix, menuItems };`,
        context,
    );
    return { ...context.fns, calls };
}

const user = { userId: 'u1', get: async () => ({ Items: [] }) };
const offered = (type, extra = {}) => sandbox(user).cardMenuCommands({ Id: 'x', Type: type, UserData: {}, ...extra }, false)
    .some(c => c.action === 'card-instant-mix');

test('the menu offers Instant mix for songs, albums, artists, genres and playlists', () => {
    for (const type of ['Audio', 'MusicAlbum', 'MusicArtist', 'MusicGenre', 'Playlist']) {
        assert.equal(offered(type), true, `${type} offers it`);
    }
    for (const type of ['Movie', 'Series', 'Episode', 'Person', 'BoxSet']) {
        assert.equal(offered(type), false, `${type} does not`);
    }
    const commands = sandbox(user).cardMenuCommands({ Id: 's1', Type: 'Audio', UserData: {} }, false);
    const mix = commands.find(c => c.action === 'card-instant-mix');
    assert.equal(mix.label, 'cards.instant_mix');
    assert.equal(mix.icon, 'ListMusic');
    assert.equal(en.cards.instant_mix, 'Instant mix');
});

test('the Instant mix request names the item, the user and a limit of 200', () => {
    const { instantMixUrl } = sandbox(user);
    assert.equal(instantMixUrl('s1'), '/Items/s1/InstantMix?UserId=u1&Limit=200');
    assert.equal(instantMixUrl('a b'), '/Items/a%20b/InstantMix?UserId=u1&Limit=200');
});

test('Instant mix plays the result as the queue, from its first item, without folders', async () => {
    const requested = [];
    const api = {
        userId: 'u1',
        get: async url => {
            requested.push(url);
            return { Items: [{ Id: 's1', Name: 'Song', Type: 'Audio' }, { Id: 'f', IsFolder: true }, { Id: 's2', Type: 'Audio' }] };
        },
    };
    const sb = sandbox(api);
    sb.menuItems.set('s1', { Id: 's1', Type: 'Audio' });
    await sb.playInstantMix({ dataset: { id: 's1' } });
    assert.deepEqual(requested, ['/Items/s1/InstantMix?UserId=u1&Limit=200']);
    assert.equal(sb.calls.queue.length, 1);
    assert.deepEqual(sb.calls.queue[0].items.map(i => i.Id), ['s1', 's2']);
    assert.equal(sb.calls.queue[0].index, 0);
});

test('an empty or failed Instant mix says so and plays nothing', async () => {
    const empty = sandbox({ userId: 'u1', get: async () => ({ Items: [] }) });
    empty.menuItems.set('g', { Id: 'g', Type: 'MusicGenre' });
    await empty.playInstantMix({ dataset: { id: 'g' } });
    assert.deepEqual(empty.calls.toast, [['player.queue_empty', 'error']]);
    assert.equal(empty.calls.queue.length, 0);

    const failed = sandbox({ userId: 'u1', get: async () => { throw new Error('boom'); } });
    failed.menuItems.set('g', { Id: 'g', Type: 'MusicGenre' });
    await failed.playInstantMix({ dataset: { id: 'g' } });
    assert.equal(failed.calls.toast.length, 1);
    assert.equal(failed.calls.toast[0][1], 'error');
    assert.equal(failed.calls.queue.length, 0);
});

test('the router and the item page reach the same menu and action', () => {
    assert.ok(router.includes("'card-instant-mix': el => playInstantMix(el),"));
    assert.ok(router.includes('playInstantMix,'));
    assert.ok(item.includes('data-action="card-menu"'), 'the item page more button opens the card menu');
});
