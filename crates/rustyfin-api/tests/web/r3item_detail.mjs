// R3 item pages: the detail page's tagline, credit lines, critic rating and "ends at"; media info;
// the version, audio and subtitle pickers; chapters; external links; "mark all played" on a series or
// season; and the admin's delete with its confirmation.
// Runs item.js's helpers in a vm sandbox and checks the markup, icons and translations as text.
// Usage: node r3item_detail.mjs (run by api_tests.rs r3item_detail).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const source = read('modules/item.js');
const router = read('modules/router.js');
const en = JSON.parse(read('translations/en.json'));
const de = JSON.parse(read('translations/de.json'));

// A top-level declaration, from its head to the end given
function slice(head, end = '\n}\n') {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `item.js has ${head}`);
    return source.slice(start, source.indexOf(end, start) + end.length);
}

const BLOCK = [
    slice('function endsAt('), slice('function criticRatingHtml('), slice('function creditLinesHtml('),
    slice('const PROVIDER_URLS = ', '\n};\n'), slice('const PROVIDER_NAMES = ', ';\n'), slice('function externalLinks('),
    slice('function externalLinksHtml('), slice('function languageName('), slice('function streamLabel('),
    slice('function versionLabels('), slice('function chosenTracks('), slice('function trackSelectsHtml('),
    slice('function changeTrackSelect('), slice('function detailSelection('), slice('function mediaInfo('),
    slice('function mediaInfoHtml('), slice('function clockTime('), slice('function chaptersHtml('),
    slice('function playedButtonTitle('), slice('function afterDeletePage('), slice('async function deleteItemFromDetail('),
].join('\n');

function sandbox({ confirmed = true, deleteFails = false } = {}) {
    const log = [];
    const context = vm.createContext({
        Intl,
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        icon: name => `<i:${name}>`,
        document: { documentElement: { lang: 'en' } },
        confirm: text => { log.push(['confirm', text]); return confirmed; },
        toast: (msg, type = 'success') => log.push(['toast', msg, type]),
        navigate: (...args) => log.push(['navigate', ...args]),
        API: {
            imageUrl: (id, type, opts) => `/Items/${id}/Images/${type}?maxWidth=${opts.maxWidth}&tag=${opts.tag}`,
            deleteItem: id => { log.push(['delete', id]); return deleteFails ? Promise.reject(new Error('nope')) : Promise.resolve(); },
            invalidateCache: prefix => log.push(['invalidate', prefix]),
        },
    });
    vm.runInContext(
        `let detailChoice = null; let activeItem = null;
         ${BLOCK}
         this.fns = { endsAt, criticRatingHtml, creditLinesHtml, externalLinks, externalLinksHtml, streamLabel,
             versionLabels, chosenTracks, trackSelectsHtml, changeTrackSelect, detailSelection, mediaInfo, mediaInfoHtml,
             chaptersHtml, playedButtonTitle, afterDeletePage, deleteItemFromDetail };
         this.setChoice = c => { detailChoice = c; };
         this.setActive = i => { activeItem = i; };`,
        context,
    );
    return { ...context.fns, setChoice: context.setChoice, setActive: context.setActive, log };
}

const plain = value => JSON.parse(JSON.stringify(value));

// Two versions of a film, as /Items/{id} sends them (no names, no DisplayTitle, no default indexes)
const stream = (Index, Type, extra = {}) => ({ Index, Type, ...extra });
const version = (Id, file, height) => ({
    Id,
    Path: `/m/Kilo Film (2021)/${file}.mkv`,
    Container: 'mkv',
    Size: 1736680,
    Bitrate: 462759,
    MediaStreams: [
        stream(0, 'Video', { Codec: 'h264', Width: height * 16 / 9, Height: height, RealFrameRate: '25/1', Profile: 'High' }),
        stream(1, 'Audio', { Codec: 'aac', Language: 'eng', ChannelLayout: 'stereo', IsDefault: true }),
        stream(2, 'Audio', { Codec: 'ac3', Language: 'deu', ChannelLayout: '5.1' }),
        stream(3, 'Subtitle', { Codec: 'subrip', Language: 'eng' }),
    ],
});
const SOURCES = [version('v1080', 'Kilo Film (2021) - 1080p', 1080), version('v720', 'Kilo Film (2021) - 720p', 720)];
const choice = (extra = {}) => ({ itemId: 'kilo', sources: SOURCES, mediaSourceId: null, audioIndex: null, subtitleIndex: null, ...extra });

