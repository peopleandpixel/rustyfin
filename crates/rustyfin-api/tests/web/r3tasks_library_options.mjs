// R3 tasks: the library dialogs turn trickplay and chapter image extraction on, so their tasks have
// libraries to work on. Runs library-admin.js's option functions in a vm sandbox.
// Usage: node r3tasks_library_options.mjs (run by api_tests.rs r3tasks_web_library_image_options).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const source = read('modules/library-admin.js');
const OPTIONS = ['EnableTrickplay', 'ExtractTrickplayImagesDuringLibraryScan', 'EnableChapterExtraction',
    'ExtractChapterImagesDuringLibraryScan'];

// A top-level declaration, from its head to the end given
function slice(head, end = '\n}\n') {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `library-admin.js has ${head}`);
    return source.slice(start, source.indexOf(end, start) + end.length);
}

// A fake form whose checkboxes are ticked as `checked` says.
function form(checked = {}, fields = {}) {
    const elements = { libraryId: { value: 'lib-1' }, ...fields };
    for (const option of OPTIONS) elements[option] = { checked: !!checked[option] };
    return { elements, ...elements, querySelectorAll: () => [{ value: '/media/films' }] };
}

function sandbox(saved = {}) {
    const calls = [];
    const toasts = [];
    const elements = {
        'library-options-modal': { showModal() { calls.push(['showModal']); }, close() { calls.push(['close']); } },
        'library-options-form': form(),
        'library-options-title': { textContent: '' },
        'library-options-images': { innerHTML: '' },
        'library-options-submit': { disabled: false },
    };
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        $: id => elements[id] || null,
        toast: (message, kind) => toasts.push([message, kind]),
        API: {
            getLibraryOptions: async id => {
                calls.push(['get', id]);
                return { ...saved };
            },
            saveLibraryOptions: async (id, options) => calls.push(['save', id, options]),
            createLibrary: async (name, type, paths, options) => {
                calls.push(['create', name, type, paths, options]);
                return { Id: 'new' };
            },
            scanLibrary: async () => {},
        },
        setAddLibraryError: () => {},
        closeAddLibrary: () => {},
        loadSidebar: async () => {},
        currentPage: 'settings',
    });
    vm.runInContext(
        `${slice('const LIBRARY_IMAGE_OPTIONS = [', '\n];\n')}
         ${slice('function libraryImageOptionsHtml(')}
         ${slice('function readLibraryImageOptions(')}
         ${slice('async function openLibraryOptions(')}
         ${slice('function closeLibraryOptions(')}
         ${slice('async function submitLibraryOptions(')}
         ${slice('async function submitAddLibrary(')}
         this.fns = { libraryImageOptionsHtml, openLibraryOptions, submitLibraryOptions, submitAddLibrary };`,
        context,
    );
    return { context, elements, calls, toasts };
}

const settle = () => new Promise(resolve => setImmediate(resolve));

test('the dialogs offer the four image options, off unless the library has them on', () => {
    const { context } = sandbox();
    const off = context.fns.libraryImageOptionsHtml('add-lib');
    for (const option of OPTIONS) {
        assert.match(off, new RegExp(`<input type="checkbox" name="${option}" id="add-lib-${option}"  aria-describedby`));
    }
    assert.doesNotMatch(off, /checked/, 'off by default, as in Jellyfin');
    const on = context.fns.libraryImageOptionsHtml('lib-opt', { EnableTrickplay: true, EnableChapterExtraction: false });
    assert.match(on, /name="EnableTrickplay" id="lib-opt-EnableTrickplay" checked/);
    assert.doesNotMatch(on, /name="EnableChapterExtraction"[^>]*checked/);
    assert.match(on, /library\.enable_trickplay_help/);
});

test('every label and help has an English and a German text', () => {
    const keys = ['enable_trickplay', 'trickplay_during_scan', 'enable_chapter_images', 'chapter_images_during_scan'];
    for (const lang of ['en', 'de']) {
        const library = JSON.parse(read(`translations/${lang}.json`)).library;
        for (const key of [...keys, ...keys.map(k => `${k}_help`), 'options_title', 'options_button', 'options_for',
            'options_save', 'options_saved', 'options_load_failed', 'options_save_failed']) {
            assert.ok(library[key], `${lang} library.${key}`);
        }
    }
    const en = JSON.parse(read('translations/en.json')).library;
    assert.equal(en.enable_trickplay, 'Enable trickplay image extraction');
    assert.equal(en.enable_chapter_images, 'Enable chapter image extraction');
    assert.equal(en.trickplay_during_scan, 'Extract trickplay images during the library scan');
    assert.equal(en.chapter_images_during_scan, 'Extract chapter images during the library scan');
});

