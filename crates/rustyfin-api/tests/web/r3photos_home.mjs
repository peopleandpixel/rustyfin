// R3 photos and home: the home page follows the user's library settings as Jellyfin Web's does:
// My Media is the server's /UserViews (in OrderedViews order, without MyMediaExcludes), its
// Recently Added rows leave out the LatestItemsExcludes libraries; and the Home settings save
// those lists. Runs
// modules/user-settings.js, modules/home.js and modules/settings.js in a vm sandbox with the API
// and the card helpers stubbed.
// Usage: node r3photos_home.mjs (run by api_tests.rs r3photos_web).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = text => text.replace(/^import [^;]*;$/gm, '').replace(/^export \{[^}]*\};$/gm, '');
const source = ['modules/playerprefs.js', 'modules/user-settings.js', 'modules/home.js', 'modules/settings.js']
    .map(f => strip(read(f))).join('\n');

const LIBS = [
    { Id: 'aaaa-1111', Name: 'Films', ContentType: 'Movies' },
    { Id: 'bbbb-2222', Name: 'Shows', ContentType: 'TvShows' },
    { Id: 'cccc-3333', Name: 'Photos', ContentType: 'Photos' },
];
const latest = lib => `/Items/Latest?ParentId=${lib}&Limit=16&Fields=PrimaryImageAspectRatio`;

function sandbox({ config = {}, prefs = {}, views = LIBS } = {}) {
    const gets = [];
    const posts = [];
    const rows = [];
    const toasts = [];
    const page = { innerHTML: '' };
    const context = vm.createContext({
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s),
        scanRing: () => '',
        backdropUrl: () => '',
        containerTilesHtml: () => '',
        revealContainerTiles: async () => {},
        serverVersion: '0',
        toast: (msg, kind) => toasts.push([msg, kind]),
        $: id => (id === 'page-home' ? page : null),
        libraries: [],
        sectionRow: (title, items) => {
            if (!items.length) return '';
            rows.push(title);
            return `<row ${title}>`;
        },
        document: { documentElement: { classList: { toggle() {} } }, getElementById: () => null },
        window: {},
        API: {
            token: 'tok',
            userId: 'u1',
            isAdmin: true,
            isLoggedIn: () => true,
            getLibraries: async () => LIBS,
            // The server's /UserViews: the user's order, hidden libraries left out
            getUserViews: async () => views,
            getNextUp: async () => ({ Items: [] }),
            getMe: async () => ({ Id: 'u1', Configuration: config }),
            get: async url => {
                gets.push(url);
                return { Id: 'usersettings', CustomPrefs: prefs };
            },
            post: async (url, body) => {
                posts.push([url, JSON.parse(JSON.stringify(body))]);
                if (url === '/Users/u1/Configuration') config = body;
            },
            async cachedGet(url) {
                gets.push(url);
                return url.startsWith('/Items/Latest') ? [{ Id: `new-${url}` }] : { Items: [] };
            },
        },
    });
    vm.runInContext(`${source}
        Object.assign(this, { renderHome, arrangeLibraries, showsInMyMedia, homeLibrariesHtml, changeMyMediaLibrary,
            changeLatestLibrary, loadUserSettings, saveUserSettings });`, context);
    return { context, gets, posts, toasts, rows: () => [...rows], page };
}

const latestRows = s => s.rows().filter(r => r.startsWith('home.latest_in'));

test('the libraries follow OrderedViews: the named ones first in its order, then the rest in theirs', () => {
    const { context } = sandbox();
    const names = (config, opts = {}) => context.arrangeLibraries(LIBS, { config, ...opts }).map(l => l.Name);
    assert.deepEqual(names({}), ['Films', 'Shows', 'Photos']);
    assert.deepEqual(names({ OrderedViews: ['CCCC3333', 'aaaa-1111'] }), ['Photos', 'Films', 'Shows']);
    assert.deepEqual(names({ OrderedViews: ['bbbb-2222'], MyMediaExcludes: ['aaaa1111'] }), ['Shows', 'Films', 'Photos']);
    assert.equal(context.showsInMyMedia('AAAA-1111', { MyMediaExcludes: ['aaaa1111'] }), false);
});