// --- Tagline, credits, critic rating, ends at ---

test('ends at counts the time left from now, for videos only', () => {
    const f = sandbox();
    const now = Date.UTC(2026, 0, 1, 20, 0);
    const film = { MediaType: 'Video', RunTimeTicks: 90 * 60 * 1e7 };
    assert.equal(f.endsAt(film, now).getTime(), now + 90 * 60 * 1000);
    const resumed = { ...film, UserData: { PlaybackPositionTicks: 30 * 60 * 1e7 } };
    assert.equal(f.endsAt(resumed, now).getTime(), now + 60 * 60 * 1000, 'the resume point shortens it');
    assert.equal(f.endsAt({ MediaType: 'Audio', RunTimeTicks: 1e9 }, now), null, 'not for audio');
    assert.equal(f.endsAt({ MediaType: 'Video' }, now), null, 'not without a runtime');
    assert.equal(f.endsAt({ ...film, StartDate: '2026-01-01' }, now), null, 'not for a scheduled program');
});

test('critic rating: 60 and up is fresh, as in Jellyfin Web', () => {
    const f = sandbox();
    assert.equal(f.criticRatingHtml({}), '');
    const fresh = f.criticRatingHtml({ CriticRating: 72 });
    assert.match(fresh, /data-fresh="true"/);
    assert.match(fresh, /72%/);
    assert.match(fresh, /<i:ThumbsUp>/);
    assert.match(f.criticRatingHtml({ CriticRating: 59.6 }), /data-fresh="false"[\s\S]*<i:ThumbsDown> 60%/);
    assert.match(f.criticRatingHtml({ CriticRating: 0 }), /0%/, 'zero is a rating');
});

test('director, writer and studio lines take the plural for more than one and link people', () => {
    const f = sandbox();
    assert.equal(f.creditLinesHtml({ People: [{ Name: 'Al', Type: 'Actor' }] }), '', 'actors are the cast row');
    const html = f.creditLinesHtml({
        People: [
            { Id: 'p1', Name: 'Ann <D>', Type: 'Director' },
            { Id: 'p2', Name: 'Will', Type: 'Writer' },
            { Name: 'Wanda', Type: 'Writer' },
        ],
        Studios: [{ Name: 'Alpha Studios', Id: 's1' }],
    });
    assert.match(html, /detail\.director<\/dt><dd[^>]*><a href="#person\/p1"[^>]*>Ann &lt;D&gt;<\/a>/);
    assert.match(html, /detail\.writers<\/dt>.*>Will<\/a>, Wanda<\/dd>/, 'a person without an id is plain text');
    assert.match(html, /detail\.studio<\/dt><dd[^>]*>Alpha Studios<\/dd>/);
    assert.ok(html.indexOf('detail.director') < html.indexOf('detail.writers') && html.indexOf('detail.writers') < html.indexOf('detail.studio'),
        'Jellyfin Web order: director, writer, studio');
});

test('the page shows the tagline, critic rating, ends at and credit lines', () => {
    const render = slice('async function renderDetail(');
    assert.match(render, /\(item\.Taglines \|\| \[\]\)\[0\]/, 'the first tagline');
    assert.match(render, /jf-tagline/);
    assert.match(render, /criticRatingHtml\(item\)/);
    assert.match(render, /detail\.ends_at/);
    assert.match(render, /creditLinesHtml\(item\)/);
});

// --- External links ---

test('external links: the server ExternalUrls, then IMDb, TMDb and TheTVDB from ProviderIds', () => {
    const f = sandbox();
    const links = plain(f.externalLinks({
        Type: 'Movie',
        ExternalUrls: [{ Name: 'TMDb', Url: 'https://www.themoviedb.org/movie/900001' }, { Name: 'Bad', Url: 'javascript:alert(1)' }],
        ProviderIds: { Tmdb: '900001', Imdb: 'tt0000001' },
    }));
    assert.deepEqual(links, [
        { Name: 'TMDb', Url: 'https://www.themoviedb.org/movie/900001' },
        { Name: 'IMDb', Url: 'https://www.imdb.com/title/tt0000001' },
    ], 'no TMDb twice, no javascript: link');
    const show = plain(f.externalLinks({ Type: 'Series', ProviderIds: { Tmdb: '42', Tvdb: '7' }, HomePageUrl: 'https://show.example' }));
    assert.deepEqual(show, [
        { Name: 'detail.website', Url: 'https://show.example' },
        { Name: 'TMDb', Url: 'https://www.themoviedb.org/tv/42' },
        { Name: 'TheTVDB', Url: 'https://thetvdb.com/?tab=series&id=7' },
    ]);
    assert.equal(f.externalLinksHtml({}), '');
    const html = f.externalLinksHtml({ ProviderIds: { Imdb: 'tt1' } });
    assert.match(html, /<a href="https:\/\/www\.imdb\.com\/title\/tt1" target="_blank" rel="noopener noreferrer"[^>]*>IMDb<\/a>/);
    assert.match(slice('async function renderDetail('), /externalLinksHtml\(item\)/);
});

