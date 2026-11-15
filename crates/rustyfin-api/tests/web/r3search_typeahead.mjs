// R3 search: the header search box searches as you type (after a 500 ms pause, as Jellyfin Web), and
// the results come grouped by type in Jellyfin Web's section order.
// Runs modules/search.js in a vm sandbox with the DOM, timers, history and API stubbed.
// Usage: node r3search_typeahead.mjs (run by api_tests.rs r3search_typeahead).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const search = read('modules/search.js');

class El {
    constructor(tag) { this.tag = tag; this.children = []; this.dataset = {}; this.html = ''; this.listeners = {}; this.value = ''; }
    set innerHTML(v) { this.html = v; this.children = v.includes('jf-scroll-row') ? [new El('row')] : []; }
    get innerHTML() { return this.html; }
    get lastElementChild() { return this.children.at(-1); }
    appendChild(c) { this.children.push(c); }
    append(...c) { this.children.push(...c); }
    addEventListener(type, fn) { this.listeners[type] = fn; }
}

// hints: section types -> hints; a held sandbox answers only on release()
function sandbox(hints = {}, { page: current = 'home' } = {}) {
    const log = [];
    const timers = new Map();
    let nextTimer = 1;
    const held = [];
    const elements = { 'search-input': new El('input'), 'page-search': new El('page') };
    elements['page-search'].querySelector = () => (elements['search-sections']?.children.length ? {} : null);
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml: s => String(s),
        $: id => (elements[id] ||= new El(id)),
        document: { createElement: tag => new El(tag), activeElement: null },
        createCardElement: item => Object.assign(new El('card'), { dataset: { page: 'detail', id: item.Id }, item }),
        navigate: (page, data) => log.push(['navigate', page, data]),
        history: { replaceState: (_s, _t, url) => log.push(['replace', url]) },
        setTimeout: (fn, ms) => { timers.set(nextTimer, { fn, ms }); return nextTimer++; },
        clearTimeout: id => timers.delete(id),
        currentPage: current,
        API: {
            get(url) {
                log.push(['get', url]);
                const types = new URL(url, 'http://x').searchParams.get('IncludeItemTypes');
                const answer = hints[types] instanceof Error
                    ? Promise.reject(hints[types])
                    : Promise.resolve({ SearchHints: hints[types] || [] });
                if (!context.hold) return answer;
                return new Promise(resolve => held.push(() => resolve(answer)));
            },
        },
    });
    const body = search.replace(/^import .*$/gm, '').replace(/^export \{[^}]*\};$/gm, '');
    vm.runInContext(`${body}\nthis.fns = { renderSearch, doSearch, searchAsYouType, SEARCH_SECTIONS };`, context);
    const flush = () => { const all = [...timers.values()]; timers.clear(); all.forEach(t => t.fn()); return all; };
    return { fns: context.fns, log, elements, context, timers, flush, release: () => held.splice(0).forEach(r => r()) };
}

const tick = () => new Promise(r => setImmediate(r));
const sectionsOf = s => s.elements['search-sections'].children.map(sec => [sec.dataset.searchSection, sec.lastElementChild.children.map(c => c.dataset.id)]);

test('the search box searches as you type, once the keys pause for 500 ms', () => {
    const s = sandbox();
    const input = s.elements['search-input'];
    assert.equal(typeof input.listeners.input, 'function', 'the box listens to input');
    input.listeners.input({ target: { value: 'du' } });
    input.listeners.input({ target: { value: 'dus' } });
    assert.equal(s.timers.size, 1, 'each key restarts the wait');
    assert.equal([...s.timers.values()][0].ms, 500);
    assert.deepEqual(s.log, []);
    s.flush();
    assert.deepEqual(s.log, [['navigate', 'search', 'dus']]);
});

test('on the search page, typing updates the results in place, without a history entry per term', async () => {
    const s = sandbox({}, { page: 'search' });
    s.fns.searchAsYouType('kilo band');
    s.flush();
    await tick();
    assert.deepEqual(s.log[0], ['replace', '#search/kilo%20band']);
    assert.ok(!s.log.some(e => e[0] === 'navigate'));
    assert.ok(s.log.some(e => e[0] === 'get'));
});

test('Enter searches at once, and a blank box searches nothing', () => {
    const s = sandbox();
    s.fns.searchAsYouType('dus');
    s.fns.doSearch('dusk');
    assert.equal(s.timers.size, 0, 'Enter drops the pending search');
    assert.deepEqual(s.log, [['navigate', 'search', 'dusk']]);
    s.fns.searchAsYouType('   ');
    s.flush();
    s.fns.doSearch('');
    assert.equal(s.log.length, 1);
});

