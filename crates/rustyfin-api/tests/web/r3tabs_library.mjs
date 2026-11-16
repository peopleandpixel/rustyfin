// R3 library tabs: a movie library's Movies, Suggestions, Favorites, Collections and Genres tabs and a
// TV library's Shows, Suggestions, Upcoming, Genres, Networks and Episodes, as Jellyfin Web's; a
// folder in a library of mixed content opens in place with breadcrumbs; and the empty search shows
// suggestions. Runs library.js's functions, libtabs.js and search.js in vm sandboxes.
// Usage: node r3tabs_library.mjs (run by api_tests.rs r3tabs_library).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const library = read('modules/library.js');
const plain = value => JSON.parse(JSON.stringify(value));
const tick = () => new Promise(r => setImmediate(r));
const strip = source => source.replace(/^import .*$/gm, '').replace(/^export \{[^}]*\};$/gms, '');
const escapeHtml = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;');
const t = (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key);

function slice(head, end = '\n}\n') {
    const start = library.indexOf(head);
    assert.ok(start >= 0, `library.js has ${head}`);
    return library.slice(start, library.indexOf(end, start) + end.length);
}

// library.js's tab, query, breadcrumb and card functions
function librarySandbox() {
    const cards = [];
    const context = vm.createContext({
        t, escapeHtml,
        libraries: [
            { Id: 'films', ContentType: 'Movies' },
            { Id: 'shows', ContentType: 'TvShows' },
            { Id: 'home', ContentType: 'HomeVideos' },
            { Id: 'mix', ContentType: '' },
        ],
        createCardElement: item => {
            const card = { item, dataset: { page: 'detail', id: item.Id } };
            cards.push(card);
            return card;
        },
        libraryCardOptions: () => ({}),
        libraryFilterQuery: filters => (filters.Filters?.length ? { Filters: filters.Filters.join(',') } : {}),
        librarySort: () => ({ sortBy: 'SortName', sortOrder: 'Ascending' }),
        studioNameOf: async () => 'Kilo TV',
        API: {
            getItem: async id => ({ Id: id, Name: 'Party' }),
            get: async () => [
                { Id: 'year', Name: '2019', Type: 'Folder' },
                { Id: 'home', Name: 'Home', Type: 'CollectionFolder' },
            ],
        },
    });
    vm.runInContext(
        `${slice('function libraryList(')}
         ${slice('function libraryTabs(')}
         ${slice('function libraryTabBar(')}
         ${slice('function libraryListQuery(')}
         ${slice('async function loadLibraryScope(')}
         ${slice('function folderCrumbs(')}
         ${slice('function libraryCard(')}
         var currentLibraryId = null, currentLibraryTab = '', currentLibraryGenre = null, currentLibraryScope = null;
         var libraryPrefs = { filters: { Filters: [] } };
         this.fns = { libraryTabs, libraryTabBar, libraryListQuery, loadLibraryScope, folderCrumbs, libraryCard };
         this.set = (id, tab, scope, filters) => {
             currentLibraryId = id; currentLibraryTab = tab; currentLibraryScope = scope;
             libraryPrefs = { filters: { Filters: filters || [] } };
         };`,
        context,
    );
    return { fns: context.fns, set: context.set, cards };
}

test("the tabs are Jellyfin Web's, in its order, each kept in the route", () => {
    const { fns } = librarySandbox();
    assert.deepEqual(plain(fns.libraryTabs('Movies').map(([key]) => key)), ['main', 'suggestions', 'favorites', 'collections', 'genres']);
    assert.deepEqual(plain(fns.libraryTabs('TvShows').map(([key]) => key)),
        ['main', 'suggestions', 'upcoming', 'genres', 'studios', 'episodes']);
    const bar = fns.libraryTabBar('shows', 'TvShows', 'upcoming');
    assert.match(bar, /aria-selected="true" data-action="navigate" data-page="library" data-id="shows\/upcoming">library.tab_upcoming</);
    assert.match(bar, /data-id="shows\/studios">library.tab_networks</);
    assert.equal((bar.match(/aria-selected="true"/g) || []).length, 1);
    // renderLibrary reads the route's tab against this list, so a tab survives back and refresh
    assert.match(library, /const known = videoTabs && libraryTabs\(contentType\)\.some\(\(\[key\]\) => key === tab\);/);
});