test('the home page shows the libraries and their Recently Added rows in the server\'s order', async () => {
    const s = sandbox({ views: [LIBS[2], LIBS[1], LIBS[0]] });
    await s.context.renderHome();
    assert.deepEqual(latestRows(s), [
        'home.latest_in|{"name":"Photos"}',
        'home.latest_in|{"name":"Shows"}',
        'home.latest_in|{"name":"Films"}',
    ]);
    const html = s.page.innerHTML;
    const at = id => html.indexOf(`data-page="library" data-id="${id}"`);
    assert.ok(at('cccc-3333') > 0 && at('cccc-3333') < at('bbbb-2222') && at('bbbb-2222') < at('aaaa-1111'), 'tile order');
});

test('a library the server leaves out of My Media has no tile, no Recently Added row and no request', async () => {
    const s = sandbox({ views: [LIBS[1], LIBS[2]], config: { LatestItemsExcludes: ['CCCC3333'] } });
    await s.context.renderHome();
    assert.deepEqual(latestRows(s), ['home.latest_in|{"name":"Shows"}']);
    assert.ok(!s.page.innerHTML.includes('data-id="aaaa-1111"'));
    assert.ok(s.page.innerHTML.includes('data-id="cccc-3333"'), 'excluded from Latest only: its tile stays');
    assert.ok(!s.gets.includes(latest('aaaa-1111')) && !s.gets.includes(latest('cccc-3333')));
    // The global library list (the sidebar and library pages) still has every library
    assert.equal(s.context.libraries.length, 3);
});

test('the Home settings list the libraries in order with My Media and Recently Added switches', async () => {
    const config = { OrderedViews: ['bbbb-2222'], MyMediaExcludes: ['cccc-3333'], LatestItemsExcludes: ['aaaa-1111'] };
    const s = sandbox({ config });
    await s.context.loadUserSettings();
    const html = s.context.homeLibrariesHtml(config, LIBS);
    const order = [...html.matchAll(/data-home-library="([^"]+)"/g)].map(m => m[1]);
    assert.deepEqual(order, ['bbbb-2222', 'aaaa-1111', 'cccc-3333']);
    assert.match(html, /data-my-media-library="cccc-3333"[^>]*data-saved="false" >/);
    assert.match(html, /data-my-media-library="aaaa-1111"[^>]*data-saved="true" checked>/);
    assert.match(html, /data-latest-library="aaaa-1111"[^>]*data-saved="false" >/);
    // The first can't move up and the last can't move down
    assert.match(html, /data-id="bbbb-2222" data-delta="-1"[^>]*disabled>/);
    assert.match(html, /data-id="cccc-3333" data-delta="1"[^>]*disabled>/);
    assert.match(html, /data-action="move-home-library" data-id="aaaa-1111" data-delta="1"[^>]*"><svg/);
});

test('turning a library\'s My Media switch off and on saves MyMediaExcludes, keeping the rest', async () => {
    const s = sandbox({ config: { MyMediaExcludes: ['bbbb2222'], OrderedViews: ['x'] } });
    await s.context.loadUserSettings();
    const input = { type: 'checkbox', checked: false, dataset: { myMediaLibrary: 'aaaa-1111', saved: 'true' } };
    await s.context.changeMyMediaLibrary(input);
    assert.deepEqual(s.posts.at(-1), ['/Users/u1/Configuration', { MyMediaExcludes: ['bbbb2222', 'aaaa-1111'], OrderedViews: ['x'] }]);
    input.checked = true;
    await s.context.changeMyMediaLibrary(input);
    assert.deepEqual(s.posts.at(-1)[1].MyMediaExcludes, ['bbbb2222']);
    assert.equal(input.dataset.saved, 'true');
});

test('a home layout save posts the whole stored CustomPrefs set, read again first, as a POST replaces it', async () => {
    const stored = { homesection0: 'resume', enableBackdrops: 'false' };
    const s = sandbox({ prefs: stored });
    await s.context.loadUserSettings();
    // Another browser saves a setting meanwhile
    stored.datetimelocale = 'de-DE';
    await s.context.saveUserSettings({ homesection1: 'nextup', homesection0: '' });
    const [url, body] = s.posts.at(-1);
    assert.equal(url, '/DisplayPreferences/usersettings?userId=u1&client=emby');
    // Cleared keys are left out, not sent as null (Jellyfin 12.4 would store the null)
    assert.deepEqual(body.CustomPrefs, { enableBackdrops: 'false', datetimelocale: 'de-DE', homesection1: 'nextup' });
});

test('every new string is in English and German', () => {
    const keys = ['home_show_in_my_media', 'home_libraries', 'home_move_up', 'home_move_down'].map(k => `settings.${k}`);
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`));
        for (const key of keys) assert.equal(typeof key.split('.').reduce((o, k) => o?.[k], strings), 'string', `${lang}: ${key}`);
    }
});
