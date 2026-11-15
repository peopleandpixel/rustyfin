// W1: the library page's filter menu sends Jellyfin Web's /Items parameters, and its sort and filters
// are saved per library in the user's DisplayPreferences.
// Runs library.js's sort, filter and preference functions in a vm sandbox.
// Usage: node w1filters_library.mjs PATH/TO/app.js (run by api_tests.rs w1filters_library).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');

// A top-level declaration, from its head to the end given
function slice(head, end = '\n}\n') {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `the client has ${head}`);
    return source.slice(start, source.indexOf(end, start) + end.length);
}

function sandbox() {
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        libraries: [
            { Id: 'films', Name: 'Films', ContentType: 'Movies' },
            { Id: 'shows', Name: 'Shows', ContentType: 'TvShows' },
        ],
    });
    vm.runInContext(
        `${slice('const LIBRARY_SORTS = [', '\n];\n')}
         ${slice('const LIBRARY_FILTER_PARAMS = ', ';\n')}
         ${slice('const LIBRARY_PREFS_CLIENT = ', ';\n')}
         ${slice('function librarySort(')}
         ${slice('function libraryFilterQuery(')}
         ${slice('function libraryFilterCount(')}
         ${slice('function libraryPrefsFromDisplay(')}
         ${slice('function libraryPrefsToDisplay(')}
         ${slice('function libraryFilterMenu(')}
         ${slice('function libraryListQuery(')}
         var libraryPrefs = libraryPrefsFromDisplay(null);
         this.fns = { librarySort, libraryFilterQuery, libraryFilterCount, libraryPrefsFromDisplay,
             libraryPrefsToDisplay, libraryFilterMenu, libraryListQuery };
         this.setPrefs = prefs => { libraryPrefs = prefs; };`,
        context,
    );
    return context;
}

const plain = value => JSON.parse(JSON.stringify(value));

function prefs(filters = {}, sort = 'SortName,ProductionYear:Ascending', reversed = false) {
    return {
        sort,
        reversed,
        filters: { Filters: [], SeriesStatus: [], Genres: [], OfficialRatings: [], Years: [], ...filters },
    };
}

test('filters join as Jellyfin Web joins them, and empty ones are left out', () => {
    const { fns } = sandbox();
    const query = fns.libraryFilterQuery(prefs({
        Filters: ['IsUnplayed', 'IsFavorite'],
        Genres: ['Action', 'Drama, Romance'],
        OfficialRatings: ['PG-13', 'R'],
        Years: ['2001', '1999'],
    }).filters);
    assert.deepEqual(plain(query), {
        Filters: 'IsUnplayed,IsFavorite',
        Genres: 'Action|Drama, Romance',
        OfficialRatings: 'PG-13|R',
        Years: '2001,1999',
    });
    assert.equal(fns.libraryFilterCount(prefs({ Filters: ['IsPlayed'], Years: ['2001', '2002'] }).filters), 3);
    assert.equal(fns.libraryFilterCount(prefs().filters), 0);
});

test('the list query carries the item type, the sort and the filters', () => {
    const context = sandbox();
    context.setPrefs(prefs({ SeriesStatus: ['Continuing'], Genres: ['Drama'] },
        'DateCreated,SortName,ProductionYear:Descending', true));
    assert.deepEqual(plain(context.fns.libraryListQuery('shows')), {
        LibraryId: 'shows',
        IncludeItemTypes: 'Series',
        SortBy: 'DateCreated,SortName,ProductionYear',
        SortOrder: 'Ascending',
        Fields: 'PrimaryImageAspectRatio',
        SeriesStatus: 'Continuing',
        Genres: 'Drama',
    });
});