test('the Favorites tab lists favourites with the chosen filters; a network and a folder narrow the list', () => {
    const { fns, set } = librarySandbox();
    set('films', 'favorites', null, ['IsUnplayed']);
    const favorites = plain(fns.libraryListQuery('films'));
    assert.equal(favorites.Filters, 'IsUnplayed,IsFavorite');
    assert.equal(favorites.IncludeItemTypes, 'Movie');
    set('films', 'favorites', null, []);
    assert.equal(fns.libraryListQuery('films').Filters, 'IsFavorite');
    set('films', '', null, []);
    assert.equal(fns.libraryListQuery('films').Filters, undefined, 'the Movies tab is not limited to favourites');
    set('shows', '', { StudioIds: 'st1', name: '' });
    const network = plain(fns.libraryListQuery('shows'));
    assert.equal(network.StudioIds, 'st1');
    assert.equal(network.IncludeItemTypes, 'Series');
    set('home', '', { ParentId: 'party', name: '', crumbs: [] });
    const folder = plain(fns.libraryListQuery('home'));
    assert.equal(folder.ParentId, 'party');
    assert.equal(folder.LibraryId, 'home');
});

test('a folder in a mixed library opens in place, under breadcrumbs from the library', async () => {
    const { fns, set } = librarySandbox();
    set('home', '', null);
    const folder = fns.libraryCard({ Id: 'party', Type: 'Folder' });
    assert.deepEqual(plain(folder.dataset), { page: 'library', id: 'home/folder/party' });
    assert.deepEqual(plain(fns.libraryCard({ Id: 'clip', Type: 'Video' }).dataset), { page: 'detail', id: 'clip' });
    set('mix', '', null);
    assert.equal(fns.libraryCard({ Id: 'f2', Type: 'Folder' }).dataset.page, 'library', 'a library with no content type is mixed');
    set('films', '', null);
    assert.equal(fns.libraryCard({ Id: 'f3', Type: 'Folder' }).dataset.page, 'detail', 'only a mixed library browses folders');

    const scope = { ParentId: 'party', name: '', crumbs: [] };
    await fns.loadLibraryScope('home', scope);
    assert.deepEqual(plain(scope), { ParentId: 'party', name: 'Party', crumbs: [{ id: 'year', name: '2019' }] });
    const crumbs = fns.folderCrumbs('home', 'Home Videos', scope);
    assert.deepEqual([...crumbs.matchAll(/data-id="([^"]+)">([^<]+)</g)].map(m => [m[1], m[2]]),
        [['home', 'Home Videos'], ['home/folder/year', '2019']]);
    assert.match(crumbs, /<span class="jf-crumb" aria-current="page">Party<\/span>/);
    const network = { StudioIds: 's1', name: '' };
    await fns.loadLibraryScope('shows', network);
    assert.equal(network.name, 'Kilo TV');
});

// libtabs.js with the DOM and API stubbed; answers maps a URL's path to its answer
function tabsSandbox(answers) {
    const elements = {};
    const gets = [];
    const element = () => ({ innerHTML: '', textContent: '', classList: { remove() {} } });
    const context = vm.createContext({
        URLSearchParams, Date, Math, Number, Promise, Array, Map,
        t, escapeHtml,
        icon: name => `<svg data-icon="${name}"></svg>`,
        $: id => (elements[id] ||= element()),
        sectionRow: (title, items, more, options) => (items.length ? `<row title="${escapeHtml(title)}" ids="${items.map(i => i.Id)}" backdrop="${!!options?.backdrop}">` : ''),
        API: {
            token: 'tok',
            get: async url => {
                gets.push(url);
                const answer = answers[url.split('?')[0]];
                if (answer instanceof Error) throw answer;
                return answer ?? {};
            },
            cachedGet: async url => {
                gets.push(url);
                return answers[url.split('?')[0]] ?? {};
            },
        },
    });
    vm.runInContext(`${strip(read('modules/libtabs.js'))}
        this.fns = { suggestionRequests, recommendationsUrl, renderSuggestions, airDayTitle, upcomingGroups, renderUpcoming, renderStudios };`, context);
    return { fns: context.fns, elements, gets };
}

const query = url => Object.fromEntries(new URL(url, 'http://x').searchParams);

