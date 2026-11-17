// R3 sync: the web client follows the server's UserDataChanged, as Jellyfin Web does. An incoming
// message redraws every visible card of its items and the folders above them (their unplayed counts
// read again), drops the cached lists, and is ignored for another user; this tab's own marks take the
// same path, so the server's echo of them draws nothing again. Runs modules/cards.js and
// modules/userdata.js in a vm sandbox with a small DOM, and checks the wiring in app.js and item.js.
// Usage: node r3sync_userdata.mjs (run by api_tests.rs r3sync_userdata).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = source => source.replace(/^import [^;]*;$/gms, '').replace(/^export \{[^}]*\};$/gms, '');

const escapeHtml = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

// A rendered card: its data-* and the parts applyCardUserData redraws
function card(id, dataset) {
    const parts = {};
    return {
        dataset: { id, played: 'false', favorite: 'false', progress: '0', unplayed: '0', likes: 'null', folder: 'false', rating: '', ...dataset },
        querySelector: sel => (parts[sel] ??= { outerHTML: '' }),
        parts,
    };
}

function sandbox({ cards = [], folders = {} } = {}) {
    const gets = [];
    const events = [];
    const timers = [];
    const cache = new Map();
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml,
        blurhashTint: () => null,
        CSS: { escape: s => s },
        setTimeout: fn => { timers.push(fn); return timers.length; },
        CustomEvent: class { constructor(type, init) { this.type = type; this.detail = init.detail; } },
        document: {
            querySelectorAll: sel => {
                const id = /data-id="([^"]*)"/.exec(sel)?.[1];
                return cards.filter(c => c.dataset.id === id);
            },
            addEventListener: () => {},
            dispatchEvent: e => events.push(e),
        },
        API: {
            userId: 'u1',
            _cache: cache,
            get: async url => {
                gets.push(url);
                const id = /\/UserItems\/([^/]+)\/UserData/.exec(url)?.[1];
                return folders[id];
            },
            invalidateCache(prefix) {
                for (const key of [...cache.keys()]) if (key.startsWith(prefix)) cache.delete(key);
            },
        },
    });
    vm.runInContext(`${strip(read('modules/cards.js'))}\n${strip(read('modules/userdata.js'))}
        this.fns = { itemCard, userDataChanged, serverUserDataChanged, setShownIds };`, context);
    // Runs the waiting timers (the folders' re-read) and lets their requests finish
    const flush = async () => {
        while (timers.length) await timers.shift()();
        await new Promise(resolve => setImmediate(resolve));
    };
    return { fns: context.fns, gets, events, cache, flush };
}

const message = (list, userId = 'u1') => ({ UserId: userId, UserDataList: list });

test('an incoming UserDataChanged redraws the episode card, its season, and its series read again', async () => {
    const episode = card('e1', { parents: 'se1 sr1', progress: '40' });
    const season = card('se1', { folder: 'true', unplayed: '3', parents: 'sr1' });
    const series = card('sr1', { folder: 'true', unplayed: '10' });
    const s = sandbox({ cards: [episode, season, series], folders: { sr1: { UnplayedItemCount: 9, Played: false }, se1: { UnplayedItemCount: 2, Played: false } } });

    // As the server sends it: the episode and its parent, the season
    s.fns.serverUserDataChanged(message([
        { ItemId: 'e1', Played: true, PlaybackPositionTicks: 0, IsFavorite: false },
        { ItemId: 'se1', Played: false, UnplayedItemCount: 2 },
    ]));
    assert.equal(episode.dataset.played, 'true');
    assert.equal(episode.dataset.progress, '0');
    assert.match(episode.parts['.jf-card-marks'].outerHTML, /jf-card-check/);
    assert.equal(season.dataset.unplayed, '2');
    assert.match(season.parts['.jf-card-marks'].outerHTML, /class="jf-card-count"[^>]*>2</);
    // The series, two levels up, isn't in the message: it reads its own data again
    assert.equal(series.dataset.unplayed, '10');
    await s.flush();
    assert.ok(s.gets.includes('/UserItems/sr1/UserData?userId=u1'), JSON.stringify(s.gets));
    assert.equal(series.dataset.unplayed, '9');
    assert.match(series.parts['.jf-card-marks'].outerHTML, /class="jf-card-count"[^>]*>9</);
    // The open lists hear of each change
    assert.deepEqual(s.events.map(e => e.detail.id).slice(0, 2), ['e1', 'se1']);
});

