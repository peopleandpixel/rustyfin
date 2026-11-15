// B171: next-episode autoplay and previous/next episode buttons in the web player.
// Runs app.js's next-episode block (finding the neighbours, the track carry-over, the credits start,
// the card and its countdown) and the autoplay setting in a vm sandbox, and checks index.html,
// icons.js and the translations as text.
// Usage: node b171_next_episode.mjs PATH/TO/app.js (run by api_tests.rs b171_next_episode).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

const appPath = process.argv[2];
const srcDir = path.dirname(appPath);
const source = fs.readFileSync(appPath, 'utf8');
const html = fs.readFileSync(path.join(srcDir, 'index.html'), 'utf8');
const iconsJs = fs.readFileSync(path.join(srcDir, 'icons.js'), 'utf8');

function slice(head) {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return source.slice(start, source.indexOf('\n}\n', start) + 3);
}

const BLOCK = [
    'function isStreamType(', 'function findStream(', 'function adjacentEpisodes(', 'function episodeTrackCarry(',
    'function carriedTracks(', 'function creditsStart(', 'function upNextStart(', 'async function loadEpisodeNav(',
    'function resetEpisodeNav(', 'function updateEpisodeButtons(', 'function playEpisode(',
    'function playAdjacentEpisode(', 'function checkUpNext(', 'function upNextAtEnd(', 'function episodeNumberLabel(',
    'function showUpNext(', 'function renderUpNextCountdown(', 'function upNextTick(', 'function playUpNextNow(',
    'function cancelUpNext(', 'function hideUpNext(', 'async function changeNextEpisodeAutoPlay(',
].map(slice).join('\n');

function element() {
    return { hidden: false, disabled: false, textContent: '', src: '', removeAttribute(name) { delete this[name]; } };
}

// A sandbox with the player's globals stubbed; API calls and plays are logged in order.
function sandbox({ episodes = SERIES, me = { Id: 'u1', Configuration: {} }, segments = [], post } = {}) {
    const log = [];
    const timers = new Map();
    let nextTimer = 1;
    const elements = {};
    const context = vm.createContext({
        console: { warn: () => {} },
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        $: id => (elements[id] ||= element()),
        cardImage: item => (item.Image ? { id: item.Id, type: 'Primary', tag: 't' } : null),
        toast: (msg, type = 'success') => log.push(['toast', msg, type]),
        setInterval: fn => { const id = nextTimer++; timers.set(id, fn); return id; },
        clearInterval: id => timers.delete(id),
        API: {
            get: path => {
                log.push(['get', path]);
                if (path.startsWith('/Shows/')) return Promise.resolve({ Items: episodes });
                if (path.startsWith('/MediaSegments/')) return Promise.resolve({ Items: segments });
                return Promise.reject(new Error('unexpected ' + path));
            },
            getMe: () => Promise.resolve(me),
            post: (path, body) => { log.push(['post', path, body]); return post ? post() : Promise.resolve({}); },
            imageUrl: (id, type, opts) => `/Items/${id}/Images/${type}?maxWidth=${opts.maxWidth}`,
        },
        playItem: (id, type, title, opts) => log.push(['playItem', id, type, title, opts]),
    });
    vm.runInContext(
        `var activePlayback = null; let episodeNav = null; let upNext = null; const UP_NEXT_SECONDS = 10;
         ${BLOCK}
         this.fns = { adjacentEpisodes, episodeTrackCarry, carriedTracks, creditsStart, upNextStart, loadEpisodeNav,
             resetEpisodeNav, playAdjacentEpisode, checkUpNext, upNextAtEnd, upNextTick, playUpNextNow, cancelUpNext,
             changeNextEpisodeAutoPlay };
         this.state = () => ({ episodeNav, upNext });
         this.setActive = s => { activePlayback = s; };`,
        context,
    );
    const tick = () => [...timers.values()].forEach(fn => fn());
    return { fns: context.fns, state: context.state, setActive: context.setActive, log, elements, timers, tick };
}

const ep = (id, season, index, extra = {}) => ({ Id: id, Type: 'Episode', Name: 'Name ' + id, ParentIndexNumber: season, IndexNumber: index, ...extra });
// Out of order on purpose; season 0 holds a special
const SERIES = [ep('s2e1', 2, 1, { Image: true }), ep('s1e2', 1, 2), ep('sp1', 0, 1), ep('s1e1', 1, 1), ep('sp2', 0, 2), ep('s2e2', 2, 2)];

function session(over = {}) {
    let pos = 0;
    return {
        itemId: 's1e2',
        runtime: 1500,
        streams: [
            { Index: 1, CodecType: 'audio', Language: 'jpn' },
            { Index: 2, CodecType: 'audio', Language: 'eng', IsDefault: true },
            { Index: 3, CodecType: 'subtitle', Language: 'eng' },
            { Index: 4, CodecType: 'subtitle', Language: 'eng', IsForced: true },
        ],
        audioIndex: 1,
        subtitleIndex: 3,
        finished: false,
        position() { return pos; },
        total() { return this.runtime; },
        seek(p) { pos = p; },
        ...over,
    };
}

