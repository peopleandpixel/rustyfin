// R3 home: the home page has a "Recently Added in <library>" row per library, and separate resume
// rows for video (Continue Watching) and audio (Continue Listening), as Jellyfin Web's home page.
// Runs modules/home.js's renderHome in a vm sandbox with the API and the card helpers stubbed.
// Usage: node r3home_rows.mjs (run by api_tests.rs r3home_rows).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const home = read('modules/home.js');
const userSettings = read('modules/playerprefs.js') + '\n' + read('modules/user-settings.js');

const LIBS = [
    { Id: 'films', Name: 'Films', ContentType: 'Movies' },
    { Id: 'shows', Name: 'Shows', ContentType: 'TvShows' },
    { Id: 'tunes', Name: 'Tunes & More', ContentType: 'Music' },
    { Id: 'sets', Name: 'Collections', ContentType: 'BoxSets' },
    { Id: 'lists', Name: 'Playlists', ContentType: 'playlists' },
];

// answers: URL -> answer (or an Error to throw); unknown URLs answer an empty list
function sandbox(answers = {}, { libs = LIBS } = {}) {
    const gets = [];
    const rows = [];
    const page = { innerHTML: '' };
    const context = vm.createContext({
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s),
        scanRing: () => '',
        backdropUrl: () => '',
        // collections.js's Collections and Playlists tiles
        containerTilesHtml: () => '',
        revealContainerTiles: async () => {},
        serverVersion: '0',
        $: id => (id === 'page-home' ? page : null),
        libraries: [],
        sectionRow: (title, items, more) => {
            if (!items.length) return '';
            rows.push({ title, ids: items.map(i => i.Id), more: more || null });
            return `<row ${title}>`;
        },
        // user-settings.js runs here too, with no saved settings
        document: { documentElement: { classList: { toggle() {} } }, getElementById: () => null },
        window: {},
        API: {
            token: 'tok',
            userId: 'u1',
            isAdmin: false,
            isLoggedIn: () => true,
            getMe: async () => ({ Id: 'u1', Configuration: {} }),
            get: async () => ({ CustomPrefs: {} }),
            getLibraries: async () => libs,
            getUserViews: async () => libs,
            getNextUp: async () => ({ Items: [] }),
            async cachedGet(url) {
                gets.push(url);
                const answer = answers[url];
                if (answer instanceof Error) throw answer;
                return answer ?? (url.startsWith('/Items/Latest') ? [] : { Items: [] });
            },
        },
    });
    const body = [userSettings, home].join('\n').replace(/^import .*$/gm, '').replace(/^export \{[^}]*\};$/gm, '');
    vm.runInContext(`${body}\nthis.renderHome = renderHome; this.homeLatestPlan = homeLatestPlan;`, context);
    return { context, gets, rows: () => JSON.parse(JSON.stringify(rows)), page };
}

const latest = (lib, limit) => `/Items/Latest?ParentId=${lib}&Limit=${limit}&Fields=PrimaryImageAspectRatio`;

test('each library but collections and playlists gets a latest row, with Jellyfin Web\'s limits', () => {
    const { context } = sandbox();
    const plan = JSON.parse(JSON.stringify(context.homeLatestPlan(LIBS)));
    assert.deepEqual(plan.map(p => p.id), ['films', 'shows', 'tunes']);
    assert.deepEqual(plan.map(p => p.url), [latest('films', 16), latest('shows', 16), latest('tunes', 30)]);
    assert.equal(plan[2].name, 'Tunes & More');
    assert.deepEqual(JSON.parse(JSON.stringify(context.homeLatestPlan(null))), []);
    // Jellyfin's CollectionType is lowercase
    const lower = JSON.parse(JSON.stringify(context.homeLatestPlan([{ Id: 'm', Name: 'M', CollectionType: 'music' }])));
    assert.equal(lower[0].url, latest('m', 30));
});

test('the home page shows resume video, resume audio, then a latest row per library in library order', async () => {
    const s = sandbox({
        '/Items/Resume?Limit=12&MediaTypes=Video': { Items: [{ Id: 'film1', Type: 'Movie' }] },
        '/Items/Resume?Limit=12&MediaTypes=Audio': { Items: [{ Id: 'song1', Type: 'Audio' }] },
        [latest('films', 16)]: [{ Id: 'film2' }],
        [latest('shows', 16)]: [{ Id: 'show1' }],
        [latest('tunes', 30)]: [{ Id: 'album1' }, { Id: 'album2' }],
    });
    await s.context.renderHome();
    assert.deepEqual(s.rows(), [
        { title: 'home.continue_watching', ids: ['film1'], more: null },
        { title: 'home.continue_listening', ids: ['song1'], more: null },
        { title: 'home.latest_in|{"name":"Films"}', ids: ['film2'], more: { page: 'library', data: 'films' } },
        { title: 'home.latest_in|{"name":"Shows"}', ids: ['show1'], more: { page: 'library', data: 'shows' } },
        { title: 'home.latest_in|{"name":"Tunes & More"}', ids: ['album1', 'album2'], more: { page: 'library', data: 'tunes' } },
    ]);
    assert.ok(!s.gets.some(u => u.startsWith('/Items/Latest?Limit')), 'no mixed latest row');
    assert.ok(!s.gets.some(u => u.includes('ParentId=sets') || u.includes('ParentId=lists')));
    assert.ok(!s.page.innerHTML.includes('home.empty_title'));
});

test('an empty library, or one whose request fails, shows no row; the others still do', async () => {
    const s = sandbox({
        [latest('films', 16)]: new Error('boom'),
        [latest('tunes', 30)]: { Items: [{ Id: 'album1' }] },
    });
    await s.context.renderHome();
    assert.deepEqual(s.rows().map(r => r.title), ['home.latest_in|{"name":"Tunes & More"}']);
});

test('with nothing to resume and nothing added, the welcome banner shows', async () => {
    const s = sandbox();
    await s.context.renderHome();
    assert.deepEqual(s.rows(), []);
    assert.ok(s.page.innerHTML.includes('home.empty_title'));
});

test('the row titles are in English and German', () => {
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`)).home;
        assert.ok(strings.continue_listening, lang);
        assert.ok(strings.latest_in.includes('{{name}}'), lang);
    }
});
