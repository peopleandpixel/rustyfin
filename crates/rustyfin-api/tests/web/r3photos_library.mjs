// R3 photos: a photo library browses its folders in place (library/<id>/folder/<path>), folders
// first and then photos and videos, each by sort name with numbers as numbers, as Jellyfin's
// IsFolder,SortName; the viewer's order, shuffle and EXIF panel rows; photos play in the viewer
// and videos in the player. Runs modules/photos.js in a vm sandbox with the API stubbed, and
// checks the wiring in cards.js, router.js and library.js as text.
// Usage: node r3photos_library.mjs (run by api_tests.rs r3photos_web).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = text => text.replace(/^import [^;]*;$/gm, '').replace(/^export \{[^}]*\};$/gm, '');

const ROOT = '/media/Photos';
const item = (id, rel, type = 'Photo', extra = {}) => ({
    Id: id, Name: rel.split('/').pop().replace(/\.\w+$/, ''), Type: type, MediaType: type === 'Photo' ? 'Photo' : 'Video',
    Path: `${ROOT}/${rel}`, ImageTags: { Primary: `t${id}` }, ...extra,
});
const ITEMS = [
    item('p1', 'Holidays/Beach/beach-10.jpg'),
    item('p2', 'Holidays/Beach/beach-2.jpg'),
    item('p3', 'Holidays/2024 Alps/alps-01.jpg'),
    item('p4', 'Family/kitchen.jpg'),
    item('v1', 'Family/birthday clip.mp4', 'Video'),
    item('p5', 'Family/Garden.jpg'),
    item('p6', 'sunset.jpg'),
    item('p7', 'holidays 10/x.jpg'),
    item('p8', 'holidays 9/y.jpg'),
];

function sandbox({ items = ITEMS, libPaths = [ROOT] } = {}) {
    const gets = [];
    const played = [];
    const page = { innerHTML: '' };
    const context = vm.createContext({
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;'),
        toast: () => {},
        $: id => (id === 'page-library' ? page : null),
        location: { hash: '#library/lib1' },
        playItem: (...args) => played.push(args),
        window: { libraries: [] },
        document: { querySelector: () => null },
        API: {
            token: 'tok',
            imageUrl: (id, type, opts) => `/Items/${id}/Images/${type}?tag=${opts.tag}`,
            get: async url => {
                gets.push(url);
                const q = new URLSearchParams(url.split('?')[1]);
                const start = Number(q.get('StartIndex'));
                const limit = Number(q.get('Limit'));
                return { Items: items.slice(start, start + limit), TotalRecordCount: items.length };
            },
        },
    });
    vm.runInContext(`${strip(read('modules/photos.js'))}
        Object.assign(this, { renderPhotoLibrary, photoFolderTree, photosUnder, relativeFolder, libraryRoots,
            viewingOrder, photoInfoRows, folderRoute, photoActions, getOpenFolder: () => openFolder });`, context);
    const lib = { id: 'lib1', name: 'Pictures', content_type: 'Photos', paths: libPaths };
    return { context, gets, played, page, lib };
}

const cardsIn = html => [...html.matchAll(/data-action="(navigate|photo-open)"[^>]*data-id="([^"]+)"/g)].map(m => m[2]);

test('a folder lists its subfolders first, then its photos and videos, by name with numbers as numbers', () => {
    const { context } = sandbox();
    const tree = context.photoFolderTree(ITEMS, [ROOT]);
    assert.deepEqual([...tree.get('').folders], ['Family', 'Holidays', 'holidays 9', 'holidays 10']);
    assert.deepEqual([...tree.get('').items.map(i => i.Id)], ['p6']);
    assert.deepEqual([...tree.get('Holidays').folders], ['2024 Alps', 'Beach']);
    assert.deepEqual([...tree.get('Holidays/Beach').items.map(i => i.Id)], ['p2', 'p1'], 'beach-2 before beach-10');
    assert.deepEqual([...tree.get('Family').items.map(i => i.Id)], ['v1', 'p5', 'p4'], 'case left out');
    // A slideshow of a folder has every photo in and under it, videos left out
    assert.deepEqual([...context.photosUnder(tree, 'Holidays').map(i => i.Id)], ['p3', 'p2', 'p1']);
    assert.deepEqual([...context.photosUnder(tree, '').map(i => i.Id)], ['p6', 'p5', 'p4', 'p3', 'p2', 'p1', 'p8', 'p7']);
});

test('folders are relative to the library path the file is under; without paths, to the deepest common folder', () => {
    const { context } = sandbox();
    assert.equal(context.relativeFolder('/media/Photos/a/b/c.jpg', ['/other', '/media/Photos/']), 'a/b');
    assert.equal(context.relativeFolder('/media/Photos/c.jpg', ['/media/Photos']), '');
    assert.equal(context.relativeFolder('/media/PhotosX/c.jpg', ['/media/Photos']), null);
    assert.deepEqual([...context.libraryRoots({}, ITEMS)], [ROOT]);
    assert.deepEqual([...context.libraryRoots({ Locations: ['/x'] }, ITEMS)], ['/x']);
    assert.equal(context.folderRoute('lib1', 'Holidays/2024 Alps'), 'lib1/folder/Holidays/2024%20Alps');
    assert.equal(context.folderRoute('lib1', ''), 'lib1');
});

test('the library page draws a folder in place with breadcrumbs, and pages through every item once', async () => {
    const many = Array.from({ length: 1203 }, (_, i) => item(`m${i}`, `Big/img-${i}.jpg`));
    const s = sandbox({ items: [...ITEMS, ...many] });
    await s.context.renderPhotoLibrary(s.lib, 'lib1', []);
    assert.equal(s.gets.length, 3, 'pages of 500');
    assert.ok(s.gets.every(u => /ParentId=lib1&Recursive=true&IncludeItemTypes=Photo%2CVideo&Fields=Path/.test(u)), s.gets[0]);
    assert.deepEqual(cardsIn(s.page.innerHTML), [
        'lib1/folder/Big', 'lib1/folder/Family', 'lib1/folder/Holidays', 'lib1/folder/holidays%209', 'lib1/folder/holidays%2010', 'p6',
    ]);
    assert.match(s.page.innerHTML, /data-action="photo-slideshow"/);

    s.context.location.hash = '#library/lib1/folder/Holidays/2024%20Alps';
    await s.context.renderPhotoLibrary(s.lib, 'lib1', ['Holidays', '2024%20Alps']);
    assert.equal(s.gets.length, 3, 'the tree is kept while the library is browsed');
    assert.deepEqual(cardsIn(s.page.innerHTML), ['lib1', 'lib1/folder/Holidays', 'p3']);
    assert.match(s.page.innerHTML, /aria-current="page">2024 Alps</);
});

test('a photo card opens the viewer at it; a video card plays in the player', async () => {
    const s = sandbox();
    await s.context.renderPhotoLibrary(s.lib, 'lib1', ['Family']);
    const opened = [];
    s.context.openPhotoViewer = (items, index) => opened.push([[...items.map(i => i.Id)], index]);
    vm.runInContext('openPhotoViewer = this.openPhotoViewer', s.context);
    const card = id => ({ dataset: { index: String(s.context.getOpenFolder().entry.items.findIndex(i => i.Id === id)) } });
    s.context.photoActions['photo-open'](card('p4'));
    assert.deepEqual(opened, [[['p5', 'p4'], 1]]);
    s.context.photoActions['photo-open'](card('v1'));
    assert.deepEqual(s.played, [['v1', 'Videos', 'birthday clip']]);
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