const plain = value => JSON.parse(JSON.stringify(value));

test('next is the following episode in season order, then the next season; specials only around a special', () => {
    const { fns } = sandbox();
    const ids = r => [r.prev?.Id ?? null, r.next?.Id ?? null];
    assert.deepEqual(ids(fns.adjacentEpisodes(SERIES.slice(), 's1e2')), ['s1e1', 's2e1']);
    assert.deepEqual(ids(fns.adjacentEpisodes(SERIES.slice(), 's1e1')), [null, 's1e2']);
    assert.deepEqual(ids(fns.adjacentEpisodes(SERIES.slice(), 's2e2')), ['s2e1', null]);
    assert.deepEqual(ids(fns.adjacentEpisodes(SERIES.slice(), 'sp1')), [null, 'sp2']);
    assert.deepEqual(ids(fns.adjacentEpisodes(SERIES.slice(), 'sp2')), ['sp1', 's1e1']);
    assert.deepEqual(ids(fns.adjacentEpisodes(SERIES.slice(), 'missing')), [null, null]);
});

test('audio and subtitle languages carry over, else the defaults stay; subtitles off stays off', () => {
    const { fns } = sandbox();
    const carry = fns.episodeTrackCarry(session());
    assert.deepEqual(plain(carry), { audioLanguage: 'jpn', subtitleLanguage: 'eng', subtitleForced: false, subtitlesOff: false });
    const next = [
        { Index: 0, CodecType: 'video' },
        { Index: 1, CodecType: 'audio', Language: 'eng', IsDefault: true },
        { Index: 2, CodecType: 'audio', Language: 'JPN' },
        { Index: 3, CodecType: 'subtitle', Language: 'eng', IsForced: true },
        { Index: 4, CodecType: 'subtitle', Language: 'eng' },
        { Index: 5, CodecType: 'subtitle', Language: 'ger', IsDefault: true },
    ];
    assert.deepEqual(plain(fns.carriedTracks(next, carry, 1, 5)), { audioIndex: 2, subtitleIndex: 4 });
    // No track in the language: the defaults stay
    assert.deepEqual(plain(fns.carriedTracks(next, { ...carry, audioLanguage: 'fre', subtitleLanguage: 'spa' }, 1, 5)), { audioIndex: 1, subtitleIndex: 5 });
    // A forced subtitle carries to the forced one
    assert.deepEqual(plain(fns.carriedTracks(next, { ...carry, subtitleForced: true }, 1, 5)).subtitleIndex, 3);
    // Subtitles turned off stay off; an episode with no subtitles at all passes nothing on
    const off = fns.episodeTrackCarry(session({ subtitleIndex: -1 }));
    assert.equal(off.subtitlesOff, true);
    assert.deepEqual(plain(fns.carriedTracks(next, off, 1, 5)), { audioIndex: 2, subtitleIndex: -1 });
    const noSubs = fns.episodeTrackCarry(session({ subtitleIndex: -1, streams: [{ Index: 1, CodecType: 'audio', Language: 'jpn' }] }));
    assert.equal(noSubs.subtitlesOff, false);
    assert.deepEqual(plain(fns.carriedTracks(next, null, 1, 5)), { audioIndex: 1, subtitleIndex: 5 });
});

test('the card starts at the credits, else 10 s before the end, else at the end of a short item', () => {
    const { fns } = sandbox();
    assert.equal(fns.creditsStart([{ Type: 'Intro', SegmentStartTicks: 10e7 }, { Type: 'Outro', SegmentStartTicks: 1400e7 }], [], 1500), 1400);
    // A credits chapter in the second half counts; opening credits don't
    const chapters = [{ Name: 'Opening Credits', StartTicks: 30e7 }, { Name: 'End Credits', StartTicks: 1380e7 }];
    assert.equal(fns.creditsStart([], chapters, 1500), 1380);
    assert.equal(fns.creditsStart([], [{ Name: 'Opening Credits', StartTicks: 30e7 }], 1500), null);
    assert.equal(fns.creditsStart(null, null, 1500), null);
    assert.equal(fns.upNextStart(1500, 1380), 1380);
    assert.equal(fns.upNextStart(1500, null), 1490);
    assert.equal(fns.upNextStart(1500, 1600), 1490);
    assert.equal(fns.upNextStart(45, null), null);
    assert.equal(fns.upNextStart(0, null), null);
});

