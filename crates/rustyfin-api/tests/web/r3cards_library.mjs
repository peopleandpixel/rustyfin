// R3 cards: the library page's sort menu per list (Jellyfin Web's per library tab), its view settings
// (poster, thumb, banner or list, and the card size) saved per library and tab, a TV library's
// Episodes tab, and the Genres tab's rows.
// Runs library.js's functions and genres.js in vm sandboxes.
// Usage: node r3cards_library.mjs (run by api_tests.rs r3cards_library).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const library = read('modules/library.js');

function slice(head, end = '\n}\n') {
    const start = library.indexOf(head);
    assert.ok(start >= 0, `library.js has ${head}`);
    return library.slice(start, library.indexOf(end, start) + end.length);
}

const plain = value => JSON.parse(JSON.stringify(value));

function sandbox() {
    const context = vm.createContext({
        t: key => key,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;'),
        cardImage: (item, landscape) => (landscape ? { tag: item.ImageTags?.Thumb } : { tag: item.ImageTags?.Primary }),
        bannerImage: item => ({ tag: item.ImageTags?.Banner }),
        libraries: [
            { Id: 'films', Name: 'Films', ContentType: 'Movies' },
            { Id: 'shows', Name: 'Shows', ContentType: 'TvShows' },
        ],
    });
    vm.runInContext(
        `${slice('const LIBRARY_SORTS = [', '\n];\n')}
         ${slice('const LIBRARY_SORT_MENUS = ', '\n};\n')}
         ${slice('function librarySorts(')}
         ${slice('function libraryList(')}
         ${slice('const LIBRARY_LAYOUTS = ', ';\n')}
         ${slice('const LIBRARY_SIZES = ', ';\n')}
         ${slice('const LAYOUT_IMAGE_TYPES = ', ';\n')}
         ${slice('const LIBRARY_PLACEHOLDER = ', ';\n')}
         ${slice('function libraryViewCustom(')}
         ${slice('const LIBRARY_FILTER_PARAMS = ', ';\n')}
         ${slice('const LIBRARY_PREFS_CLIENT = ', ';\n')}
         ${slice('function librarySort(')}
         ${slice('function libraryFilterQuery(')}
         ${slice('function libraryPrefsFromDisplay(')}
         ${slice('function libraryPrefsToDisplay(')}
         ${slice('function libraryPrefsId(')}
         ${slice('function libraryListQuery(')}
         ${slice('function libraryViewMenu(')}
         ${slice('function libraryTabBar(')}
         ${slice('function libraryShape(')}
         ${slice('function libraryCardOptions(')}
         ${slice('function libraryGridClass(')}
         ${slice('function libraryPlaceholder(')}
         ${slice('function libraryPictureTag(')}
         var currentLibraryTab = '', currentLibraryGenre = null;
         var libraryPrefs = libraryPrefsFromDisplay(null);
         this.fns = { librarySorts, libraryList, libraryPrefsFromDisplay, libraryPrefsToDisplay, libraryPrefsId,
             libraryListQuery, libraryViewMenu, libraryTabBar, libraryShape, libraryCardOptions, libraryGridClass,
             libraryPlaceholder, libraryPictureTag };
         this.set = (tab, genre, prefs) => { currentLibraryTab = tab; currentLibraryGenre = genre; if (prefs) libraryPrefs = prefs; };`,
        context,
    );
    return context;
}

const keys = sorts => plain(sorts.map(([value]) => value.split(/[,:]/)[0]));

test("each list's sort menu is Jellyfin Web's, without Random", () => {
    const { fns } = sandbox();
    assert.deepEqual(keys(fns.librarySorts('movies')),
        ['SortName', 'CommunityRating', 'DateCreated', 'DatePlayed', 'OfficialRating', 'PlayCount', 'PremiereDate', 'Runtime']);
    assert.deepEqual(keys(fns.librarySorts('series')),
        ['SortName', 'CommunityRating', 'DateCreated', 'DateLastContentAdded', 'SeriesDatePlayed', 'OfficialRating', 'PremiereDate']);
    assert.deepEqual(keys(fns.librarySorts('episodes'))[0], 'SeriesSortName');
    assert.deepEqual(keys(fns.librarySorts('albums')), ['SortName', 'CommunityRating', 'PremiereDate', 'DateCreated']);
    for (const list of ['movies', 'series', 'episodes', 'albums', 'boxsets', 'playlists', 'mixed', 'unknown']) {
        const sorts = fns.librarySorts(list);
        assert.ok(sorts.every(Boolean), list);
        assert.ok(sorts.every(([value, label]) => /:(Ascending|Descending)$/.test(value) && label.startsWith('library.sort_')), list);
        assert.ok(!sorts.some(([value]) => value.startsWith('Random')), list);
    }
    assert.equal(fns.libraryList('TvShows'), 'series');
    assert.equal(fns.libraryList('TvShows', 'episodes'), 'episodes');
    assert.equal(fns.libraryList('Movies'), 'movies');
    assert.equal(fns.libraryList('HomeVideos'), 'mixed');
});