// --- Media info ---

test('media info lists the file, then each video, audio and subtitle stream', () => {
    const f = sandbox();
    const info = plain(f.mediaInfo(SOURCES[0]));
    assert.deepEqual(info.facts, [
        ['detail.container', 'MKV'],
        ['detail.size', 'detail.megabytes|{"value":2}'],
        ['detail.bitrate', 'detail.kbps|{"value":463}'],
    ]);
    assert.deepEqual(info.streams.map(s => s.type), ['Video', 'Audio', 'Audio', 'Subtitle']);
    assert.deepEqual(info.streams[0].facts, [
        ['detail.codec', 'H264'],
        ['detail.profile', 'High'],
        ['detail.resolution', '1920×1080'],
        ['detail.framerate', 'detail.fps|{"value":25}'],
    ], 'missing facts are left out');
    assert.deepEqual(info.streams[1].facts, [
        ['detail.language', 'English'],
        ['detail.codec', 'AAC'],
        ['detail.layout', 'stereo'],
        ['detail.default', 'detail.yes'],
    ]);
    assert.deepEqual(info.streams[3].facts, [['detail.language', 'English'], ['detail.codec', 'SUBRIP']]);
    assert.equal(f.mediaInfo(undefined), null);
    const html = f.mediaInfoHtml(SOURCES[0]);
    assert.match(html, /<details class="jf-media-info[^"]*" data-media-info>/, 'collapsed, a platform <details>');
    assert.match(html, /detail\.audio: English - AAC - stereo - detail\.default/);
    assert.equal(f.mediaInfoHtml({ MediaStreams: [] }), '');
});

// --- Version, audio and subtitle pickers ---

test('stream labels: DisplayTitle, else language, codec and layout', () => {
    const f = sandbox();
    assert.equal(f.streamLabel({ DisplayTitle: 'English - Dolby Digital - 5.1' }), 'English - Dolby Digital - 5.1');
    assert.equal(f.streamLabel({ Index: 2, Type: 'Audio', Codec: 'ac3', Language: 'ger', ChannelLayout: '5.1' }), 'German - AC3 - 5.1');
    assert.equal(f.streamLabel({ Index: 4, Type: 'Subtitle', Codec: 'ass', Language: 'und', IsForced: true, IsExternal: true }),
        'ASS - detail.forced - detail.external');
    assert.equal(f.streamLabel({ Index: 5, Type: 'Subtitle' }), 'detail.track_number|{"number":5}');
});

test('version labels: the name, else the part of the file name the versions do not share', () => {
    const f = sandbox();
    assert.deepEqual(plain(f.versionLabels(SOURCES)), ['1080p', '720p']);
    assert.deepEqual(plain(f.versionLabels([{ Path: '/m/Film 1080p.mkv' }, { Path: '/m/Film 10bit.mkv' }])), ['1080p', '10bit'],
        'cut at a separator only');
    assert.deepEqual(plain(f.versionLabels([{ Name: 'Director cut', Path: '/a.mkv' }, { Path: '/m/Film [4K].mkv' }, { Path: '/m/Film.mkv' }])),
        ['Director cut', 'Film [4K]', 'Film']);
    assert.deepEqual(plain(f.versionLabels([{ MediaStreams: [{ Type: 'Video', Height: 480 }] }, {}])),
        ['480p', 'detail.version_number|{"number":2}']);
});

