// R3 photos: a photo library browses the server's Folder items in place (library/<id>/folder/<folder
// id>, as a mixed library does): a folder's own items asked with Jellyfin Web's IsFolder,SortName
// order, its breadcrumbs from its ancestors, a slideshow of every photo under it; the viewer's
// order, shuffle and EXIF panel rows; photos play in the viewer and videos in the player. Runs
// modules/photos.js in a vm sandbox with the API stubbed, and checks the wiring in cards.js,
// router.js and library.js as text.
// Usage: node r3photos_library.mjs (run by api_tests.rs r3photos_web).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = text => text.replace(/^import [^;]*;$/gm, '').replace(/^export \{[^}]*\};$/gm, '');

// A library "lib1" with: folders Family (f1) and Holidays (f2, holding 2024 Alps, f3), a photo at
// the top, and in Family a video and two photos. Lists come back in the order the server sorts.
const item = (Id, Name, Type, extra = {}) => ({
    Id, Name, Type, IsFolder: Type === 'Folder', ImageTags: Type === 'Folder' ? {} : { Primary: `t${Id}` }, ...extra,
});
const CHILDREN = {
    lib1: [item('f1', 'Family', 'Folder'), item('f2', 'Holidays', 'Folder'), item('p6', 'sunset', 'Photo')],
    f1: [item('v1', 'birthday clip', 'Video'), item('p5', 'garden', 'Photo'), item('p4', 'kitchen', 'Photo')],
    f2: [item('f3', '2024 Alps', 'Folder')],
    f3: [item('p1', 'alps-01', 'Photo'), item('p2', 'alps-02', 'Photo')],
};
const ITEMS = { f1: item('f1', 'Family', 'Folder'), f2: item('f2', 'Holidays', 'Folder'), f3: item('f3', '2024 Alps', 'Folder') };
const ANCESTORS = {
    f3: [item('f2', 'Holidays', 'Folder'), { Id: 'lib1', Name: 'Pictures', Type: 'CollectionFolder' }, { Id: 'root', Type: 'UserRootFolder' }],
    f1: [{ Id: 'lib1', Name: 'Pictures', Type: 'CollectionFolder' }],
};

function sandbox({ big = 0 } = {}) {
    const gets = [];
    const played = [];
    const page = { innerHTML: '' };
    const children = { ...CHILDREN, lib1: [...CHILDREN.lib1, ...Array.from({ length: big }, (_, i) => item(`m${i}`, `img-${i}`, 'Photo'))] };
    const context = vm.createContext({
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;'),
        toast: () => {},
        $: id => (id === 'page-library' ? page : null),
        location: { hash: '#library/lib1' },
        playItem: (...args) => played.push(args),
        window: {},
        document: { querySelector: () => null },
        API: {
            token: 'tok',
            imageUrl: (id, type, opts) => `/Items/${id}/Images/${type}?tag=${opts.tag}`,
            get: async url => {
                gets.push(url);
                const [path, query] = url.split('?');
                const ancestors = path.match(/^\/Items\/([^/]+)\/Ancestors$/);
                if (ancestors) return ANCESTORS[ancestors[1]] || [];
                const one = path.match(/^\/Items\/([^/]+)$/);
                if (one) return ITEMS[one[1]];
                const q = new URLSearchParams(query);
                const under = id => (children[id] || []).flatMap(i => (i.IsFolder ? under(i.Id) : [i]));
                const all = q.get('Recursive') === 'true'
                    ? under(q.get('ParentId')).filter(i => i.Type === q.get('IncludeItemTypes'))
                    : children[q.get('ParentId')] || [];
                const start = Number(q.get('StartIndex'));
                return { Items: all.slice(start, start + Number(q.get('Limit'))), TotalRecordCount: all.length };
            },
        },
    });
    vm.runInContext(`${strip(read('modules/photos.js'))}
        Object.assign(this, { renderPhotoLibrary, photosUnder, folderTrail, viewingOrder, photoInfoRows, folderRoute,
            photoActions, getOpenFolder: () => openFolder });`, context);
    const lib = { id: 'lib1', name: 'Pictures', content_type: 'Photos' };
    return { context, gets, played, page, lib };
}

