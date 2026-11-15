// The web player's play queue: Play all and Shuffle build it the way Jellyfin Web does (/Items with
// SortBy=Random for Shuffle), previous/next and the end of an item walk it, and the next-episode card and
// autoplay setting work with it. Runs queue.js and player.js's next-item block in a vm sandbox and checks the
// markup as text.
// Usage: node r3player_queue.mjs (run by api_tests.rs r3player_queue).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, src), 'utf8');
const queueJs = read('modules/queue.js');
const playerJs = read('modules/player.js');

function slice(source, head) {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `source has ${head}`);
    return source.slice(start, source.indexOf('\n}\n', start) + 3);
}
// A top-level `const NAME = ...;` line
function decl(source, name) {
    const m = new RegExp(`^const ${name} = .*;$`, 'm').exec(source);
    assert.ok(m, `source declares ${name}`);
    return m[0];
}

const QUEUE = ['function queueStreamType(', 'function queueQuery(', 'function queueStartIndex(', 'async function playContainer(',
    'function playQueueAt(', 'function queueNeighbours(', 'function playQueueStep(', 'function clearQueue('];
const NAV = ['function isStreamType(', 'function findStream(', 'function defaultSubtitleIndex(', 'function adjacentEpisodes(',
    'function episodeTrackCarry(', 'function creditsStart(', 'function upNextStart(', 'function loadMediaSegments(',
    'async function loadEpisodeNav(', 'function resetEpisodeNav(', 'function updateEpisodeButtons(', 'function playEpisode(',
    'function playAdjacentEpisode(', 'function checkUpNext(', 'function upNextAtEnd(', 'function episodeNumberLabel(',
    'function showUpNext(', 'function renderUpNextCountdown(', 'function upNextTick(', 'function playUpNextNow(',
    'function cancelUpNext(', 'function hideUpNext('];

const element = () => ({ hidden: false, disabled: false, textContent: '', title: '', attrs: {}, setAttribute(k, v) { this.attrs[k] = v; }, removeAttribute() {} });

// queue.js and the player's next-item block in one sandbox; playItem is logged, not run
function sandbox({ responses = {}, me = { Configuration: {} } } = {}) {
    const log = [];
    const elements = {};
    const handlers = {};
    const context = vm.createContext({
        console: { warn: () => {} },
        URLSearchParams,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        $: id => (elements[id] ||= element()),
        toast: (msg, type) => log.push(['toast', msg, type]),
        cardImage: () => null,
        setInterval: () => 1,
        clearInterval: () => {},
        setMediaSessionHandler: (action, fn) => { handlers[action] = fn; },
        API: {
            get: path => {
                log.push(['get', path]);
                const hit = Object.keys(responses).find(p => path.startsWith(p));
                return hit ? Promise.resolve(responses[hit]) : Promise.reject(new Error('unexpected ' + path));
            },
            getMe: () => Promise.resolve(me),
        },
    });
    vm.runInContext(
        `let playQueue = null; ${decl(queueJs, 'QUEUE_LIMIT')} ${decl(queueJs, 'SERIES_LIMIT')}
         var activePlayback = null; let episodeNav = null; let upNext = null; const UP_NEXT_SECONDS = 10;
         ${QUEUE.map(h => slice(queueJs, h)).join('\n')}
         ${NAV.map(h => slice(playerJs, h)).join('\n')}
         this.fns = { queueQuery, queueStartIndex, playContainer, playQueueAt, queueNeighbours, playQueueStep, clearQueue,
             loadEpisodeNav, playAdjacentEpisode, checkUpNext, upNextAtEnd, playUpNextNow, resetEpisodeNav };
         this.state = () => ({ playQueue, episodeNav, upNext });
         this.setActive = s => { activePlayback = s; };`,
        context,
    );
    // playItem as the player runs it: the next session becomes the active one
    context.playItem = (id, type, title, opts) => log.push(['playItem', id, type, title, JSON.parse(JSON.stringify(opts))]);
    return { fns: context.fns, state: context.state, setActive: context.setActive, log, elements, handlers };
}

const query = path => Object.fromEntries(new URL(path, 'http://x').searchParams);
const session = (itemId, streamType = 'Videos') => ({ itemId, streamType, runtime: 1500, streams: [], audioIndex: null, subtitleIndex: null, position: () => 0, total: () => 1500 });