test('a new library is created with the image options ticked in the add dialog', async () => {
    const { context, calls } = sandbox();
    const f = form({ EnableTrickplay: true, ExtractChapterImagesDuringLibraryScan: true }, {
        name: { value: 'Films' },
        type: { value: 'Movies' },
        realtimeMonitor: { checked: true },
    });
    await context.fns.submitAddLibrary({ preventDefault() {}, target: f });
    const create = calls.find(call => call[0] === 'create');
    assert.deepEqual(JSON.parse(JSON.stringify(create[4])), {
        EnableRealtimeMonitor: true,
        EnableTrickplay: true,
        ExtractTrickplayImagesDuringLibraryScan: false,
        EnableChapterExtraction: false,
        ExtractChapterImagesDuringLibraryScan: true,
    });
});

test('the options dialog shows a library\'s saved options and saves them over the others', async () => {
    const saved = { EnableRealtimeMonitor: false, MetadataProvider: 'TheMovieDb', EnableTrickplay: false,
        EnableChapterExtraction: true };
    const { context, elements, calls, toasts } = sandbox(saved);
    await context.fns.openLibraryOptions('lib-1', 'Films');
    assert.equal(elements['library-options-form'].elements.libraryId.value, 'lib-1');
    assert.match(elements['library-options-title'].textContent, /library\.options_title\|.*Films/);
    const html = elements['library-options-images'].innerHTML;
    assert.match(html, /name="EnableChapterExtraction" id="lib-opt-EnableChapterExtraction" checked/);
    assert.doesNotMatch(html, /name="EnableTrickplay"[^>]*checked/);
    assert.ok(calls.some(call => call[0] === 'showModal'));

    const f = form({ EnableTrickplay: true, ExtractTrickplayImagesDuringLibraryScan: true, EnableChapterExtraction: true });
    await context.fns.submitLibraryOptions({ preventDefault() {}, target: f });
    await settle();
    const save = calls.find(call => call[0] === 'save');
    assert.equal(save[1], 'lib-1');
    assert.deepEqual(JSON.parse(JSON.stringify(save[2])), {
        EnableRealtimeMonitor: false,
        MetadataProvider: 'TheMovieDb',
        EnableTrickplay: true,
        ExtractTrickplayImagesDuringLibraryScan: true,
        EnableChapterExtraction: true,
        ExtractChapterImagesDuringLibraryScan: false,
    });
    assert.deepEqual(toasts, [['library.options_saved', undefined]]);
    assert.ok(calls.some(call => call[0] === 'close'));
});

test('options are saved through Jellyfin\'s route, with the library id and its whole options', async () => {
    const api = read('api.js');
    const start = api.indexOf('    saveLibraryOptions(');
    assert.ok(start >= 0, 'api.js has saveLibraryOptions');
    const method = api.slice(start, api.indexOf('\n    },\n', start) + 7);
    const posts = [];
    const API = vm.runInNewContext(`({ post(path, body) { this.posts.push([path, body]); return Promise.resolve({}); },
        ${method} })`);
    API.posts = posts;
    await API.saveLibraryOptions('lib-1', { EnableTrickplay: true });
    // Jellyfin's names for the same settings never ride along, so a stale one can't win
    await API.saveLibraryOptions('lib-1', {
        EnableTrickplay: true, EnableTrickplayImageExtraction: false,
        EnableChapterExtraction: false, EnableChapterImageExtraction: true, EnableRealtimeMonitor: false,
    });
    assert.deepEqual(JSON.parse(JSON.stringify(posts)), [
        ['/Library/VirtualFolders/LibraryOptions', { Id: 'lib-1', LibraryOptions: { EnableTrickplay: true } }],
        ['/Library/VirtualFolders/LibraryOptions', { Id: 'lib-1', LibraryOptions: {
            EnableTrickplay: true, EnableChapterExtraction: false, EnableRealtimeMonitor: false,
        } }],
    ]);
});

test('the dialog reads the stored options fresh, never from a cache', () => {
    const api = read('api.js');
    assert.match(api, /getLibraryOptions\(id\) \{ return this\.get\(`\/Library\/\$\{id\}\/Options`, \{ cache: 'no-store' \}\); \}/);
    assert.doesNotMatch(api, /getLibraryOptions\(id\) \{ return this\.cachedGet/);
});

test('the page has the dialogs, and the admin library list opens the options', () => {
    const html = read('index.html');
    assert.match(html, /<div id="add-lib-image-options"/);
    assert.match(html, /<dialog id="library-options-modal"/);
    assert.match(html, /<form id="library-options-form" onsubmit="submitLibraryOptions\(event\)"/);
    assert.match(html, /onclick="closeLibraryOptions\(\)"/);
    assert.match(read('modules/admin.js'), /data-action="library-options" data-id=/);
    assert.match(read('modules/router.js'), /'library-options': el => openLibraryOptions\(el\.dataset\.id, el\.dataset\.name\)/);
    const app = read('app.js');
    const exposed = app.slice(app.indexOf('Object.assign(window, {'));
    assert.match(exposed, /closeLibraryOptions/);
    assert.match(exposed, /submitLibraryOptions/);
});