test('each section asks for its own types; results show grouped in Jellyfin Web\'s order', async () => {
    const s = sandbox({
        Audio: [{ Id: 'song1', Name: 'Kilo Remix', Type: 'Audio' }],
        MusicArtist: [{ Id: 'kilo', Name: 'Kilo Band', Type: 'MusicArtist' }],
        Movie: [{ Id: 'film1', Name: 'Kilo', Type: 'Movie', PrimaryImageTag: 'tag' }],
        Person: [{ Id: 'p1', Name: 'Kilo Actor', Type: 'Person' }],
    });
    await s.fns.renderSearch('kilo');
    const urls = s.log.filter(e => e[0] === 'get').map(e => e[1]);
    assert.equal(urls.length, s.fns.SEARCH_SECTIONS.length);
    for (const url of urls) assert.match(url, /^\/Search\/Hints\?SearchTerm=kilo&IncludeItemTypes=[A-Za-z,]+&Limit=24$/);
    assert.deepEqual(JSON.parse(JSON.stringify(s.fns.SEARCH_SECTIONS.map(([key]) => key))), [
        'movies', 'shows', 'episodes', 'people', 'playlists', 'artists', 'albums', 'songs', 'videos', 'photos',
        'audiobooks', 'books', 'collections',
    ]);
    assert.deepEqual(sectionsOf(s), [['movies', ['film1']], ['people', ['p1']], ['artists', ['kilo']], ['songs', ['song1']]]);
    assert.ok(s.elements['page-search'].html.includes('search.results_for|{"term":"kilo","count":4}'));
    const people = s.elements['search-sections'].children[1];
    assert.ok(people.html.includes('search.section_people'));
    assert.equal(people.lastElementChild.children[0].dataset.page, 'person', 'a person opens the person page');
    assert.equal(s.elements['search-sections'].children[0].lastElementChild.children[0].dataset.page, 'detail');
    assert.deepEqual(JSON.parse(JSON.stringify(s.elements['search-sections'].children[0].lastElementChild.children[0].item.ImageTags)), { Primary: 'tag' });
});

test('a section whose request fails is left out, and the others still show', async () => {
    const s = sandbox({
        Movie: new Error('boom'),
        Audio: [{ Id: 'song1', Name: 'Kilo Remix', Type: 'Audio' }],
    });
    await s.fns.renderSearch('kilo');
    assert.deepEqual(sectionsOf(s), [['songs', ['song1']]]);
    assert.ok(!s.elements['page-search'].html.includes('boom'));
});

test('when every section fails, the error shows', async () => {
    const s = sandbox(Object.fromEntries(['Movie', 'Series', 'Episode', 'Person', 'Playlist', 'MusicArtist', 'MusicAlbum',
        'Audio', 'Video,MusicVideo', 'Photo', 'AudioBook', 'Book', 'BoxSet'].map(k => [k, new Error('server down')])));
    await s.fns.renderSearch('kilo');
    assert.ok(s.elements['page-search'].html.includes('server down'));
});

test('no hits in any section shows the no-results message', async () => {
    const s = sandbox();
    await s.fns.renderSearch('zzz');
    assert.ok(s.elements['page-search'].html.includes('search.no_results'));
});

test('an older search that answers late is dropped', async () => {
    const s = sandbox({ Movie: [{ Id: 'film1', Name: 'Kilo', Type: 'Movie' }] });
    s.context.hold = true;
    const older = s.fns.renderSearch('ki');
    s.context.hold = false;
    await s.fns.renderSearch('kilo');
    const after = s.elements['page-search'].html;
    s.release();
    await older;
    assert.equal(s.elements['page-search'].html, after);
    assert.ok(after.includes('"term":"kilo"'));
});

test('the section names are in English and German, and the box no longer asks for Enter', () => {
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`));
        for (const key of ['movies', 'shows', 'episodes', 'people', 'playlists', 'artists', 'albums', 'songs', 'videos',
            'photos', 'audiobooks', 'books', 'collections']) {
            assert.ok(strings.search[`section_${key}`], `${lang} search.section_${key}`);
        }
        assert.ok(!/enter/i.test(strings.nav.search_placeholder), lang);
    }
    assert.ok(!read('index.html').includes('(press Enter)'));
});
