// R3 music: a music library has Jellyfin Web's tabs (albums, album artists, artists, songs, genres).
// Runs modules/music.js's tab functions in a vm sandbox with the DOM and API stubbed, and checks
// library.js and the translations as text. Usage: node r3music_tabs.mjs (run by api_tests.rs r3music_tabs).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const music = read('modules/music.js');
const library = read('modules/library.js');
const plain = value => JSON.parse(JSON.stringify(value));

function element() {
    return {
        html: '',
        textContent: '',
        classList: { removed: [], remove(c) { this.removed.push(c); } },
        set innerHTML(v) { this.html = v; },
        get innerHTML() { return this.html; },
        insertAdjacentHTML(_, h) { this.html += h; },
        appendChild(child) { this.html += child.html ?? ''; },
        replaceChildren() { this.cleared = true; },
        set outerHTML(v) { this.replacedWith = v; },
    };
}

// pages: the answer per request URL (a function of it); answers wait for release() when held
function sandbox(pages) {
    const log = [];
    const elements = {};
    const held = [];
    const context = vm.createContext({
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        $: id => (elements[id] ||= element()),
        toast: msg => log.push(['toast', msg]),
        document: { createDocumentFragment: () => element() },
        createCardElement: item => ({ html: `<card ${item.Id}>` }),
        setupInfiniteScroll: cb => { log.push(['scroll']); context.more = cb; },
        destroyInfiniteScroll: () => log.push(['destroy']),
        addInfiniteScrollSentinel: () => log.push(['sentinel']),
        API: {
            token: 'tok',
            cachedGet(url) {
                log.push(['get', url]);
                const answer = Promise.resolve(pages(url));
                if (!context.hold) return answer;
                return new Promise(resolve => held.push(() => resolve(answer)));
            },
        },
    });
    const body = music.replace(/^import .*$/gm, '').replace(/^export \{[^}]*\};$/gms, '');
    vm.runInContext(`${body}
        this.fns = { musicTab, rememberMusicTab, musicTabBar, musicTabRequest, renderMusicTab, MUSIC_TABS };`, context);
    return { fns: context.fns, log, elements, context, release: () => held.splice(0).forEach(r => r()) };
}

const songs = (start, count) => Array.from({ length: count }, (_, i) => ({
    Id: `s${start + i}`, Name: `Song ${start + i}`, IndexNumber: 1, Album: 'Lima Sessions', Artists: ['Kilo Band'], RunTimeTicks: 30000000,
}));

test('the tabs are Jellyfin Web\'s, in its order, and each opens its route', () => {
    const { fns } = sandbox(() => ({}));
    const tabs = ['suggestions', 'albums', 'albumartists', 'artists', 'playlists', 'songs', 'genres'];
    assert.deepEqual(plain(fns.MUSIC_TABS.map(([key]) => key)), tabs);
    const bar = fns.musicTabBar('lib1', 'songs');
    for (const key of tabs) {
        assert.ok(bar.includes(`data-action="navigate" data-page="library" data-id="lib1/${key}"`), key);
    }
    assert.match(bar, /aria-selected="true"[^>]*data-id="lib1\/songs"/);
    assert.equal((bar.match(/aria-selected="true"/g) || []).length, 1);
    // A genre's albums keep the Genres tab selected
    assert.match(fns.musicTabBar('lib1', 'genre'), /aria-selected="true"[^>]*data-id="lib1\/genres"/);
});

test('a route names its tab; without one the library opens on its last tab, else Suggestions', () => {
    const { fns } = sandbox(() => ({}));
    assert.equal(fns.musicTab('lib1', undefined), 'suggestions');
    assert.equal(fns.musicTab('lib1', 'artists'), 'artists');
    assert.equal(fns.musicTab('lib1', 'genres', 'g1'), 'genre');
    assert.equal(fns.musicTab('lib1', 'bogus'), 'suggestions');
    fns.rememberMusicTab('lib1', 'songs');
    assert.equal(fns.musicTab('lib1', undefined), 'songs');
    assert.equal(fns.musicTab('lib2', undefined), 'suggestions');
    fns.rememberMusicTab('lib1', 'genre');
    assert.equal(fns.musicTab('lib1', undefined), 'genres');
});