test('the selects: version for more than one source, audio disabled for one track, subtitles with Off', () => {
    const f = sandbox();
    const html = f.trackSelectsHtml(choice());
    assert.match(html, /data-track-select="version"><option value="v1080" selected>1080p<\/option><option value="v720">720p<\/option>/);
    assert.match(html, /data-track-select="audio"><option value="1" selected>/, 'the default audio track');
    assert.match(html, /data-track-select="subtitle"><option value="-1" selected>detail\.subtitles_off<\/option><option value="3">/,
        'subtitles start off without a default one');
    const single = { itemId: 'x', sources: [{ Id: 's', MediaStreams: [stream(0, 'Video'), stream(1, 'Audio', { Codec: 'aac' })] }] };
    const one = f.trackSelectsHtml(single);
    assert.doesNotMatch(one, /track-select="version"/);
    assert.match(one, /data-track-select="audio" disabled>/);
    assert.doesNotMatch(one, /track-select="subtitle"/);
    assert.equal(f.trackSelectsHtml(null), '');
    assert.equal(f.trackSelectsHtml({ itemId: 'x', sources: [] }), '');
    // The server's default indexes win when it sends them
    const defaults = { itemId: 'x', sources: [{ ...SOURCES[0], DefaultAudioStreamIndex: 2, DefaultSubtitleStreamIndex: 3 }] };
    assert.deepEqual(plain(f.chosenTracks(defaults)).audioIndex, 2);
    assert.deepEqual(plain(f.chosenTracks(defaults)).subtitleIndex, 3);
});

test('a choice passes only what the user picked; a new version starts from its defaults', () => {
    const f = sandbox();
    f.setChoice(choice());
    assert.deepEqual(plain(f.detailSelection('kilo')), { mediaSourceId: 'v1080' }, 'untouched tracks stay the player default');
    assert.equal(f.detailSelection('other'), null, 'only for the item on the page');
    let c = f.changeTrackSelect(choice(), 'audio', '2');
    c = f.changeTrackSelect(c, 'subtitle', '-1');
    f.setChoice(c);
    assert.deepEqual(plain(f.detailSelection('kilo')), { mediaSourceId: 'v1080', audioIndex: 2, subtitleIndex: -1 });
    c = f.changeTrackSelect(c, 'version', 'v720');
    assert.equal(c.audioIndex, null);
    assert.equal(c.subtitleIndex, null);
    f.setChoice(c);
    assert.deepEqual(plain(f.detailSelection('kilo')), { mediaSourceId: 'v720' });
    assert.match(f.trackSelectsHtml(c), /<option value="v720" selected>/);
    f.setChoice({ itemId: 'one', sources: [SOURCES[0]], mediaSourceId: null, audioIndex: null, subtitleIndex: 3 });
    assert.deepEqual(plain(f.detailSelection('one')), { subtitleIndex: 3 }, 'no version for a single source');
});