test('nothing saved, or a sort this client lacks, gives the defaults', () => {
    const { fns } = sandbox();
    assert.deepEqual(plain(fns.libraryPrefsFromDisplay(null)), plain(prefs()));
    // The server's default answer for a key nobody saved
    const fresh = { Id: 'u-films', SortBy: null, SortOrder: null, Client: 'rustyfin', CustomPrefs: {} };
    assert.deepEqual(plain(fns.libraryPrefsFromDisplay(fresh)), plain(prefs()));
    const other = { SortBy: 'Random', SortOrder: 'Descending', CustomPrefs: { Genres: 7 } };
    assert.deepEqual(plain(fns.libraryPrefsFromDisplay(other)), plain(prefs()));
});

test('sort and filters survive a save and a load', () => {
    const { fns } = sandbox();
    const cases = [
        prefs(),
        prefs({ Filters: ['IsResumable'], Genres: ['Sci-Fi', 'Drama, Romance'], Years: ['2020'] },
            'PremiereDate,SortName,ProductionYear:Descending'),
        prefs({ OfficialRatings: ['TV-MA'], SeriesStatus: ['Ended'] }, 'SortName,ProductionYear:Ascending', true),
        prefs({}, 'CommunityRating,SortName,ProductionYear:Descending', true),
    ];
    for (const saved of cases) {
        const body = plain(fns.libraryPrefsToDisplay('films', saved));
        assert.deepEqual(plain(fns.libraryPrefsFromDisplay(body)), plain(saved), JSON.stringify(body));
    }
});

test('the saved body has every field the server reads back, and the order it lists in', () => {
    const { fns } = sandbox();
    const body = plain(fns.libraryPrefsToDisplay('films',
        prefs({ Filters: ['IsPlayed'] }, 'SortName,ProductionYear:Ascending', true)));
    assert.deepEqual(body, {
        Id: 'films',
        Client: 'rustyfin',
        SortBy: 'SortName,ProductionYear',
        SortOrder: 'Descending',
        CustomPrefs: { Filters: 'IsPlayed' },
        RememberIndexing: false,
        RememberSorting: true,
        PrimaryImageHeight: 0,
        PrimaryImageWidth: 0,
        ShowBackdrop: true,
        ShowSidebar: false,
        ScrollDirection: 'Horizontal',
    });
});

test('an empty list offers the reset only while a filter is on', () => {
    const context = vm.createContext({
        t: key => key,
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;'),
    });
    vm.runInContext(
        `${slice('const LIBRARY_FILTER_PARAMS = ', ';\n')}
         ${slice('function libraryFilterCount(')}
         ${slice('function libraryEmptyHtml(')}
         var currentLibraryId = 'films', libraryPrefs;
         this.empty = filters => { libraryPrefs = { filters }; return libraryEmptyHtml(); };`,
        context,
    );
    const plainEmpty = context.empty({ Genres: [] });
    assert.match(plainEmpty, /library\.empty/);
    assert.ok(!plainEmpty.includes('clear-library-filters'), plainEmpty);
    const filtered = context.empty({ Genres: ['Drama'] });
    assert.match(filtered, /library\.no_filter_results/);
    assert.match(filtered, /data-action="clear-library-filters" data-id="films"/);
    // Focusable, so the grid can move focus to it when the list empties
    assert.match(plainEmpty, /tabindex="-1"/);
    assert.match(filtered, /tabindex="-1"/);
});

// The checkboxes of a menu: [param, value, checked]
function boxes(html) {
    return [...html.matchAll(/<input type="checkbox" data-param="([^"]+)" value="([^"]*)"( checked)?>/g)]
        .map(([, param, value, checked]) => [param, value, !!checked]);
}

