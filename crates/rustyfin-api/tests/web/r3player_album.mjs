// Play album: an album page's Play queues every track in disc and track order, and a click on a track
// plays the album from that track. Runs music.js's track list and playAlbumFrom in a vm sandbox, over
// the rows the list renders, and checks the router's wiring as text.
// Usage: node r3player_album.mjs (run by api_tests.rs r3player_album).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, src), 'utf8');
const musicJs = read('modules/music.js');

function slice(head) {
    const start = musicJs.indexOf(head);
    assert.ok(start >= 0, `music.js has ${head}`);
    return musicJs.slice(start, musicJs.indexOf('\n}\n', start) + 3);
}

// The rendered rows as elements: each `jf-track-row` with its data-* attributes, in document order
function rowsOf(html) {
    const rows = [];
    const re = /<div class="jf-track-row[^"]*" data-action="([^"]+)" data-id="([^"]+)" data-stream-type="Audio" data-title="([^"]*)">/g;
    for (let m; (m = re.exec(html));) {
        rows.push({ dataset: { action: m[1], id: m[2], title: m[3] }, matches: s => s === '.jf-track-row' });
    }
    return rows;
}

function sandbox() {
    const plays = [];
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: () => '',
        escapeHtml: s => String(s),
        playQueueAt: (items, index) => plays.push({ items: JSON.parse(JSON.stringify(items)), index }),
    });
    vm.runInContext(`const itemId = item => item.Id || item.id;
        ${['function trackDuration(', 'function trackDiscGroups(', 'function albumArtistNames(', 'function trackRow(',
        'function albumTrackList(', 'function playAlbumFrom('].map(slice).join('\n')}
        this.fns = { albumTrackList, playAlbumFrom };`, context);
    return { fns: context.fns, plays };
}

// Two discs, listed out of order by the server
const TRACKS = [
    { Id: 'dawn', Name: 'Dawn', ParentIndexNumber: 2, IndexNumber: 1 },
    { Id: 'small', Name: 'Small Hours', ParentIndexNumber: 1, IndexNumber: 3 },
    { Id: 'dusk', Name: 'Dusk', ParentIndexNumber: 1, IndexNumber: 1 },
    { Id: 'midnight', Name: 'Midnight', ParentIndexNumber: 1, IndexNumber: 2 },
];
const ORDER = ['dusk', 'midnight', 'small', 'dawn'];

test('Play queues every track in disc and track order, from the first', () => {
    const { fns, plays } = sandbox();
    const html = fns.albumTrackList({ AlbumArtist: 'Mike Ensemble' }, TRACKS);
    const rows = rowsOf(html);
    assert.deepEqual(rows.map(r => r.dataset.id), ORDER);
    assert.ok(rows.every(r => r.dataset.action === 'play-album'), 'album rows play the album');
    assert.match(html, /data-action="play-album">/);
    const list = { querySelectorAll: () => rows };
    const button = { dataset: {}, matches: () => false, closest: s => (s === '#album-tracks' ? list : null) };
    fns.playAlbumFrom(button);
    assert.equal(plays.length, 1);
    assert.equal(plays[0].index, 0);
    assert.deepEqual(plays[0].items.map(i => i.Id), ORDER);
    assert.deepEqual(plays[0].items[2], { Id: 'small', Name: 'Small Hours', MediaType: 'Audio' });
});

test('a click on a track plays the album from that track; the songs tab still plays one song', () => {
    const { fns, plays } = sandbox();
    const rows = rowsOf(fns.albumTrackList({ AlbumArtist: 'Mike Ensemble' }, TRACKS));
    const list = { querySelectorAll: () => rows };
    for (const row of rows) row.closest = s => (s === '#album-tracks' ? list : null);
    fns.playAlbumFrom(rows[2]);
    assert.equal(plays[0].index, 2, 'from Small Hours');
    assert.deepEqual(plays[0].items.map(i => i.Id), ORDER, 'the whole album is queued');
    // Outside an album page nothing is queued
    fns.playAlbumFrom({ dataset: {}, matches: () => false, closest: () => null });
    assert.equal(plays.length, 1);
    // The songs tab's rows keep data-action="play"
    assert.match(musicJs, /trackRow\(it, \{ showAlbum: true \}\)/);
});

test('the router and the queue are wired in', () => {
    const router = read('modules/router.js');
    assert.match(router, /'play-album': el => playAlbumFrom\(el\)/);
    assert.match(router, /import \{ playAlbumFrom \} from '\.\/music\.js';/);
    assert.match(musicJs, /import \{ playQueueAt \} from '\.\/queue\.js';/);
    assert.ok(!musicJs.includes('TODO(play queue)'));
    assert.match(read('modules/queue.js'), /export \{[^}]*\bplayQueueAt\b/);
});
