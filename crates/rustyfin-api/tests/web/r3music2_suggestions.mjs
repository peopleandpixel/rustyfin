// The music library's Suggestions tab: first and the default tab, its four sections' requests (latest, recently
// and frequently played, random), and the drawing of the sections with the empty ones left out. Runs music.js's
// functions in a vm sandbox with stubs.
// Usage: node r3music2_suggestions.mjs (run by api_tests.rs r3music2_suggestions).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const music = read('modules/music.js');
const library = read('modules/library.js');

function between(source, head, end) {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `has ${head}`);
    const stop = source.indexOf(end, start);
    assert.ok(stop > start, `has ${end}`);
    return source.slice(start, stop);
}

// The tab list, the tab a library opens on, the suggestion requests and the section drawing of music.js
function sandbox(api) {
    const els = new Map();
    const $ = id => {
        if (!els.has(id)) els.set(id, { id, innerHTML: '', textContent: '' });
        return els.get(id);
    };
    const context = vm.createContext({
        t: key => key,
        $,
        API: api,
        URLSearchParams,
        String,
        Number,
        console,
        // a section as a marker: its title and its item count, or nothing for an empty one
        sectionRow: (title, items) => (items.length ? `<section>${title}:${items.length}</section>` : ''),
        escapeHtml: s => String(s),
        createCardElement: () => null,
        setupInfiniteScroll: () => {},
        destroyInfiniteScroll: () => {},
        addInfiniteScrollSentinel: () => {},
        playQueueAt: () => {},
        icon: () => '',
        toast: () => {},
    });
    vm.runInContext(
        `${between(music, 'const MUSIC_TABS', 'const MUSIC_PAGE_SIZE')}
         ${between(music, 'const musicTabMemory', 'function rememberMusicTab')}
         ${between(music, 'const SUGGESTION_LIMIT', '// Draws a music library tab other than')}
         this.fns = { MUSIC_TABS, musicTab, suggestionSections, fillSuggestions };`,
        context,
    );
    return { ...context.fns, $ };
}

// Arrays made inside the sandbox are another realm's, so they are compared as plain JSON
const plain = value => JSON.parse(JSON.stringify(value));
const params = url => Object.fromEntries(new URL(url, 'http://x').searchParams);

test('Suggestions is the first tab, and the tab a library opens on without a memory', () => {
    const { MUSIC_TABS, musicTab } = sandbox({ token: 't', userId: 'u1' });
    assert.deepEqual(plain(MUSIC_TABS.map(([key]) => key)),['suggestions', 'albums', 'albumartists', 'artists', 'playlists', 'songs', 'genres']);
    assert.equal(MUSIC_TABS[0][1], 'music.tab_suggestions');
    assert.equal(musicTab('lib1', undefined), 'suggestions');
    assert.equal(musicTab('lib1', 'albums'), 'albums', 'a named tab still wins');
});

test('the four suggestion requests are built on the user items, in order', () => {
    const { suggestionSections } = sandbox({ token: 't', userId: 'u1' });
    const sections = suggestionSections('lib1', 'u1');
    assert.deepEqual(plain(sections.map(s => s.key)), ['latest', 'recent', 'frequent', 'random']);
    assert.deepEqual(plain(sections.map(s => s.title)), [
        'music.suggestions_latest', 'music.suggestions_recent', 'music.suggestions_frequent', 'music.suggestions_random',
    ]);
    const [latest, recent, frequent, random] = sections.map(s => s.url);

    assert.match(latest, /^\/Users\/u1\/Items\?/);
    assert.deepEqual(params(latest), {
        ParentId: 'lib1', Recursive: 'true', Limit: '16', Fields: 'PrimaryImageAspectRatio', IncludeItemTypes: 'MusicAlbum',
        SortBy: 'DateCreated', SortOrder: 'Descending',
    });

    for (const [url, sortBy] of [[recent, 'DatePlayed'], [frequent, 'PlayCount']]) {
        assert.match(url, /^\/Users\/u1\/Items\?/);
        assert.deepEqual(params(url), {
            ParentId: 'lib1', Recursive: 'true', Limit: '16', Fields: 'PrimaryImageAspectRatio', IncludeItemTypes: 'Audio',
            Filters: 'IsPlayed', SortOrder: 'Descending', SortBy: sortBy,
        });
    }

    assert.match(random, /^\/Users\/u1\/Items\?/);
    assert.deepEqual(params(random), {
        ParentId: 'lib1', Recursive: 'true', Limit: '16', Fields: 'PrimaryImageAspectRatio', IncludeItemTypes: 'MusicAlbum',
        SortBy: 'Random',
    });
});

test('no user, no suggestions', () => {
    const { suggestionSections } = sandbox({ token: 't', userId: '' });
    assert.deepEqual(plain(suggestionSections('lib1', '')), []);
});

test('the sections are drawn in order, an empty or failed one left out', async () => {
    const answers = {
        '/Users/u1/Items?ParentId=lib1&Recursive=true&Limit=16&Fields=PrimaryImageAspectRatio&IncludeItemTypes=Audio&Filters=IsPlayed&SortOrder=Descending&SortBy=DatePlayed': { Items: [] },
    };
    const api = {
        token: 't',
        userId: 'u1',
        cachedGet: async url => {
            const hit = Object.keys(answers).find(key => url.startsWith(key));
            if (hit) return answers[hit];
            if (url.includes('SortBy=PlayCount')) return { Items: [{ Id: 'b' }, { Id: 'c' }] };
            if (url.includes('SortBy=DateCreated')) return { Items: [{ Id: 'a' }] };
            throw new Error('offline');
        },
    };
    const sb = sandbox(api);
    await sb.fillSuggestions('lib1', 0, 't');
    assert.equal(sb.$('music-items').innerHTML, '<section>music.suggestions_latest:1</section><section>music.suggestions_frequent:2</section>');
});

test('the Suggestions tab draws its sections, and the library page routes it', () => {
    assert.ok(music.includes("if (tab === 'suggestions') {\n        await fillSuggestions(libId, gen, session);"));
    assert.ok(music.includes("import { createCardElement, sectionRow } from './cards.js';"));
    assert.ok(library.includes("musicView && musicView !== 'albums'"), 'a non-Albums music tab is drawn by music.js');
});