test('Play all lists a container\'s items in Jellyfin Web\'s order; Shuffle asks the server for SortBy=Random', () => {
    const { fns } = sandbox();
    assert.equal(fns.queueQuery({ Id: 's', Type: 'Series' }, false), '/Shows/s/Episodes?Limit=100');
    assert.equal(fns.queueQuery({ Id: 'se', Type: 'Season', SeriesId: 's' }, false), '/Shows/s/Episodes?SeasonId=se');
    assert.deepEqual(query(fns.queueQuery({ Id: 's', Type: 'Series' }, true)),
        { ParentId: 's', Recursive: 'true', IncludeItemTypes: 'Episode', SortBy: 'Random', Limit: '100' });
    assert.deepEqual(query(fns.queueQuery({ Id: 'se', Type: 'Season' }, true)),
        { ParentId: 'se', Recursive: 'true', IncludeItemTypes: 'Episode', SortBy: 'Random', Limit: '300' });
    assert.deepEqual(query(fns.queueQuery({ Id: 'a', Type: 'MusicAlbum' }, false)),
        { ParentId: 'a', Recursive: 'true', MediaTypes: 'Audio', SortBy: 'ParentIndexNumber,IndexNumber,SortName', Limit: '300' });
    assert.deepEqual(query(fns.queueQuery({ Id: 'ar', Type: 'MusicArtist' }, true)),
        { ArtistIds: 'ar', Recursive: 'true', MediaTypes: 'Audio', SortBy: 'Random', Limit: '300' });
    // A playlist and a collection keep their saved order
    for (const Type of ['Playlist', 'BoxSet']) {
        assert.deepEqual(query(fns.queueQuery({ Id: 'p', Type }, false)), { ParentId: 'p', Recursive: 'true', MediaTypes: 'Audio,Video', Limit: '300' });
    }
    // A movie library by name; Shuffle on it is random
    assert.deepEqual(query(fns.queueQuery({ Id: 'lib', Type: 'CollectionFolder', CollectionType: 'movies' }, false)),
        { ParentId: 'lib', Recursive: 'true', MediaTypes: 'Audio,Video', SortBy: 'SortName', Limit: '300' });
    assert.equal(query(fns.queueQuery({ Id: 'lib', Type: 'CollectionFolder', CollectionType: 'music' }, true)).SortBy, 'Random');
});

test('a series starts at its first unplayed episode; Shuffle and other containers start at the top', () => {
    const { fns } = sandbox();
    const items = [{ UserData: { Played: true } }, { UserData: { Played: false } }, {}];
    assert.equal(fns.queueStartIndex({ Type: 'Series' }, items, false), 1);
    assert.equal(fns.queueStartIndex({ Type: 'Season' }, [{ UserData: { Played: true } }], false), 0);
    assert.equal(fns.queueStartIndex({ Type: 'Series' }, items, true), 0);
    assert.equal(fns.queueStartIndex({ Type: 'MusicAlbum' }, items, false), 0);
});

test('Shuffle plays the random list from its first item; previous and next walk it; an empty one says so', async () => {
    const songs = [{ Id: 'x', MediaType: 'Audio', Name: 'X' }, { Id: 'y', MediaType: 'Audio', Name: 'Y' }, { Id: 'f', IsFolder: true }];
    const box = sandbox({ responses: { '/Items/al': { Id: 'al', Type: 'MusicAlbum' }, '/Items?': { Items: songs } } });
    await box.fns.playContainer('al', { shuffle: true });
    assert.equal(query(box.log[1][1]).SortBy, 'Random');
    assert.deepEqual(box.log[2], ['playItem', 'x', 'Audio', 'X', { fromStart: true, queued: true }]);
    assert.equal(box.state().playQueue.items.length, 2, 'folders are left out');
    assert.deepEqual(JSON.parse(JSON.stringify(box.fns.queueNeighbours())), { prev: null, next: songs[1] });
    assert.equal(box.fns.playQueueStep(1), true);
    assert.deepEqual(box.log[3], ['playItem', 'y', 'Audio', 'Y', { fromStart: true, queued: true, carry: null }]);
    assert.equal(box.fns.playQueueStep(1), false, 'no item after the last');
    box.fns.clearQueue();
    assert.equal(box.fns.queueNeighbours(), null);

    const empty = sandbox({ responses: { '/Items/al': { Id: 'al', Type: 'MusicAlbum' }, '/Items?': { Items: [] } } });
    await empty.fns.playContainer('al');
    assert.deepEqual(empty.log.at(-1), ['toast', 'player.queue_empty', 'error']);
});

