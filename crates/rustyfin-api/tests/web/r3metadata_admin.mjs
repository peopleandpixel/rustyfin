// The web client's admin metadata tools: the refresh dialog's query per mode, the metadata editor's
// save body and locks, the Identify and remote image markup, the item page's buttons, and their
// wiring, icons and translations.
// Runs metadata-admin.js's helpers in a vm sandbox and checks the rest as text.
// Usage: node r3metadata_admin.mjs (run by api_tests.rs r3metadata_web).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const source = read('modules/metadata-admin.js');
const item = read('modules/item.js');
const router = read('modules/router.js');
const api = read('api.js');
const icons = read('icons.js');
const app = read('app.js');
const en = JSON.parse(read('translations/en.json'));
const de = JSON.parse(read('translations/de.json'));

// A top-level declaration of `text`, from its head to the end given
function slice(text, head, end = '\n}\n') {
    const start = text.indexOf(head);
    assert.ok(start >= 0, `has ${head}`);
    return text.slice(start, text.indexOf(end, start) + end.length);
}

const context = vm.createContext({
    t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
    escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
    icon: name => `<i:${name}>`,
});
vm.runInContext(
    `${slice(source, 'const LOCKABLE_FIELDS = ', ';\n')}
     ${slice(source, 'function refreshQuery(')}
     ${slice(source, 'function splitList(')}
     ${slice(source, 'function itemUpdateBody(')}
     ${slice(source, 'function searchResultHtml(')}
     ${slice(source, 'function remoteImageHtml(')}
     ${slice(item, 'function metadataToolButtons(')}
     this.fns = { refreshQuery, splitList, itemUpdateBody, searchResultHtml, remoteImageHtml, metadataToolButtons };`,
    context,
);
const fns = context.fns;
const plain = value => JSON.parse(JSON.stringify(value));

test('each refresh mode sends what Jellyfin Web sends', () => {
    assert.deepEqual(plain(fns.refreshQuery('scan', true)), {
        MetadataRefreshMode: 'Default', ImageRefreshMode: 'Default', ReplaceAllMetadata: false, ReplaceAllImages: false,
    });
    assert.deepEqual(plain(fns.refreshQuery('missing', false)), {
        MetadataRefreshMode: 'FullRefresh', ImageRefreshMode: 'FullRefresh', ReplaceAllMetadata: false, ReplaceAllImages: false,
    });
    assert.deepEqual(plain(fns.refreshQuery('all', true)), {
        MetadataRefreshMode: 'FullRefresh', ImageRefreshMode: 'FullRefresh', ReplaceAllMetadata: true, ReplaceAllImages: true,
    });
    // The dialog starts on "scan", as Jellyfin's does, and hides "replace images" for it
    assert.match(source, /<option value="scan" selected>/);
    assert.match(source, /form\.mode\.value === 'scan'/);
    // The query reaches the server
    assert.match(api, /refreshItem\(id, options\)[\s\S]*?new URLSearchParams\(options\)/);
    assert.match(router, /async function refreshItemMetadata\(button, id, options\)[\s\S]*?API\.refreshItem\(id, options\)/);
});