test('the page wires the pickers: selects under the buttons, media info of the chosen version, Play passes them', () => {
    const render = slice('async function renderDetail(');
    assert.match(render, /id="detail-track-selects"[^>]*>\$\{trackSelectsHtml\(detailChoice\)\}/);
    assert.match(render, /id="detail-media-info">\$\{detailChoice \? mediaInfoHtml\(chosenTracks\(detailChoice\)\.source\)/);
    assert.equal((render.match(/data-detail-tracks="true"/g) || []).length, 2, 'Play/Resume and Play from beginning');
    assert.match(source, /document\.addEventListener\('change', e => \{\s*const name = e\.target\.dataset\?\.trackSelect;/);
    assert.match(router, /if \(el\.dataset\.detailTracks === 'true'\) \{\s*const selection = detailSelection\(id\) \|\| \{\};/);
});

// --- Chapters ---

test('chapters: image where the server has one, name and start time; a click plays from there', () => {
    const f = sandbox();
    assert.equal(f.chaptersHtml({ Id: 'kilo', Chapters: [] }), '');
    const html = f.chaptersHtml({
        Id: 'kilo',
        Name: 'Kilo "Film"',
        Chapters: [
            { Name: 'Opening', StartPositionTicks: 0, ImageTag: 'tag0' },
            { StartPositionTicks: 3725 * 1e7 },
        ],
    });
    assert.match(html, /<h2[^>]*>detail\.chapters<\/h2>/);
    assert.match(html, /src="\/Items\/kilo\/Images\/Chapter\/0\?maxWidth=400&tag=tag0"/);
    assert.equal((html.match(/<img /g) || []).length, 1, 'no image request without an ImageTag');
    assert.match(html, /data-action="play" data-id="kilo" data-stream-type="Videos" data-title="Kilo &quot;Film&quot;" data-start-ticks="0" data-detail-tracks="true"/);
    assert.match(html, /data-start-ticks="37250000000"/);
    assert.match(html, />Opening<\/p>\s*<p[^>]*>0:00<\/p>/);
    assert.match(html, />detail\.chapter_number\|\{&quot;number&quot;:2\}<\/p>\s*<p[^>]*>1:02:05<\/p>/);
    assert.match(slice('async function renderDetail('), /canPlay && streamType === 'Videos' \? chaptersHtml\(item\)/);
    assert.match(router, /if \(el\.dataset\.startTicks !== undefined\) selection\.startTicks = Number\(el\.dataset\.startTicks\);/);
});

// --- Mark all played ---

test('a series or season marks all its episodes, and says so', () => {
    const f = sandbox();
    assert.equal(f.playedButtonTitle(true, false), 'detail.mark_all_played');
    assert.equal(f.playedButtonTitle(true, true), 'detail.mark_all_unplayed');
    assert.equal(f.playedButtonTitle(false, false), 'detail.mark_played');
    const render = slice('async function renderDetail(');
    assert.match(render, /const isContainerOfEpisodes = type === 'Series' \|\| type === 'Season';/);
    assert.match(render, /data-action="toggle-played"[^>]*data-all="\$\{isContainerOfEpisodes\}"/);
    const toggle = slice('async function togglePlayed(');
    assert.match(toggle, /if \(all\) toast\(current \? t\('detail\.all_marked_unplayed'\) : t\('detail\.all_marked_played'\)\);/);
    assert.match(router, /'toggle-played': el => togglePlayed\(el\.dataset\.id, el\.dataset\.played === 'true', el\.dataset\.all === 'true'\)/);
});

// --- Delete ---

test('delete asks first, then deletes and leaves for the parent', async () => {
    const f = sandbox();
    f.setActive({ Id: 'e1', Name: 'Pilot', SeasonId: 's1', SeriesId: 'sr' });
    await f.deleteItemFromDetail('e1');
    assert.deepEqual(plain(f.log), [
        ['confirm', 'detail.delete_confirm|{"name":"Pilot"}'],
        ['delete', 'e1'],
        ['invalidate', '/Items'],
        ['invalidate', '/Shows'],
        ['toast', 'detail.deleted|{"name":"Pilot"}', 'success'],
        ['navigate', 'detail', 's1'],
    ]);
    assert.deepEqual(plain(f.afterDeletePage({ Id: 'se', SeriesId: 'sr' })), ['detail', 'sr']);
    assert.deepEqual(plain(f.afterDeletePage({ Id: 'm' })), ['home']);
    assert.deepEqual(plain(f.afterDeletePage(null)), ['home']);
});

test('delete does nothing when the user cancels, and reports a failure', async () => {
    const cancelled = sandbox({ confirmed: false });
    await cancelled.deleteItemFromDetail('m');
    assert.deepEqual(plain(cancelled.log), [['confirm', 'detail.delete_confirm|{"name":""}']]);
    const failing = sandbox({ deleteFails: true });
    await failing.deleteItemFromDetail('m');
    assert.deepEqual(plain(failing.log.slice(1)), [['delete', 'm'], ['toast', 'detail.delete_failed|{"message":"nope"}', 'error']]);
});

test('delete is an admin button', () => {
    const render = slice('async function renderDetail(');
    const admin = render.slice(render.indexOf('${API.isAdmin ? `'));
    assert.ok(admin.indexOf('data-action="delete-item"') > 0 && admin.indexOf('data-action="delete-item"') < admin.indexOf("` : ''}"),
        'inside the admin-only block');
    assert.match(router, /'delete-item': el => deleteItemFromDetail\(el\.dataset\.id\)/);
});

// --- Icons and strings ---

test('the icons are registered and every new string is in English and German', () => {
    const icons = read('icons.js');
    for (const name of ['ThumbsUp', 'ThumbsDown', 'Trash2']) {
        assert.equal((icons.match(new RegExp(`\\b${name}\\b`, 'g')) || []).length, 2, `${name} imported and in window.lucideIcons`);
    }
    const keys = new Set([...source.matchAll(/'(detail\.[a-z_]+)'/g)].map(m => m[1]));
    assert.ok(keys.size > 40);
    for (const key of keys) {
        const name = key.slice('detail.'.length);
        assert.ok(en.detail[name], `en has ${key}`);
        assert.ok(de.detail[name], `de has ${key}`);
    }
});