test("a sort the list doesn't offer gives its default; the episodes list keeps its own", () => {
    const { fns } = sandbox();
    const runtime = { SortBy: 'Runtime,SortName', SortOrder: 'Ascending', CustomPrefs: {} };
    assert.equal(fns.libraryPrefsFromDisplay(runtime, 'movies').sort, 'Runtime,SortName:Ascending');
    assert.equal(fns.libraryPrefsFromDisplay(runtime, 'series').sort, 'SortName,ProductionYear:Ascending');
    const episodes = { SortBy: 'SeriesSortName,ParentIndexNumber,IndexNumber', SortOrder: 'Descending', CustomPrefs: {} };
    const prefs = fns.libraryPrefsFromDisplay(episodes, 'episodes');
    assert.equal(prefs.sort, 'SeriesSortName,ParentIndexNumber,IndexNumber:Ascending');
    assert.equal(prefs.reversed, true);
    assert.equal(fns.libraryPrefsFromDisplay(null, 'episodes').sort, 'SeriesSortName,ParentIndexNumber,IndexNumber:Ascending');
});

test('view settings save under Jellyfin names, and the defaults save nothing', () => {
    const { fns } = sandbox();
    const base = fns.libraryPrefsFromDisplay(null);
    assert.deepEqual(plain(base.view), { layout: 'poster', size: 'medium' });
    assert.deepEqual(plain(fns.libraryPrefsToDisplay('films', base).CustomPrefs), {});
    const cases = [
        [{ layout: 'thumb', size: 'small' }, { ImageType: 'Thumb', CardSize: 'small' }],
        [{ layout: 'banner', size: 'medium' }, { ImageType: 'Banner' }],
        [{ layout: 'list', size: 'large' }, { ViewMode: 'list', CardSize: 'large' }],
    ];
    for (const [view, custom] of cases) {
        const prefs = { ...base, filters: { ...base.filters, Genres: ['Drama'] }, view };
        const body = plain(fns.libraryPrefsToDisplay('films', prefs));
        assert.deepEqual(body.CustomPrefs, { Genres: 'Drama', ...custom });
        assert.deepEqual(plain(fns.libraryPrefsFromDisplay(body)), plain(prefs));
    }
    const odd = fns.libraryPrefsFromDisplay({ CustomPrefs: { ImageType: 'Logo', CardSize: 'huge' } });
    assert.deepEqual(plain(odd.view), { layout: 'poster', size: 'medium' });
});

test('the episodes tab: its own saved preferences, episodes, and landscape cards', () => {
    const context = sandbox();
    const { fns } = context;
    assert.equal(fns.libraryPrefsId('shows', ''), 'shows');
    assert.equal(fns.libraryPrefsId('shows', 'episodes'), 'shows-episodes');
    context.set('episodes', null, fns.libraryPrefsFromDisplay(null, 'episodes'));
    const query = plain(fns.libraryListQuery('shows'));
    assert.equal(query.IncludeItemTypes, 'Episode');
    assert.equal(query.SortBy, 'SeriesSortName,ParentIndexNumber,IndexNumber');
    assert.equal(fns.libraryShape(), 'backdrop');
    assert.deepEqual(plain(fns.libraryCardOptions()), { backdrop: true });
    assert.match(fns.libraryPlaceholder(), /class="jf-poster-backdrop"/);
    assert.equal(fns.libraryGridClass(), 'jf-lib-grid jf-vgrid jf-shape-backdrop jf-size-medium');
});

test("a genre's list asks for that genre", () => {
    const context = sandbox();
    context.set('', { id: 'g7', name: 'Drama' });
    const query = plain(context.fns.libraryListQuery('films'));
    assert.equal(query.IncludeItemTypes, 'Movie');
    assert.equal(query.GenreIds, 'g7');
});

test('the shape follows the view: cards, placeholders and the picture compared', () => {
    const context = sandbox();
    const { fns } = context;
    const item = { ImageTags: { Primary: 'p', Thumb: 'th', Banner: 'b' } };
    const at = layout => ({ ...fns.libraryPrefsFromDisplay(null), view: { layout, size: 'small' } });
    const cases = [
        ['poster', 'poster', { backdrop: false }, 'p', 'jf-poster'],
        ['thumb', 'backdrop', { backdrop: true }, 'th', 'jf-poster-backdrop'],
        ['banner', 'banner', { banner: true }, 'b', 'jf-poster-banner'],
        ['list', 'list', { list: true }, 'p', 'jf-list-thumb'],
    ];
    for (const [layout, shape, options, tag, box] of cases) {
        context.set('', null, at(layout));
        assert.equal(fns.libraryShape(), shape);
        assert.deepEqual(plain(fns.libraryCardOptions()), options);
        assert.equal(fns.libraryPictureTag(item), tag);
        assert.match(fns.libraryPlaceholder(), new RegExp(`class="${box}"`));
        assert.equal(fns.libraryGridClass(), `jf-lib-grid jf-vgrid jf-shape-${shape} jf-size-small`);
    }
});