const cardsIn = html => [...html.matchAll(/data-action="(navigate|photo-open)"[^>]*data-id="([^"]+)"/g)].map(m => m[2]);
const query = url => Object.fromEntries(new URLSearchParams(url.split('?')[1]));

test('a folder asks for its own items in Jellyfin Web\'s order and shows its subfolders, then its files', async () => {
    const s = sandbox();
    await s.context.renderPhotoLibrary(s.lib, 'lib1', '');
    assert.deepEqual(query(s.gets[0]), {
        ParentId: 'lib1', SortBy: 'IsFolder,SortName', SortOrder: 'Ascending', Fields: 'PrimaryImageAspectRatio,SortName',
        StartIndex: '0', Limit: '500',
    });
    assert.deepEqual(cardsIn(s.page.innerHTML), ['lib1/folder/f1', 'lib1/folder/f2', 'p6']);
    assert.match(s.page.innerHTML, /data-action="photo-slideshow"/);
    assert.match(s.page.innerHTML, /aria-current="page">Pictures</);
});

test('a folder deep down gets breadcrumbs from its ancestors, up to the library', async () => {
    const s = sandbox();
    s.context.location.hash = '#library/lib1/folder/f3';
    await s.context.renderPhotoLibrary(s.lib, 'lib1', 'f3');
    assert.ok(s.gets.includes('/Items/f3/Ancestors'));
    assert.deepEqual(cardsIn(s.page.innerHTML), ['lib1', 'lib1/folder/f2', 'p1', 'p2']);
    assert.match(s.page.innerHTML, /aria-current="page">2024 Alps</);
    assert.equal(s.context.folderRoute('lib1', 'f3'), 'lib1/folder/f3');
    assert.equal(s.context.folderRoute('lib1', ''), 'lib1');
    // A folder page left while it loads isn't drawn
    const loading = s.context.renderPhotoLibrary(s.lib, 'lib1', 'f1');
    s.context.location.hash = '#home';
    await loading;
    assert.deepEqual(cardsIn(s.page.innerHTML), []);
});

test('a big folder is read in pages until every item is in', async () => {
    const s = sandbox({ big: 1203 });
    await s.context.renderPhotoLibrary(s.lib, 'lib1', '');
    assert.deepEqual(s.gets.map(u => query(u).StartIndex), ['0', '500', '1000']);
    assert.equal(cardsIn(s.page.innerHTML).length, 1206);
});

test('a photo card opens the viewer at it; a video card plays in the player; a slideshow has every photo below', async () => {
    const s = sandbox();
    s.context.location.hash = '#library/lib1/folder/f1';
    await s.context.renderPhotoLibrary(s.lib, 'lib1', 'f1');
    const opened = [];
    s.context.openPhotoViewer = (items, index, opts) => opened.push([[...items.map(i => i.Id)], index, opts?.shuffle ?? null]);
    vm.runInContext('openPhotoViewer = this.openPhotoViewer', s.context);
    const card = id => ({ dataset: { index: String(s.context.getOpenFolder().files.findIndex(i => i.Id === id)) } });
    s.context.photoActions['photo-open'](card('p4'));
    assert.deepEqual(opened.pop(), [['p5', 'p4'], 1, null]);
    s.context.photoActions['photo-open'](card('v1'));
    assert.deepEqual(s.played, [['v1', 'Videos', 'birthday clip']]);

    s.context.location.hash = '#library/lib1';
    await s.context.renderPhotoLibrary(s.lib, 'lib1', '');
    await s.context.photoActions['photo-slideshow']({ dataset: { shuffle: 'true' } });
    // Every photo below, in the server's order (the stub lists them depth first)
    assert.deepEqual(opened.pop(), [['p5', 'p4', 'p1', 'p2', 'p6'], 0, true]);
    assert.deepEqual(query(s.gets.at(-1)).Recursive, 'true');
    assert.deepEqual(query(s.gets.at(-1)).IncludeItemTypes, 'Photo');
});