test('an episode loads its neighbours; the card counts down 10 s and plays the next with the tracks carried', async () => {
    const box = sandbox();
    const { fns, elements, log } = box;
    const s = session();
    box.setActive(s);
    await fns.loadEpisodeNav(s, { Type: 'Episode', SeriesId: 'series-1', Chapters: [] });
    assert.deepEqual(log.filter(e => e[0] === 'get').map(e => e[1]), ['/Shows/series-1/Episodes', '/MediaSegments/s1e2']);
    assert.equal(elements['player-btn-prev-episode'].hidden, false);
    assert.equal(elements['player-btn-prev-episode'].disabled, false);
    assert.equal(elements['player-btn-next-episode'].disabled, false);

    s.seek(1400);
    fns.checkUpNext(s);
    assert.equal(box.state().upNext, null, 'no card before the last 10 s');
    s.seek(1491);
    fns.checkUpNext(s);
    const card = elements['player-up-next'];
    assert.equal(card.hidden, false);
    assert.equal(elements['player-up-next-title'].textContent, 'Name s2e1');
    assert.equal(elements['player-up-next-number'].textContent, 'player.episode_number|{"season":2,"episode":1}');
    assert.equal(elements['player-up-next-img'].src, '/Items/s2e1/Images/Primary?maxWidth=320');
    assert.equal(elements['player-up-next-countdown'].hidden, false);
    assert.equal(elements['player-up-next-countdown'].textContent, 'player.up_next_countdown|{"count":10}');

    for (let i = 0; i < 9; i++) box.tick();
    assert.equal(elements['player-up-next-countdown'].textContent, 'player.up_next_countdown|{"count":1}');
    assert.equal(log.some(e => e[0] === 'playItem'), false);
    box.tick();
    const plays = log.filter(e => e[0] === 'playItem');
    assert.equal(plays.length, 1);
    const [, id, type, title, opts] = plays[0];
    assert.deepEqual([id, type, title], ['s2e1', 'Videos', 'Name s2e1']);
    assert.equal(opts.fromStart, true);
    assert.equal(opts.carry.audioLanguage, 'jpn');
    assert.equal(s.finished, true, 'the old episode is reported as watched to the end');
    assert.equal(card.hidden, true);
    assert.equal(box.timers.size, 0);
});

test('seeking back hides the card; the control bar plays neighbours without marking this one watched', async () => {
    const box = sandbox();
    const { fns, elements, log } = box;
    const s = session();
    box.setActive(s);
    await fns.loadEpisodeNav(s, { Type: 'Episode', SeriesId: 'series-1' });
    s.seek(1495);
    fns.checkUpNext(s);
    assert.equal(elements['player-up-next'].hidden, false);
    s.seek(600);
    fns.checkUpNext(s);
    assert.equal(elements['player-up-next'].hidden, true);
    assert.equal(box.timers.size, 0);

    fns.playAdjacentEpisode(-1);
    const play = log.find(e => e[0] === 'playItem');
    assert.equal(play[1], 's1e1');
    assert.equal(play[4].fromStart, true);
    assert.equal(play[4].carry.subtitleLanguage, 'eng');
    assert.equal(s.finished, false);
});

test('the card shows at the credits from an Outro segment', async () => {
    const box = sandbox({ segments: [{ Type: 'Outro', SegmentStartTicks: 1300e7 }] });
    const s = session();
    box.setActive(s);
    await box.fns.loadEpisodeNav(s, { Type: 'Episode', SeriesId: 'series-1' });
    s.seek(1299);
    box.fns.checkUpNext(s);
    assert.equal(box.state().upNext, null);
    s.seek(1300);
    box.fns.checkUpNext(s);
    assert.equal(box.elements['player-up-next'].hidden, false);
});

test('autoplay off: the card shows without a countdown and waits for Play now', async () => {
    const box = sandbox({ me: { Id: 'u1', Configuration: { EnableNextEpisodeAutoPlay: false } } });
    const { fns, elements, log } = box;
    const s = session();
    box.setActive(s);
    await fns.loadEpisodeNav(s, { Type: 'Episode', SeriesId: 'series-1' });
    s.seek(1495);
    fns.checkUpNext(s);
    assert.equal(elements['player-up-next'].hidden, false);
    assert.equal(elements['player-up-next-countdown'].hidden, true);
    assert.equal(box.timers.size, 0);
    fns.playUpNextNow();
    assert.equal(log.filter(e => e[0] === 'playItem').length, 1);
    assert.equal(s.finished, true);
});