test("Suggestions: Continue Watching, the latest, Next Up for TV, and a movie library's recommendation rows", async () => {
    const s = tabsSandbox({
        '/UserItems/Resume': { Items: [{ Id: 'r1' }] },
        '/Items/Latest': [{ Id: 'l1' }, { Id: 'l2' }],
        '/Shows/NextUp': { Items: [] },
        '/Movies/Recommendations': [
            { RecommendationType: 'SimilarToRecentlyPlayed', BaselineItemName: 'Alpha', Items: [{ Id: 'a1' }] },
            { RecommendationType: 'SimilarToLikedItem', BaselineItemName: 'Bravo', Items: [{ Id: 'b1' }] },
        ],
    });
    const tv = plain(s.fns.suggestionRequests('shows', 'TvShows'));
    assert.deepEqual(tv.map(([title]) => title), ['home.continue_watching', 'library.latest_episodes', 'home.next_up']);
    assert.deepEqual(query(tv[0][1]), {
        ParentId: 'shows', IncludeItemTypes: 'Episode', Recursive: 'true', Limit: '12', Fields: 'PrimaryImageAspectRatio', EnableTotalRecordCount: 'false',
    });
    assert.equal(query(tv[2][1]).ParentId, 'shows');
    assert.deepEqual(query(s.fns.recommendationsUrl('films')), {
        ParentId: 'films', CategoryLimit: '6', ItemLimit: '8', Fields: 'PrimaryImageAspectRatio',
    });

    await s.fns.renderSuggestions('films', 'Films', 'Movies', '<nav>');
    await tick();
    assert.ok(s.gets.some(url => url.startsWith('/Movies/Recommendations?')));
    const rows = [...s.elements['suggestion-rows'].innerHTML.matchAll(/<row title="([^"]+)" ids="([^"]*)"/g)].map(m => [m[1], m[2]]);
    assert.deepEqual(rows, [
        ['home.continue_watching', 'r1'],
        ['library.latest_movies', 'l1,l2'],
        ['library.because_you_watched|{&quot;name&quot;:&quot;Alpha&quot;}', 'a1'],
        ['library.because_you_like|{&quot;name&quot;:&quot;Bravo&quot;}', 'b1'],
    ]);
});

test('Suggestions with nothing to show says so; a TV library asks no recommendations', async () => {
    const s = tabsSandbox({});
    await s.fns.renderSuggestions('shows', 'Shows', 'TvShows', '<nav>');
    await tick();
    assert.ok(!s.gets.some(url => url.startsWith('/Movies/Recommendations')));
    assert.match(s.elements['suggestion-rows'].innerHTML, /library.suggestions_empty/);
});

test('Upcoming: the library\'s episodes still to air, in a row per day', async () => {
    const now = new Date(2026, 10, 16, 12);
    const s = tabsSandbox({});
    assert.equal(s.fns.airDayTitle(new Date(2026, 10, 16, 21), now), 'library.today');
    assert.equal(s.fns.airDayTitle(new Date(2026, 10, 17, 1), now), 'library.tomorrow');
    assert.doesNotMatch(s.fns.airDayTitle(new Date(2026, 10, 20), now), /library\./);
    const groups = plain(s.fns.upcomingGroups([
        { Id: 'e1', PremiereDate: new Date(2026, 10, 16, 20).toISOString() },
        { Id: 'e2', PremiereDate: new Date(2026, 10, 17, 20).toISOString() },
        { Id: 'e3', PremiereDate: new Date(2026, 10, 17, 21).toISOString() },
        { Id: 'e4' },
    ], now));
    assert.deepEqual(groups.map(([title, items]) => [title, items.map(i => i.Id)]),
        [['library.today', ['e1']], ['library.tomorrow', ['e2', 'e3']], ['library.date_unknown', ['e4']]]);

    // A date-only premiere, or midnight UTC, is that calendar day wherever the client is
    const dated = plain(s.fns.upcomingGroups([
        { Id: 'd1', PremiereDate: '2026-11-17' },
        { Id: 'd2', PremiereDate: '2026-11-17T00:00:00.0000000Z' },
        { Id: 'd3', PremiereDate: '2026-11-16T00:00:00+00:00' },
    ], now));
    assert.deepEqual(dated.map(([title, items]) => [title, items.map(i => i.Id)]),
        [['library.tomorrow', ['d1', 'd2']], ['library.today', ['d3']]]);

    const tab = tabsSandbox({ '/Shows/Upcoming': { Items: [] } });
    await tab.fns.renderUpcoming('shows', 'Shows', 'TvShows', '<nav>');
    assert.equal(query(tab.gets[0]).ParentId, 'shows');
    assert.match(tab.elements['upcoming-rows'].innerHTML, /library.upcoming_empty/);
});