test('the viewing order is the list\'s, or a shuffle that starts at the chosen photo and has every photo once', () => {
    const { context } = sandbox();
    assert.deepEqual([...context.viewingOrder(4, false, 2)], [0, 1, 2, 3]);
    let seed = 7;
    const random = () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    const order = [...context.viewingOrder(20, true, 5, random)];
    assert.equal(order[0], 5);
    assert.deepEqual([...order].sort((a, b) => a - b), [...Array(20).keys()]);
    assert.notDeepEqual(order, [...Array(20).keys()]);
});

test('the EXIF panel shows what the scan read, in readable units, and leaves out what is missing', () => {
    const { context } = sandbox();
    const rows = Object.fromEntries(context.photoInfoRows({
        Width: 1200, Height: 800, CameraMake: 'Canon', CameraModel: 'EOS R6', ExposureTime: 0.004, Aperture: 2.8,
        FocalLength: 50, IsoSpeedRating: 200, Latitude: 48.8584, Longitude: 2.2945, Path: '/media/Photos/a.jpg',
    }));
    assert.equal(rows['photos.exif_size'], '1200 × 800');
    assert.equal(rows['photos.exif_camera'], 'Canon EOS R6');
    assert.equal(rows['photos.exif_exposure'], '1/250 s');
    assert.equal(rows['photos.exif_aperture'], 'ƒ/2.8');
    assert.equal(rows['photos.exif_focal'], '50 mm');
    assert.equal(rows['photos.exif_iso'], '200');
    assert.equal(rows['photos.exif_location'], '48.85840, 2.29450');
    assert.ok(!('photos.exif_altitude' in rows) && !('photos.exif_software' in rows));
    assert.deepEqual([...context.photoInfoRows({})], []);
});

test('the viewer has next/previous, arrow keys, swipe, zoom and pan, and the slideshow controls', () => {
    const photos = read('modules/photos.js');
    for (const key of ['ArrowRight', 'ArrowLeft', 'Escape', "' '"]) assert.ok(photos.includes(`${key}:`), key);
    for (const event of ['wheel', 'dblclick', 'pointerdown', 'pointermove', 'pointerup']) assert.ok(photos.includes(`'${event}'`), event);
    for (const control of ['photo-prev', 'photo-next', 'photo-play', 'photo-shuffle', 'photo-info', 'data-photo-interval']) {
        assert.ok(photos.includes(control), control);
    }
});

test('photos play in the viewer from cards, card menus, detail pages and the library', () => {
    assert.match(read('modules/cards.js'), /canPlay = \[[^\]]*'Photo'\]/);
    assert.match(read('modules/item.js'), /canPlay = \[[^\]]*'Photo'\]/);
    assert.match(read('modules/router.js'), /streamType === 'Photo'\) return openPhotoItem/);
    assert.match(read('modules/router.js'), /el\.dataset\.type === 'Photo' && el\.dataset\.page === 'detail'/);
    assert.match(read('modules/cardmenu.js'), /item\?\.Type === 'Photo'\) openPhotoViewer/);
    assert.match(read('modules/library.js'), /contentType === 'Photos'\) \{[^}]*renderPhotoLibrary/);
    const icons = read('icons.js');
    for (const name of ['ChevronRight', 'ChevronUp', 'ChevronDown', 'Info']) {
        assert.equal(icons.split(new RegExp(`\\b${name}\\b`)).length - 1, 2, `${name} imported and registered`);
    }
});

test('every photos string is in English and German', () => {
    const used = new Set([...read('modules/photos.js').matchAll(/t\(`?'?(photos\.[a-z_]+)/g)].map(m => m[1]).filter(k => !k.endsWith('_')));
    // The EXIF rows' labels are built as photos.exif_<row>
    for (const row of ['taken', 'size', 'camera', 'software', 'exposure', 'aperture', 'focal', 'iso', 'orientation', 'location', 'altitude', 'path']) {
        used.add(`photos.exif_${row}`);
    }
    assert.ok(used.size > 25, String(used.size));
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`));
        for (const key of used) assert.equal(typeof key.split('.').reduce((o, k) => o?.[k], strings), 'string', `${lang}: ${key}`);
    }
});