test('in a queue the control bar\'s previous/next play its neighbours, also for movies, and the OS keys follow', async () => {
    const movies = [{ Id: 'm1', Type: 'Movie', Name: 'One' }, { Id: 'm2', Type: 'Movie', Name: 'Two' }];
    const box = sandbox({ responses: { '/MediaSegments/': { Items: [] } } });
    box.fns.playQueueAt(movies, 0);
    const s = session('m1');
    box.setActive(s);
    await box.fns.loadEpisodeNav(s, { Type: 'Movie', Id: 'm1' });
    const nav = box.state().episodeNav;
    assert.equal(nav.queue, true);
    assert.equal(box.elements['player-btn-next-episode'].hidden, false);
    assert.equal(box.elements['player-btn-next-episode'].disabled, false);
    assert.equal(box.elements['player-btn-prev-episode'].disabled, true);
    assert.equal(box.elements['player-btn-next-episode'].title, 'player.next_item');
    assert.equal(typeof box.handlers.nexttrack, 'function');
    assert.equal(box.handlers.previoustrack, null, 'no previous: the OS hides its button');
    box.handlers.nexttrack();
    assert.deepEqual(box.log.at(-1).slice(0, 4), ['playItem', 'm2', 'Videos', 'Two']);
    assert.equal(box.log.at(-1)[4].queued, true);
    assert.equal(box.state().playQueue.index, 1);
});

test('a video queue shows the next-item card and honours the autoplay setting; music plays straight on', async () => {
    const movies = [{ Id: 'm1', Type: 'Movie', Name: 'One' }, { Id: 'm2', Type: 'Movie', Name: 'Two' }];
    const box = sandbox({ responses: { '/MediaSegments/': { Items: [] } }, me: { Configuration: { EnableNextEpisodeAutoPlay: false } } });
    box.fns.playQueueAt(movies, 0);
    const s = session('m1');
    box.setActive(s);
    await box.fns.loadEpisodeNav(s, { Type: 'Movie' });
    assert.equal(box.state().episodeNav.autoPlay, false);
    box.fns.upNextAtEnd(s);
    assert.ok(box.state().upNext, 'the card shows');
    assert.equal(box.elements['player-up-next-heading'].textContent, 'player.up_next');
    assert.match(box.elements['player-up-next-status'].textContent, /^player\.up_next_item_announce_manual/);
    assert.equal(box.log.filter(e => e[0] === 'playItem').length, 1, 'autoplay off waits for Play now');
    box.fns.playUpNextNow();
    assert.deepEqual(box.log.at(-1).slice(0, 2), ['playItem', 'm2']);
    assert.equal(s.finished, true);

    const songs = [{ Id: 'a1', MediaType: 'Audio', Name: 'A' }, { Id: 'a2', MediaType: 'Audio', Name: 'B' }];
    const music = sandbox({ me: { Configuration: { EnableNextEpisodeAutoPlay: false } } });
    music.fns.playQueueAt(songs, 0);
    const a = session('a1', 'Audio');
    music.setActive(a);
    await music.fns.loadEpisodeNav(a, { Type: 'Audio' });
    assert.equal(music.log.some(e => e[0] === 'get'), false, 'no segments for music');
    assert.equal(music.state().episodeNav.autoPlay, true, 'the setting holds back videos only');
    music.fns.checkUpNext(a);
    assert.equal(music.state().upNext, null, 'no card for a song');
    music.fns.upNextAtEnd(a);
    assert.equal(music.state().upNext, null);
    assert.deepEqual(music.log.at(-1).slice(0, 3), ['playItem', 'a2', 'Audio']);
});

test('the player ends the queue on any other play and on close; Play all and Shuffle are wired in', () => {
    const play = slice(playerJs, 'async function playItem(');
    assert.match(play, /queued = false/);
    assert.match(play, /if \(!queued\) clearQueue\(\);/);
    assert.match(slice(playerJs, 'function onPlayerClosed('), /clearQueue\(\);/);
    const router = read('modules/router.js');
    assert.match(router, /'play-all': el => playContainer\(el\.dataset\.id\)/);
    assert.match(router, /shuffle: el => playContainer\(el\.dataset\.id, \{ shuffle: true \}\)/);
    const item = read('modules/item.js');
    assert.match(item, /\['Series', 'Season', 'MusicAlbum', 'MusicArtist', 'Playlist', 'BoxSet', 'Folder'\]/);
    assert.match(item, /data-action="play-all"/);
    assert.match(item, /data-action="shuffle"/);
    assert.match(read('modules/library.js'), /data-action="shuffle" data-id="\$\{escapeHtml\(libId\)\}"/);
    assert.match(read('app.js'), /import '\.\/modules\/queue\.js';/);
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`)).player;
        for (const key of ['shuffle', 'queue_empty', 'up_next', 'previous_item', 'next_item', 'up_next_item_announce', 'up_next_item_announce_manual']) {
            assert.ok(strings[key], `${lang} has player.${key}`);
        }
    }
    const icons = read('icons.js');
    assert.equal(icons.match(/\bShuffle\b/g).length, 2, 'Shuffle is imported and registered');
});
