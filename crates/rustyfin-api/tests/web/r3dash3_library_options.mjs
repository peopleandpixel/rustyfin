// R3 admin dashboard 3: the library options editor shows the server's metadata savers and, per item
// type, its metadata and image fetchers in the saved order, and saves Jellyfin's LibraryOptions:
// language, country, savers, fetchers and their order, and the library and subtitle options.
// Runs library-options.js (imports and exports stripped) in a vm sandbox with stubs.
// Usage: node r3dash3_library_options.mjs (run by api_tests.rs r3dash3_web_library_options).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const code = read('modules/library-options.js')
    .replace(/^import [\s\S]*?;\n/gm, '')
    .replace(/^export \{[\s\S]*?\};\n?/m, '');

const same = (actual, expected, message) => assert.deepEqual(JSON.parse(JSON.stringify(actual ?? null)), expected ?? null, message);
const escapeHtml = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

const AVAILABLE = {
    MetadataSavers: [{ Name: 'Nfo', DefaultEnabled: false }],
    TypeOptions: [{
        Type: 'Movie',
        MetadataFetchers: [{ Name: 'TMDb', DefaultEnabled: true }, { Name: 'OMDb', DefaultEnabled: true }, { Name: 'TheTVDB', DefaultEnabled: false }],
        ImageFetchers: [{ Name: 'TMDb', DefaultEnabled: true }, { Name: 'Fanart', DefaultEnabled: true }],
    }],
};

function sandbox(api = {}) {
    const calls = [];
    const box = { innerHTML: '' };
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml,
        icon: name => `<i data-icon="${name}"></i>`,
        $: () => box,
        document: { querySelector: () => null },
        CSS: { escape: s => s },
        API: { get: async path => { calls.push(path); return api[path.split('?')[0]]; } },
    });
    vm.runInContext(`${code}\nthis.fns = { renderLibraryOptionsEditor, readLibraryOptionsEditor, closeLibraryOptionsEditor, moveFetcher, editorClick };`, context);
    return { fns: context.fns, calls, box };
}

function form(values) {
    const elements = {};
    for (const [name, value] of Object.entries(values)) {
        elements[name] = typeof value === 'boolean' ? { checked: value } : { value: String(value) };
    }
    return { elements };
}

test('the editor loads the choices for the library\'s content type and shows its fields', async () => {
    const s = sandbox({
        '/Libraries/AvailableOptions': AVAILABLE,
        '/Localization/Cultures': [{ DisplayName: 'German', TwoLetterISOLanguageName: 'de' }],
        '/Localization/Countries': [{ DisplayName: 'Germany', TwoLetterISORegionName: 'DE' }],
    });
    await s.fns.renderLibraryOptionsEditor(s.box, { PreferredMetadataLanguage: 'de', EnableEmbeddedTitles: true }, 'movies');
    assert.ok(s.calls.includes('/Libraries/AvailableOptions?libraryContentType=movies'));
    const html = s.box.innerHTML;
    assert.match(html, /<option value="de" selected>German<\/option>/);
    assert.match(html, /name="EnableEmbeddedTitles" class="[^"]*" checked/);
    assert.match(html, /name="AutomaticallyAddToCollection"/);
    assert.doesNotMatch(html, /name="EnableEmbeddedEpisodeInfos"/, 'a TV option is not shown for movies');
    assert.match(html, /data-lo-list="meta:Movie"/);
    assert.match(html, /data-lo-list="image:Movie"/);
    assert.match(html, /library_options\.applied_hint/);
});

test('fetchers keep their saved order and ticks, and a move changes the saved order', async () => {
    const s = sandbox({ '/Libraries/AvailableOptions': AVAILABLE });
    const saved = {
        MetadataSavers: ['Nfo'],
        TypeOptions: [
            { Type: 'Movie', MetadataFetchers: ['OMDb'], MetadataFetcherOrder: ['OMDb', 'TMDb'], ImageOptions: [{ Type: 'Backdrop', Limit: 3 }] },
            { Type: 'Book', MetadataFetchers: ['Other'] },
        ],
    };
    await s.fns.renderLibraryOptionsEditor(s.box, saved, 'movies');
    // OMDb first (saved), then TMDb, then TheTVDB (new); only OMDb ticked
    s.fns.moveFetcher('meta:Movie', 2, -1);
    s.fns.editorClick({ dataset: { loToggle: 'image:Movie', index: '1' }, checked: false });
    const out = s.fns.readLibraryOptionsEditor(form({
        PreferredMetadataLanguage: '', MetadataCountryCode: 'DE', AutomaticRefreshIntervalDays: '90',
        EnableRealtimeMonitor: true, SaveLocalMetadata: false, AllowEmbeddedSubtitles: 'AllowText',
    }));
    same(out.MetadataSavers, ['Nfo']);
    assert.equal(out.PreferredMetadataLanguage, null);
    assert.equal(out.MetadataCountryCode, 'DE');
    assert.equal(out.AutomaticRefreshIntervalDays, 90);
    assert.equal(out.EnableRealtimeMonitor, true);
    assert.equal(out.AllowEmbeddedSubtitles, 'AllowText');
    same(out.TypeOptions, [
        { Type: 'Book', MetadataFetchers: ['Other'] },
        {
            Type: 'Movie', MetadataFetchers: ['OMDb'], MetadataFetcherOrder: ['OMDb', 'TheTVDB', 'TMDb'],
            ImageOptions: [{ Type: 'Backdrop', Limit: 3 }],
            ImageFetchers: ['TMDb'], ImageFetcherOrder: ['TMDb', 'Fanart'],
        },
    ]);
});

test('with nothing saved the server\'s defaults are ticked; a closed editor reads nothing', async () => {
    const s = sandbox({ '/Libraries/AvailableOptions': AVAILABLE });
    await s.fns.renderLibraryOptionsEditor(s.box, {}, 'movies');
    const out = s.fns.readLibraryOptionsEditor(form({}));
    same(out.MetadataSavers, []);
    same(out.TypeOptions[0].MetadataFetchers, ['TMDb', 'OMDb']);
    same(out.TypeOptions[0].ImageFetchers, ['TMDb', 'Fanart']);
    s.fns.closeLibraryOptionsEditor();
    same(s.fns.readLibraryOptionsEditor(form({})), {});
});

test('the options dialog renders the editor and saves its options with the image options', () => {
    const admin = read('modules/library-admin.js');
    assert.match(admin, /await renderLibraryOptionsEditor\(\$\('library-options-more'\), options, collectionType\)/);
    assert.match(admin, /\.\.\.options, \.\.\.readLibraryOptionsEditor\(form\), \.\.\.readLibraryImageOptions\(form\)/);
    assert.match(read('index.html'), /<div id="library-options-more"/);
    assert.match(read('modules/admin.js'), /data-action="library-options"[^>]*data-type="\$\{escapeHtml\(l\.CollectionType \|\| ''\)\}"/);
});