test('the menu offers the status filters, the library values and series status for shows only', () => {
    const { fns } = sandbox();
    const lists = { Genres: ['Drama', 'Action'], Years: [2001, 2010], OfficialRatings: ['R'] };
    const movies = boxes(fns.libraryFilterMenu('Movies', lists, prefs({ Filters: ['IsFavorite'] }).filters));
    assert.deepEqual(movies, [
        ['Filters', 'IsUnplayed', false], ['Filters', 'IsPlayed', false],
        ['Filters', 'IsResumable', false], ['Filters', 'IsFavorite', true],
        ['Genres', 'Action', false], ['Genres', 'Drama', false],
        ['OfficialRatings', 'R', false],
        ['Years', '2010', false], ['Years', '2001', false],
    ]);
    const shows = boxes(fns.libraryFilterMenu('TvShows', {}, prefs({ SeriesStatus: ['Ended'] }).filters));
    assert.deepEqual(shows.filter(([param]) => param === 'SeriesStatus'),
        [['SeriesStatus', 'Continuing', false], ['SeriesStatus', 'Ended', true]]);
    // No values, no group
    const html = fns.libraryFilterMenu('Movies', {}, prefs().filters);
    assert.ok(!html.includes('library.filter_genres'), html);
});

test('a saved value the server no longer lists stays checked, and values are escaped', () => {
    const { fns } = sandbox();
    const filters = prefs({ Genres: ['Gone "Genre"'], Years: ['1999'] }).filters;
    const menu = fns.libraryFilterMenu('Movies', { Genres: ['<b>Drama</b>'], Years: [2001] }, filters);
    assert.ok(!menu.includes('<b>'), menu);
    const found = boxes(menu).filter(([param]) => param !== 'Filters');
    assert.deepEqual(found, [
        ['Genres', '&lt;b&gt;Drama&lt;/b&gt;', false], ['Genres', 'Gone &quot;Genre&quot;', true],
        ['Years', '2001', false], ['Years', '1999', true],
    ]);
});

test('preference saves go one at a time, in order, so the last one is kept', async () => {
    const pending = [];
    const sent = [];
    const toasts = [];
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        toast: (msg, type) => toasts.push([msg, type]),
        API: {
            userId: 'u1',
            saveDisplayPreferences: (id, client, body) => {
                sent.push([id, client, body.SortBy]);
                return new Promise((resolve, reject) => pending.push({ resolve, reject }));
            },
        },
    });
    vm.runInContext(
        `${slice('const LIBRARY_SORTS = [', '\n];\n')}
         ${slice('const LIBRARY_FILTER_PARAMS = ', ';\n')}
         ${slice('const LIBRARY_PREFS_CLIENT = ', ';\n')}
         ${slice('function librarySort(')}
         ${slice('function libraryFilterQuery(')}
         ${slice('function libraryPrefsToDisplay(')}
         ${slice('let librarySaves = ', ';\n')}
         ${slice('function saveLibraryPrefs(')}
         const libraryPrefsCache = new Map();
         var libraryPrefs;
         this.save = sort => { libraryPrefs = { sort, reversed: false, filters: {} }; saveLibraryPrefs('films'); };
         this.cached = key => libraryPrefsCache.get(key);`,
        context,
    );
    const tick = () => new Promise(resolve => setImmediate(resolve));
    context.save('SortName,ProductionYear:Ascending');
    context.save('PremiereDate,SortName,ProductionYear:Descending');
    context.save('DateCreated,SortName,ProductionYear:Descending');
    await tick();
    // Only the first is on its way; the session copy already has the last.
    assert.deepEqual(sent, [['films', 'rustyfin', 'SortName,ProductionYear']]);
    assert.equal(context.cached('u1:films').sort, 'DateCreated,SortName,ProductionYear:Descending');
    // A failed save is reported and the next one still goes.
    pending[0].reject(new Error('offline'));
    await tick();
    assert.equal(sent.length, 2);
    assert.deepEqual(toasts, [['library.prefs_save_failed|{"message":"offline"}', 'error']]);
    pending[1].resolve({});
    await tick();
    assert.deepEqual(sent.map(([, , sortBy]) => sortBy), [
        'SortName,ProductionYear', 'PremiereDate,SortName,ProductionYear', 'DateCreated,SortName,ProductionYear',
    ]);
    pending[2].resolve({});
});