test('signing out forgets the tabs last shown', () => {
    const { fns, context } = sandbox(() => ({}));
    fns.rememberMusicTab('lib1', 'songs');
    assert.equal(fns.musicTab('lib1', undefined), 'songs');
    context.API.token = null;
    assert.equal(fns.musicTab('lib1', undefined), 'suggestions');
    context.API.token = 'other-user';
    assert.equal(fns.musicTab('lib1', undefined), 'suggestions');
});

test('a first page that fails shows the error, not Loading', async () => {
    const s = sandbox(() => { throw new Error('server down'); });
    await s.fns.renderMusicTab('lib1', 'Music', 'artists');
    assert.equal(s.elements['music-count'].textContent, '');
    assert.ok(s.elements['music-items'].replacedWith.includes('server down'));
});

test('each tab asks Jellyfin\'s endpoint for its list, a page at a time, within the library', () => {
    const { fns } = sandbox(() => ({}));
    const req = (tab, start, genre) => plain(fns.musicTabRequest('lib1', tab, start, genre));
    assert.deepEqual(req('albumartists', 0), { path: '/Artists/AlbumArtists', query: { ParentId: 'lib1', StartIndex: 0, Limit: 100 } });
    assert.deepEqual(req('artists', 100), { path: '/Artists', query: { ParentId: 'lib1', StartIndex: 100, Limit: 100 } });
    assert.equal(req('genres', 0).path, '/MusicGenres');
    assert.equal(req('genres', 0).query.ParentId, 'lib1');
    const song = req('songs', 0);
    assert.equal(song.path, '/Items');
    assert.deepEqual([song.query.ParentId, song.query.IncludeItemTypes, song.query.Recursive], ['lib1', 'Audio', true]);
    const genre = req('genre', 0, 'g1');
    assert.deepEqual([genre.path, genre.query.IncludeItemTypes, genre.query.GenreIds], ['/Items', 'MusicAlbum', 'g1']);
    assert.equal(fns.musicTabRequest('lib1', 'albums', 0), null);
});

test('the songs tab lists track rows that play, and loads the next page as it scrolls', async () => {
    const s = sandbox(url => {
        const start = Number(new URL(url, 'http://x').searchParams.get('StartIndex'));
        return { Items: songs(start, start === 0 ? 100 : 50), TotalRecordCount: 150 };
    });
    await s.fns.renderMusicTab('lib1', 'Music', 'songs');
    const page = s.elements['page-library'];
    assert.deepEqual(page.classList.removed, ['jf-with-rail']);
    assert.match(page.html, /aria-selected="true"[^>]*data-id="lib1\/songs"/);
    const first = s.log.find(e => e[0] === 'get')[1];
    assert.match(first, /^\/Items\?ParentId=lib1&IncludeItemTypes=Audio&Recursive=true&.*StartIndex=0&Limit=100$/);
    const rows = s.elements['music-items'].html;
    assert.equal((rows.match(/data-action="play"/g) || []).length, 100);
    assert.match(rows, /data-id="s0" data-stream-type="Audio"/);
    assert.ok(rows.includes('Kilo Band · Lima Sessions'));
    assert.equal(s.elements['music-count'].textContent, 'library.item_count|{"count":150}');
    assert.ok(s.log.some(e => e[0] === 'sentinel') && s.log.some(e => e[0] === 'scroll'));
    await s.context.more();
    assert.match(s.log.filter(e => e[0] === 'get')[1][1], /StartIndex=100&Limit=100$/);
    assert.equal((s.elements['music-items'].html.match(/data-action="play"/g) || []).length, 150);
    assert.ok(s.elements['music-more'].cleared, 'the sentinel goes once every page is in');
});