test('the view menu and the tab bars', () => {
    const { fns } = sandbox();
    const menu = fns.libraryViewMenu({ layout: 'banner', size: 'large' });
    assert.match(menu, /name="lib-layout" value="banner" checked/);
    assert.match(menu, /name="lib-size" value="large" checked/);
    assert.equal((menu.match(/name="lib-layout"/g) || []).length, 4);
    const tv = fns.libraryTabBar('shows', 'TvShows', 'episodes');
    assert.deepEqual([...tv.matchAll(/data-id="([^"]+)"/g)].map(m => m[1]), ['shows', 'shows/episodes', 'shows/genres']);
    assert.match(tv, /aria-selected="true" data-action="navigate" data-page="library" data-id="shows\/episodes"/);
    const films = fns.libraryTabBar('films', 'Movies', 'main');
    assert.deepEqual([...films.matchAll(/data-id="([^"]+)"/g)].map(m => m[1]), ['films', 'films/genres']);
    assert.match(films, /library.tab_movies/);
});

test("the genres tab: a row per genre, loaded as it nears the screen, each opening that genre's list", async () => {
    const slots = new Map();
    const gets = [];
    const cached = [];
    const page = { innerHTML: '', classList: { remove() {} } };
    const slot = i => ({ innerHTML: '', dataset: { genreRow: i }, classList: { remove() {} } });
    const elements = { 'page-library': page, 'genre-rows': { set innerHTML(html) { for (const m of html.matchAll(/data-genre-row="(\d+)"/g)) slots.set(m[1], slot(m[1])); } }, 'genre-count': { textContent: '' } };
    // A stand-in observer: the test says which slots come into view
    let observer = null;
    class IntersectionObserver {
        constructor(callback, options) { this.callback = callback; this.options = options; this.watched = new Set(); observer = this; }
        observe(el) { this.watched.add(el); }
        unobserve(el) { this.watched.delete(el); }
        disconnect() { this.watched.clear(); }
        show(...ids) { this.callback(ids.map(id => ({ isIntersecting: true, target: slots.get(id) }))); }
    }
    const answer = url => {
        if (url.startsWith('/Genres?')) return { Items: [{ Id: 'g1', Name: 'Action' }, { Id: 'g2', Name: 'Drama' }, { Id: 'g3', Name: 'Empty' }] };
        const genre = new URLSearchParams(url.split('?')[1]).get('GenreIds');
        return { Items: genre === 'g3' ? [] : [{ Id: `${genre}-a` }, { Id: `${genre}-b` }] };
    };
    const context = vm.createContext({
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: () => '',
        escapeHtml: s => String(s),
        console,
        $: id => elements[id] || null,
        sectionRow: (title, items, more) => (items.length ? `row:${title}:${items.map(i => i.Id).join(',')}:${more.data}` : ''),
        IntersectionObserver,
        document: {
            querySelector: sel => slots.get(sel.match(/"(\d+)"/)[1]),
            querySelectorAll: () => [...slots.values()],
        },
        API: {
            token: 'tok',
            async cachedGet(url) { cached.push(url); return answer(url); },
            async get(url) { gets.push(url); return answer(url); },
        },
    });
    const source = read('modules/genres.js').replace(/^import [^;]*;$/gms, '').replace(/^export \{[^}]*\};$/gms, '');
    vm.runInContext(`${source}\nthis.renderGenreRows = renderGenreRows; this.genreNameOf = genreNameOf;`, context);
    const tick = () => new Promise(resolve => setImmediate(resolve));
    await context.renderGenreRows('films', 'Films', 'Movies', '<tabs>');
    assert.match(page.innerHTML, /<tabs>/);
    // Nothing asked until a row nears the screen; then only that row
    assert.equal(observer.watched.size, 3);
    assert.match(observer.options.rootMargin, /^\d+px 0px$/);
    assert.deepEqual(gets, []);
    observer.show('0', '2');
    await tick();
    assert.deepEqual(gets.map(u => new URLSearchParams(u.split('?')[1]).get('GenreIds')), ['g1', 'g3']);
    assert.equal(slots.get('1').innerHTML, '');
    observer.show('1');
    await tick();
    // The random rows are never taken from the cache; only the genre list is
    assert.equal(cached.length, 1);
    const genres = new URLSearchParams(cached[0].split('?')[1]);
    assert.deepEqual(Object.fromEntries(genres), {
        ParentId: 'films', IncludeItemTypes: 'Movie', Recursive: 'true', SortBy: 'SortName', SortOrder: 'Ascending',
    });
    const row = new URLSearchParams(gets.find(u => u.includes('GenreIds=g1')).split('?')[1]);
    assert.equal(row.get('SortBy'), 'Random');
    assert.equal(row.get('Limit'), '25');
    assert.equal(row.get('ParentId'), 'films');
    assert.deepEqual([...slots.values()].map(s => s.innerHTML), [
        'row:Action:g1-a,g1-b:films/genres/g1',
        'row:Drama:g2-a,g2-b:films/genres/g2',
        '',
    ]);
    assert.equal(elements['genre-count'].textContent, 'library.genre_count|{"count":3}');
    assert.equal(await context.genreNameOf('films', 'Movies', 'g2'), 'Drama');
    assert.equal(await context.genreNameOf('films', 'Movies', 'nope'), '');
});