test("Networks: a card per network of the library's shows, each opening that network's list", async () => {
    const s = tabsSandbox({ '/Studios': { Items: [{ Id: 'st1', Name: 'Kilo TV' }, { Id: 'st2', Name: 'Lima', ImageTags: { Thumb: 'th' } }] } });
    await s.fns.renderStudios('shows', 'Shows', 'TvShows', '<nav>');
    assert.deepEqual(query(s.gets[0]), {
        ParentId: 'shows', IncludeItemTypes: 'Series', Recursive: 'true', SortBy: 'SortName', SortOrder: 'Ascending',
        Fields: 'PrimaryImageAspectRatio', EnableTotalRecordCount: 'false',
    });
    const grid = s.elements['studio-grid'].innerHTML;
    assert.deepEqual([...grid.matchAll(/data-page="library" data-id="([^"]+)"/g)].map(m => m[1]), ['shows/studios/st1', 'shows/studios/st2']);
    assert.match(grid, /\/Items\/st2\/Images\/Thumb\?fillWidth=480&amp;quality=90&amp;tag=th/);
    assert.match(s.elements['libtab-status'].textContent, /library.studio_count\|\{"count":2\}/);
});

// search.js with the DOM stubbed
function searchSandbox(answer, current = 'home') {
    const log = [];
    const listeners = {};
    const elements = {
        'search-input': { value: '', addEventListener: (type, fn) => { listeners[type] = fn; } },
        'page-search': { innerHTML: 'old results' },
    };
    const context = vm.createContext({
        t, escapeHtml, console: { warn() {} },
        $: id => elements[id],
        document: { activeElement: null },
        navigate: (page, data) => log.push(['navigate', page, data]),
        history: { replaceState: (_s, _t, url) => log.push(['replace', url]) },
        setTimeout, clearTimeout,
        currentPage: current,
        API: {
            get: async url => {
                log.push(['get', url]);
                if (answer instanceof Error) throw answer;
                return answer;
            },
        },
    });
    vm.runInContext(`${strip(read('modules/search.js'))}\nthis.fns = { renderSearch, doSearch };`, context);
    return { fns: context.fns, log, elements, listeners };
}

test('the empty search shows suggestions: favourite or liked titles first, then random ones, as links', async () => {
    const s = searchSandbox({ Items: [{ Id: 'm1', Name: 'Alpha' }, { Id: 's1', Name: 'Bravo <Show>' }] }, 'search');
    await s.fns.renderSearch('');
    const [[, url]] = s.log;
    assert.deepEqual(query(url), {
        SortBy: 'IsFavoriteOrLiked,Random', IncludeItemTypes: 'Movie,Series,MusicArtist', Limit: '20', Recursive: 'true',
        ImageTypeLimit: '0', EnableImages: 'false', EnableTotalRecordCount: 'false',
    });
    assert.ok(url.startsWith('/Items?'));
    const page = s.elements['page-search'].innerHTML;
    assert.match(page, /search.suggestions/);
    assert.deepEqual([...page.matchAll(/data-page="detail" data-id="([^"]+)">([^<]*)</g)].map(m => [m[1], m[2]]),
        [['m1', 'Alpha'], ['s1', 'Bravo &lt;Show>']]);

    // Emptying the box on the search page shows them again; elsewhere an empty Enter does nothing
    s.fns.doSearch('  ');
    assert.deepEqual(s.log.at(-1)[0], 'get');
    assert.ok(s.log.some(([kind, value]) => kind === 'replace' && value === '#search'));
    const away = searchSandbox({ Items: [] });
    away.fns.doSearch('');
    assert.deepEqual(away.log, []);
});

test('no suggestions, or a failed request, leaves the page blank; the empty box opens the search page', async () => {
    for (const answer of [{ Items: [] }, new Error('down')]) {
        const s = searchSandbox(answer, 'search');
        await s.fns.renderSearch('');
        assert.equal(s.elements['page-search'].innerHTML, '');
    }
    const s = searchSandbox({ Items: [] });
    s.listeners.focus({ target: { value: '' } });
    assert.deepEqual(s.log, [['navigate', 'search', '']]);
    s.listeners.focus({ target: { value: 'du' } });
    assert.equal(s.log.length, 1, 'a box with a term keeps the page');
});