test('the cached lists go, other answers stay', () => {
    const s = sandbox();
    for (const key of ['/Items?ParentId=l1', '/Items/e1', '/Shows/NextUp?Limit=24', '/UserItems/Resume', '/Users/u1/Items/Latest', '/System/Info/Public']) {
        s.cache.set(key, { data: {}, time: 0 });
    }
    s.fns.serverUserDataChanged(message([{ ItemId: 'e1', Played: true }]));
    assert.deepEqual([...s.cache.keys()], ['/System/Info/Public']);
});

test("another user's message changes nothing on screen", () => {
    const episode = card('e1');
    const s = sandbox({ cards: [episode] });
    s.fns.serverUserDataChanged(message([{ ItemId: 'e1', Played: true }], 'u2'));
    assert.equal(episode.dataset.played, 'false');
    assert.equal(s.events.length, 0);
});

test("this tab's own mark takes the same path, and the server's echo of it draws nothing again", () => {
    const movie = card('m1');
    const s = sandbox({ cards: [movie] });
    s.fns.userDataChanged('m1', { IsFavorite: true });
    assert.equal(movie.dataset.favorite, 'true');
    assert.equal(s.events.length, 1);
    s.fns.serverUserDataChanged(message([{ ItemId: 'm1', IsFavorite: true }]));
    assert.equal(s.events.length, 1);
    // A change from another device does draw
    s.fns.serverUserDataChanged(message([{ ItemId: 'm1', IsFavorite: false }]));
    assert.equal(movie.dataset.favorite, 'false');
    assert.equal(s.events.length, 2);
});

test('the open item page counts as shown, so a folder page reads its counts again', async () => {
    const episode = card('e1', { parents: 'se1' });
    const s = sandbox({ cards: [episode], folders: { se1: { UnplayedItemCount: 1 } } });
    s.fns.setShownIds(() => ['se1']);
    s.fns.userDataChanged('e1', { Played: true });
    await s.flush();
    assert.deepEqual(s.gets, ['/UserItems/se1/UserData?userId=u1']);
    assert.ok(s.events.some(e => e.detail.id === 'se1' && e.detail.data.UnplayedItemCount === 1));
});

test("an episode's card names its season and series", () => {
    const s = sandbox();
    const html = s.fns.itemCard({ Id: 'e1', Name: 'Pilot', Type: 'Episode', SeasonId: 'se1', SeriesId: 'sr1', UserData: {} });
    assert.match(html, /data-parents="se1 sr1"/);
    const movie = s.fns.itemCard({ Id: 'm1', Name: 'Alpha', Type: 'Movie', UserData: {} });
    assert.doesNotMatch(movie, /data-parents/);
});

test('app.js subscribes to UserDataChanged; the item page and the queue follow the one path', () => {
    const app = read('app.js');
    assert.match(app, /onSocketMessage\('UserDataChanged', serverUserDataChanged\)/);
    assert.match(app, /document\.addEventListener\('jf-userdata', detailUserDataChanged\)/);
    assert.match(app, /document\.addEventListener\('jf-userdata', queueUserDataChanged\)/);
    const item = read('modules/item.js');
    assert.match(item, /userDataChanged\(id, \{ IsFavorite: !current \}\)/);
    assert.match(item, /userDataChanged\(id, \{ \.\.\.\(answer && answer\.Played !== undefined \? answer : \{\}\), Played: !current \}\)/);
    assert.match(read('modules/cardmenu.js'), /import \{ userDataChanged \} from '\.\/userdata\.js';/);
    assert.match(read('modules/multiselect.js'), /import \{ userDataChanged \} from '\.\/userdata\.js';/);
});