test('Cancel closes the card and the episode ends without it; a short episode shows it at the end', async () => {
    const box = sandbox();
    const { fns, elements, log } = box;
    const s = session();
    box.setActive(s);
    await fns.loadEpisodeNav(s, { Type: 'Episode', SeriesId: 'series-1' });
    s.seek(1495);
    fns.checkUpNext(s);
    fns.cancelUpNext();
    assert.equal(elements['player-up-next'].hidden, true);
    fns.checkUpNext(s);
    fns.upNextAtEnd(s);
    assert.equal(elements['player-up-next'].hidden, true);
    for (let i = 0; i < 12; i++) box.tick();
    assert.equal(log.some(e => e[0] === 'playItem'), false);

    const short = sandbox();
    const s2 = session({ runtime: 40 });
    short.setActive(s2);
    await short.fns.loadEpisodeNav(s2, { Type: 'Episode', SeriesId: 'series-1' });
    s2.seek(39);
    short.fns.checkUpNext(s2);
    assert.equal(short.state().upNext, null);
    short.fns.upNextAtEnd(s2);
    assert.equal(short.elements['player-up-next'].hidden, false);
});

test('the last episode gets no card and a disabled next button; movies get no buttons', async () => {
    const box = sandbox();
    const { fns, elements } = box;
    const s = session({ itemId: 's2e2' });
    box.setActive(s);
    await fns.loadEpisodeNav(s, { Type: 'Episode', SeriesId: 'series-1' });
    assert.equal(elements['player-btn-next-episode'].disabled, true);
    assert.equal(elements['player-btn-prev-episode'].disabled, false);
    s.seek(1495);
    fns.checkUpNext(s);
    fns.upNextAtEnd(s);
    assert.equal(box.state().upNext, null);
    fns.resetEpisodeNav();
    assert.equal(elements['player-btn-next-episode'].hidden, true);

    const movie = sandbox();
    const m = session({ itemId: 'movie' });
    movie.setActive(m);
    await movie.fns.loadEpisodeNav(m, { Type: 'Movie' });
    assert.equal(movie.log.length, 0);
    assert.equal(movie.state().episodeNav, null);
});

test('the setting saves EnableNextEpisodeAutoPlay into the user configuration, keeping the rest', async () => {
    const box = sandbox({ me: { Id: 'u1', Configuration: { SubtitleMode: 'Always' } } });
    const input = { checked: false, disabled: false };
    await box.fns.changeNextEpisodeAutoPlay(input);
    assert.deepEqual(plain(box.log.find(e => e[0] === 'post')), ['post', '/Users/u1/Configuration', { SubtitleMode: 'Always', EnableNextEpisodeAutoPlay: false }]);
    assert.equal(input.disabled, false);

    const failing = sandbox({ post: () => Promise.reject(new Error('nope')) });
    const input2 = { checked: false, disabled: false };
    await failing.fns.changeNextEpisodeAutoPlay(input2);
    assert.equal(input2.checked, true, 'a failed save puts the switch back');
    assert.deepEqual(failing.log.at(-1), ['toast', 'nope', 'error']);
});

test('the player wires the block in: reports, stop, ended and fullscreen', () => {
    const play = slice('async function playItem(');
    assert.match(play, /carriedTracks\(streams, carry, audioIndex, subtitleIndex\)/);
    assert.match(play, /resetEpisodeNav\(\)/);
    assert.match(play, /loadEpisodeNav\(session, item\)/);
    assert.match(play, /checkUpNext\(session\)/);
    assert.match(play, /session\.stop\(\);\s*upNextAtEnd\(session\);/);
    assert.match(play, /session\.finished \? Math\.max\(session\.position\(\), session\.total\(\)\)/);
    assert.match(play, /fields\.PlaySessionId = this\.playSessionId/);
    assert.match(slice('function toggleFullscreen('), /\.jf-player-container/);
});

test('markup, icons and translations', () => {
    for (const id of ['player-btn-prev-episode', 'player-btn-next-episode']) {
        assert.match(html, new RegExp(`<button id="${id}"[^>]*\\bhidden>`), `${id} starts hidden`);
    }
    for (const id of ['player-up-next', 'player-up-next-img', 'player-up-next-number', 'player-up-next-title', 'player-up-next-countdown']) {
        assert.ok(html.includes(`id="${id}"`), `index.html has ${id}`);
    }
    assert.match(html, /onclick="cancelUpNext\(\)"/);
    assert.match(html, /onclick="playUpNextNow\(\)"/);
    assert.match(iconsJs, /SkipBack, SkipForward/);
    const dir = path.join(srcDir, 'translations');
    for (const file of fs.readdirSync(dir)) {
        const data = JSON.parse(fs.readFileSync(path.join(dir, file), 'utf8'));
        for (const key of ['playback', 'next_episode_autoplay', 'next_episode_autoplay_saved']) {
            assert.ok(data.settings[key], `${file} has settings.${key}`);
        }
        if (file === 'en.json' || file === 'de.json') {
            for (const key of ['previous_episode', 'next_episode', 'play_now', 'episode_number', 'up_next_countdown']) {
                assert.ok(data.player[key], `${file} has player.${key}`);
            }
        }
    }
});