test('artists show as cards, genres as tiles that open their albums', async () => {
    const a = sandbox(() => ({ Items: [{ Id: 'kilo', Name: 'Kilo Band', Type: 'MusicArtist' }], TotalRecordCount: 1 }));
    await a.fns.renderMusicTab('lib1', 'Music', 'albumartists');
    assert.equal(a.log.find(e => e[0] === 'get')[1], '/Artists/AlbumArtists?ParentId=lib1&StartIndex=0&Limit=100');
    assert.ok(a.elements['music-items'].html.includes('<card kilo>'));
    assert.ok(!a.log.some(e => e[0] === 'scroll'), 'one page: no infinite scroll');

    const g = sandbox(() => ({ Items: [{ Id: 'g1', Name: 'Ambient', Type: 'MusicGenre' }], TotalRecordCount: 1 }));
    await g.fns.renderMusicTab('lib1', 'Music', 'genres');
    assert.match(g.elements['music-items'].html, /data-action="navigate" data-page="library" data-id="lib1\/genres\/g1"/);
    assert.ok(g.elements['music-items'].html.includes('>Ambient<'));
});

test('a genre lists its albums under its name', async () => {
    const s = sandbox(url => (url.startsWith('/MusicGenres')
        ? { Items: [{ Id: 'g0', Name: 'Pop' }, { Id: 'g1', Name: 'Ambient' }] }
        : { Items: [{ Id: 'lima', Name: 'Lima Sessions', Type: 'MusicAlbum' }], TotalRecordCount: 1 }));
    await s.fns.renderMusicTab('lib1', 'Music', 'genre', 'g1');
    await new Promise(r => setImmediate(r));
    const items = s.log.filter(e => e[0] === 'get').map(e => e[1]).find(u => u.startsWith('/Items'));
    assert.match(items, /IncludeItemTypes=MusicAlbum&Recursive=true&GenreIds=g1/);
    assert.ok(s.elements['music-items'].html.includes('<card lima>'));
    assert.equal(s.elements['music-genre'].textContent, 'Ambient');
});

test('a tab left before its list arrives draws nothing', async () => {
    const s = sandbox(url => ({ Items: [{ Id: url.includes('Album') ? 'old' : 'new', Name: 'x' }], TotalRecordCount: 1 }));
    s.context.hold = true;
    const left = s.fns.renderMusicTab('lib1', 'Music', 'albumartists');
    const shown = s.fns.renderMusicTab('lib1', 'Music', 'artists');
    s.release();
    await Promise.all([left, shown]);
    const html = s.elements['music-items'].html;
    assert.ok(html.includes('<card new>'));
    assert.ok(!html.includes('<card old>'));
});

test('the library page hands every music tab but Albums to music.js, and shows the tabs over Albums', () => {
    assert.ok(library.includes("import { musicTab, rememberMusicTab, musicTabBar, renderMusicTab } from './music.js';"));
    assert.match(library, /async function renderLibrary\(route\) \{\s*const \[libId, tab, sub\] = String\(route \|\| ''\)\.split\('\/'\);/);
    assert.match(library, /const musicView = contentType === 'Music' \? musicTab\(libId, tab, sub\) : null;/);
    assert.match(library, /if \(musicView && musicView !== 'albums'\) \{\s*stopLibraryLoad\(\);\s*\/\/[^\n]*\n\s*currentLibraryRoute = [^;]+;\s*renderMusicTab\(libId, libName, musicView, sub\);\s*return;/);
    assert.ok(library.includes("${musicView ? musicTabBar(libId, 'albums') : ''}"));
});

test('a scan refresh draws the music tab shown again, genre included, not the library\'s first view', () => {
    assert.match(library, /currentLibraryId = libId;\s*currentLibraryRoute = route;/);
    assert.ok(library.includes("currentLibraryRoute = musicView === 'genre' ? `${libId}/genres/${sub}` : `${libId}/${musicView}`;"));
    assert.match(library, /export \{\s*currentLibraryId, currentLibraryRoute,/);
    // A music tab has no grid view, so the refresh falls through to drawing the route again
    assert.match(read('modules/library-admin.js'), /if \(!libraryRefreshInPlace\(\)\) renderLibrary\(currentLibraryRoute\);/);
});

test('the tab names are in English and German', () => {
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`)).music;
        for (const key of ['tabs', 'tab_albums', 'tab_album_artists', 'tab_artists', 'tab_songs', 'tab_genres']) {
            assert.ok(strings[key], `${lang} music.${key}`);
        }
    }
});
