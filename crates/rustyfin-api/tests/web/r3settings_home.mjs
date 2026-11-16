// R3 settings: the home screen follows the user's home settings, as Jellyfin Web's: the sections in
// the order of the homesection0..n user settings ('none' hides one), and no "Recently Added" row for
// a library in UserConfiguration.LatestItemsExcludes. Runs modules/user-settings.js and
// modules/home.js's renderHome in a vm sandbox with the API and the card helpers stubbed.
// Usage: node r3settings_home.mjs (run by api_tests.rs r3settings_home).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const source = [read('modules/playerprefs.js'), read('modules/user-settings.js'), read('modules/home.js')].join('\n')
    .replace(/^import .*$/gm, '')
    .replace(/^export \{[^}]*\};$/gm, '');

const LIBS = [
    { Id: 'aaaa-1111', Name: 'Films', ContentType: 'Movies' },
    { Id: 'bbbb-2222', Name: 'Shows', ContentType: 'TvShows' },
];
const latest = lib => `/Items/Latest?ParentId=${lib}&Limit=16&Fields=PrimaryImageAspectRatio`;
const ANSWERS = {
    '/Items/Resume?Limit=12&MediaTypes=Video': { Items: [{ Id: 'film1', Type: 'Movie' }] },
    '/Items/Resume?Limit=12&MediaTypes=Audio': { Items: [{ Id: 'song1', Type: 'Audio' }] },
    [latest('aaaa-1111')]: [{ Id: 'film2' }],
    [latest('bbbb-2222')]: [{ Id: 'show1' }],
};

function sandbox({ config = {}, prefs = {} } = {}) {
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
        serverVersion: '0',
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
            isAdmin: false,
            isLoggedIn: () => true,
            getLibraries: async () => LIBS,
            getNextUp: async () => ({ Items: [{ Id: 'ep1', Type: 'Episode' }] }),
            getMe: async () => ({ Id: 'u1', Configuration: config }),
            get: async url => {
                gets.push(url);
                return { Id: 'usersettings', CustomPrefs: prefs };
            },
            async cachedGet(url) {
                gets.push(url);
                return ANSWERS[url] ?? (url.startsWith('/Items/Latest') ? [] : { Items: [] });
            },
        },
    });
    vm.runInContext(`${source}\nthis.renderHome = renderHome; this.homeSections = homeSections; this.homeSlotValues = homeSlotValues; this.showsInLatest = showsInLatest;`, context);
    return { context, gets, rows: () => [...rows], page };
}

test('the sections default to Jellyfin Web\'s order; a slot set to none hides that section', () => {
    const { context } = sandbox();
    const sections = prefs => [...context.homeSections(name => prefs[name] ?? null)];
    assert.deepEqual(sections({}), ['smalllibrarytiles', 'resume', 'resumeaudio', 'nextup', 'latestmedia']);
    assert.deepEqual(
        sections({ homesection0: 'latestmedia', homesection1: 'nextup', homesection2: 'none', homesection4: 'resume' }),
        ['latestmedia', 'nextup', 'resume'],
        'a section set twice shows once, where it is set first',
    );
    assert.deepEqual(sections({ homesection0: 'livetv' }), ['resume', 'resumeaudio', 'nextup', 'latestmedia'], 'unknown sections are left out');
});

test('slots saved by Jellyfin Web read as Jellyfin Web reads them: ten slots, its defaults, its ids skipped', () => {
    const { context } = sandbox();
    const sections = prefs => [...context.homeSections(name => prefs[name] ?? null)];
    // What Jellyfin Web's home screen settings save untouched: every slot, with its defaults
    const jellyfinDefaults = {
        homesection0: 'smalllibrarytiles', homesection1: 'resume', homesection2: 'resumeaudio', homesection3: 'resumebook',
        homesection4: 'livetv', homesection5: 'nextup', homesection6: 'latestmedia', homesection7: 'none',
        homesection8: 'none', homesection9: 'none',
    };
    assert.deepEqual(sections(jellyfinDefaults), ['smalllibrarytiles', 'resume', 'resumeaudio', 'nextup', 'latestmedia']);
    // Next Up moved to the top and Continue Listening turned off in Jellyfin Web
    assert.deepEqual(
        sections({ ...jellyfinDefaults, homesection0: 'nextup', homesection2: 'none', homesection5: 'smalllibrarytiles' }),
        ['nextup', 'resume', 'smalllibrarytiles', 'latestmedia'],
    );
    // A saved slot means the user chose: a section no slot names stays hidden
    assert.deepEqual(sections({ homesection0: 'latestmedia', homesection1: 'none', homesection2: 'none', homesection5: 'none' }), ['latestmedia']);
    assert.deepEqual(sections({ homesection0: 'folders' }), ['smalllibrarytiles', 'resume', 'resumeaudio', 'nextup', 'latestmedia']);
    // Writing one slot writes all ten, keeping the ones Rustyfin doesn't show
    const written = JSON.parse(JSON.stringify(context.homeSlotValues(2, 'none', name => jellyfinDefaults[name] ?? null)));
    assert.deepEqual(written, { ...jellyfinDefaults, homesection2: 'none' });
});

test('the home page shows its sections in the user\'s order and leaves out the hidden ones', async () => {
    const s = sandbox({ prefs: { homesection0: 'latestmedia', homesection1: 'nextup', homesection2: 'none', homesection3: 'resume', homesection4: 'none' } });
    await s.context.renderHome();
    assert.deepEqual(s.rows(), [
        'home.latest_in|{"name":"Films"}',
        'home.latest_in|{"name":"Shows"}',
        'home.next_up',
        'home.continue_watching',
    ]);
    assert.ok(!s.page.innerHTML.includes('home.my_media'), 'the library tiles are hidden');
    assert.ok(s.gets.includes('/DisplayPreferences/usersettings?userId=u1&client=emby'));
});

test('a library left out of Recently Added gets no row and no request; ids match without case or hyphens', async () => {
    const s = sandbox({ config: { LatestItemsExcludes: ['AAAA1111'] } });
    await s.context.renderHome();
    assert.deepEqual(s.rows(), [
        'home.continue_watching',
        'home.continue_listening',
        'home.next_up',
        'home.latest_in|{"name":"Shows"}',
    ]);
    assert.ok(!s.gets.includes(latest('aaaa-1111')));
    assert.ok(s.page.innerHTML.includes('home.my_media'), 'the library tiles show by default');
    assert.equal(s.context.showsInLatest('bbbb-2222', { LatestItemsExcludes: ['aaaa-1111'] }), true);
});