test('the metadata editor saves Jellyfin-shaped fields and locks the unticked fields', () => {
    const enabled = { Name: true, Overview: false, Genres: true, OfficialRating: true, Cast: false,
        ProductionLocations: true, Runtime: true, Studios: true, Tags: true };
    const body = plain(fns.itemUpdateBody({
        name: ' Alpha ', originalTitle: '', sortName: '', overview: 'Text', tagline: 'Go',
        year: '2001', premiereDate: '2001-05-04', officialRating: 'PG-13', communityRating: '7.5', criticRating: 'x',
        genres: 'Drama, comedy, , drama', tags: '', lockData: false, enabled,
    }));
    assert.deepEqual(body, {
        Name: 'Alpha', OriginalTitle: null, ForcedSortName: null, Overview: 'Text', Taglines: ['Go'],
        ProductionYear: 2001, PremiereDate: '2001-05-04T00:00:00.0000000Z', CommunityRating: 7.5, CriticRating: null,
        OfficialRating: 'PG-13', Genres: ['Drama', 'comedy'], Tags: [], LockData: false, LockedFields: ['Overview', 'Cast'],
    });
    const locked = plain(fns.itemUpdateBody({ name: 'A', year: '', lockData: true, enabled: {} }));
    assert.equal(locked.LockData, true);
    assert.equal(locked.ProductionYear, null);
    assert.equal(locked.LockedFields.length, 9);
    assert.match(api, /updateItem\(id, fields\) \{ return this\.post\(`\/Items\/\$\{id\}`, fields\)/);
});

test('Identify results and remote images are escaped and carry their index', () => {
    const html = fns.searchResultHtml({ Name: '<b>X</b>', ProductionYear: 2001, ImageUrl: 'https://i/x.jpg"', Overview: 'o&o' }, 3);
    assert.match(html, /data-result="3"/);
    assert.match(html, /&lt;b&gt;X&lt;\/b&gt; \(2001\)/);
    assert.match(html, /x\.jpg&quot;/);
    assert.match(html, /o&amp;o/);
    const image = fns.remoteImageHtml({ Url: 'u', ThumbnailUrl: 't', Width: 1000, Height: 1500, Language: 'en',
        CommunityRating: 5.3, VoteCount: 12 }, 2);
    assert.match(image, /data-download="2"/);
    assert.match(image, /src="t"/);
    assert.match(image, /metadata\.image_size\|\{&quot;width&quot;:1000,&quot;height&quot;:1500\} · en · metadata\.votes\|\{&quot;rating&quot;:&quot;5\.3&quot;,&quot;count&quot;:12\}/);
    assert.match(api, /RemoteSearch\/Apply\/\$\{id\}\?ReplaceAllImages=/);
    assert.match(api, /RemoteImages\/Download\?/);
    // Jellyfin's upload body: the picture base64-encoded, with its own content type
    assert.match(api, /readAsDataURL\(file\)/);
    assert.match(api, /'Content-Type': file\.type/);
});

test('the item page offers the tools to admins, Identify only for movies and series', () => {
    const movie = fns.metadataToolButtons({ Id: 'm1', Type: 'Movie' });
    for (const action of ['edit-metadata', 'identify-item', 'edit-images']) assert.match(movie, new RegExp(`data-action="${action}" data-id="m1"`));
    assert.match(movie, /<i:Pencil>[\s\S]*<i:ScanSearch>[\s\S]*<i:Images>/);
    const episode = fns.metadataToolButtons({ Id: 'e1', Type: 'Episode' });
    assert.doesNotMatch(episode, /identify-item/);
    assert.match(episode, /edit-images/);
    // Inside the admin-only part of the action row
    const adminRow = item.slice(item.indexOf('${API.isAdmin ? `'), item.indexOf("` : ''}", item.indexOf('${API.isAdmin ? `')));
    assert.match(adminRow, /metadataToolButtons\(item\)/);
    assert.match(router, /'refresh-item': el => openRefreshDialog\(options => refreshItemMetadata\(el, el\.dataset\.id, options\)\)/);
    for (const [action, fn] of [['edit-metadata', 'openMetadataEditor'], ['identify-item', 'openIdentifyDialog'], ['edit-images', 'openImageEditor']]) {
        assert.match(router, new RegExp(`'${action}': el => ${fn}\\(el\\.dataset\\.id\\)`));
    }
    assert.match(app, /import '\.\/modules\/metadata-admin\.js';/);
    for (const name of ['Pencil', 'ScanSearch', 'Images']) {
        assert.equal(icons.split(new RegExp(`\\b${name}\\b`)).length - 1, 2, `${name} imported and registered`);
    }
});

test('every metadata string has English and German text', () => {
    const keys = new Set([...source.matchAll(/t\('metadata\.([a-z_]+)'/g)].map(m => m[1]));
    for (const field of context.LOCKABLE_FIELDS ?? vm.runInContext('LOCKABLE_FIELDS', context)) keys.add(`field_${field}`);
    for (const type of ['Primary', 'Backdrop', 'Logo', 'Thumb', 'Banner', 'Art', 'Disc']) keys.add(`type_${type}`);
    assert.ok(keys.size > 40, `found ${keys.size} keys`);
    for (const key of keys) {
        assert.ok(en.metadata[key], `en metadata.${key}`);
        assert.ok(de.metadata[key], `de metadata.${key}`);
    }
});
