// R3 music: the album page lists its tracks by disc, in track order, with their numbers and
// lengths, and a click on a track plays it.
// Runs modules/music.js's track list functions in a vm sandbox and checks item.js and the
// translations as text. Usage: node r3music_album.mjs (run by api_tests.rs r3music_album).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const music = read('modules/music.js');

// The module without its import and export lines, in a sandbox with the client's globals stubbed
function sandbox() {
    const body = music.replace(/^import .*$/gm, '').replace(/^export \{[^}]*\};$/gm, '');
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        icon: name => `<svg data-icon="${name}"></svg>`,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
    });
    vm.runInContext(`${body}\nthis.fns = { trackDuration, trackDiscGroups, trackRow, albumTrackList };`, context);
    return context.fns;
}

// Two discs, sent unsorted as the server sends them
const NIGHTS = [
    { Id: 'dusk', Name: 'Dusk', IndexNumber: 1, ParentIndexNumber: 1, RunTimeTicks: 30000000, Artists: ['Mike Ensemble'] },
    { Id: 'small', Name: 'Small Hours', IndexNumber: 1, ParentIndexNumber: 2, RunTimeTicks: 2450000000, Artists: ['Mike Ensemble'] },
    { Id: 'dawn', Name: 'Dawn', IndexNumber: 2, ParentIndexNumber: 2, RunTimeTicks: 37250000000, Artists: ['Mike Ensemble'] },
    { Id: 'midnight', Name: 'Midnight', IndexNumber: 2, ParentIndexNumber: 1, RunTimeTicks: 30000000, Artists: ['Mike Ensemble'] },
];

test('track lengths read m:ss, and h:mm:ss from an hour on', () => {
    const { trackDuration } = sandbox();
    assert.equal(trackDuration(30000000), '0:03');
    assert.equal(trackDuration(2450000000), '4:05');
    assert.equal(trackDuration(37250000000), '1:02:05');
    assert.equal(trackDuration(0), '');
    assert.equal(trackDuration(undefined), '');
});

test('tracks group by disc, each disc in track order', () => {
    const { trackDiscGroups } = sandbox();
    const groups = trackDiscGroups(NIGHTS).map(g => [g.disc, g.tracks.map(tr => tr.Id)]);
    assert.deepEqual(JSON.parse(JSON.stringify(groups)), [[1, ['dusk', 'midnight']], [2, ['small', 'dawn']]]);
});

test('tracks without numbers go to disc 1, after the numbered ones', () => {
    const { trackDiscGroups } = sandbox();
    const groups = trackDiscGroups([
        { Id: 'b', Name: 'B' }, { Id: 'two', IndexNumber: 2 }, { Id: 'a', Name: 'A' }, { Id: 'one', IndexNumber: 1 },
    ]);
    assert.equal(groups.length, 1);
    assert.deepEqual(Array.from(groups[0].tracks, tr => tr.Id), ['one', 'two', 'a', 'b']);
});

test('a two-disc album gets disc headings; each row has its number and length and plays its track', () => {
    const { albumTrackList } = sandbox();
    const html = albumTrackList({ Name: 'November Nights', AlbumArtist: 'Mike Ensemble' }, NIGHTS);
    assert.ok(html.includes('music.disc|{"number":1}') && html.includes('music.disc|{"number":2}'));
    assert.ok(html.indexOf('music.disc|{"number":1}') < html.indexOf('music.disc|{"number":2}'));
    assert.ok(html.includes('music.tracks_count|{"count":4}'));
    for (const id of ['dusk', 'midnight', 'small', 'dawn']) {
        assert.match(html, new RegExp(`class="jf-track-row[^"]*" data-action="play" data-id="${id}" data-stream-type="Audio"`));
    }
    // Disc order, then track order
    const order = ['Dusk', 'Midnight', 'Small Hours', 'Dawn'].map(n => html.indexOf(`>${n}<`));
    assert.deepEqual([...order].sort((a, b) => a - b), order);
    assert.ok(order.every(i => i > 0));
    assert.ok(html.includes('<span class="jf-track-duration">4:05</span>'));
    assert.ok(html.includes('<span class="jf-track-duration">1:02:05</span>'));
    assert.match(html, /<span class="group-hover:hidden">2<\/span>/);
    // The album artist is not repeated on every row
    assert.ok(!html.includes('text-white/50">Mike Ensemble<'));
    // Play starts from the first track of disc 1
    assert.match(html, /class="jf-btn-primary[^"]*" data-action="play" data-id="dusk" data-stream-type="Audio"/);
});

test('a one-disc album has no disc heading, and a compilation names each track\'s artists', () => {
    const { albumTrackList } = sandbox();
    const html = albumTrackList({ AlbumArtist: 'Various Artists' }, [
        { Id: 'p', Name: 'Papa Song', IndexNumber: 1, Artists: ['Papa Trio'] },
        { Id: 'q', Name: 'Quebec Song', IndexNumber: 2, Artists: ['Quebec Duo', 'Kilo Band'] },
    ]);
    assert.ok(!html.includes('music.disc|'));
    assert.ok(html.includes('>Papa Trio<'));
    assert.ok(html.includes('>Quebec Duo, Kilo Band<'));
});

test('with several album artists, only a track with an artist outside them names its artists', () => {
    const { albumTrackList } = sandbox();
    const html = albumTrackList({ AlbumArtist: 'Kilo Band', AlbumArtists: [{ Name: 'Kilo Band' }, { Name: 'Mike Ensemble' }] }, [
        { Id: 'a', Name: 'Joint', IndexNumber: 1, Artists: ['Mike Ensemble', 'Kilo Band'] },
        { Id: 'b', Name: 'Solo', IndexNumber: 2, Artists: ['Mike Ensemble'] },
        { Id: 'c', Name: 'Guest', IndexNumber: 3, Artists: ['Kilo Band', 'Papa Trio'] },
    ]);
    assert.ok(!html.includes('text-white/50">Mike Ensemble, Kilo Band<'), 'both are album artists');
    assert.ok(!html.includes('text-white/50">Mike Ensemble<'), 'a second album artist alone is not a guest');
    assert.ok(html.includes('text-white/50">Kilo Band, Papa Trio<'));
});

test('Play and the rows read the id in either case', () => {
    const { albumTrackList } = sandbox();
    const html = albumTrackList({}, [{ id: 'lower', name: 'Lower', IndexNumber: 1 }]);
    assert.match(html, /class="jf-btn-primary[^"]*" data-action="play" data-id="lower" data-stream-type="Audio" data-title="Lower"/);
    assert.match(html, /class="jf-track-row[^"]*" data-action="play" data-id="lower"/);
});

test('an album with no tracks has no Play button', () => {
    const { albumTrackList } = sandbox();
    assert.ok(!albumTrackList({}, []).includes('jf-btn-primary'));
});

test('the item page draws an album\'s children as this track list', () => {
    const item = read('modules/item.js');
    assert.ok(item.includes("import { albumTrackList } from './music.js';"));
    assert.match(item, /type === 'MusicAlbum'\) \{\s*seasonsOrEpisodesHtml = albumTrackList\(item, children\);/);
});

test('the track list strings are in English and German', () => {
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`)).music;
        assert.ok(strings.tracks_count.includes('{{count}}'), lang);
        assert.ok(strings.disc.includes('{{number}}'), lang);
    }
});
