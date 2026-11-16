// The web client's extras: the item page's Trailer button (a local trailer plays in the player, else a remote
// one opens in a new tab), its Special Features row, and theme songs (Jellyfin Web's enableThemeSongs), which
// play quietly on item pages and stop on leaving them or on playback.
// Runs item.js's and theme-media.js's code in a vm sandbox and checks the markup and wiring as text.
// Usage: node r3extras_web.mjs (run by api_tests.rs r3extras_web).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const item = read('modules/item.js');
const theme = read('modules/theme-media.js');
const router = read('modules/router.js');
const player = read('modules/player.js');
const settings = read('modules/settings.js');
const api = read('api.js');
const app = read('app.js');
const en = JSON.parse(read('translations/en.json'));
const de = JSON.parse(read('translations/de.json'));

function slice(source, head, end = '\n}\n') {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `has ${head}`);
    return source.slice(start, source.indexOf(end, start) + end.length);
}

function itemSandbox() {
    const context = vm.createContext({
        t: key => key,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        icon: name => `<i:${name}>`,
        formatRuntime: ticks => (ticks ? `${Math.round(ticks / 6e8)}m` : ''),
        API: { imageUrl: (id, type) => `/Items/${id}/Images/${type}` },
    });
    vm.runInContext(
        `${slice(item, 'const EXTRA_TYPE_KEYS = ', '\n};\n')}
         ${slice(item, 'function trailerButtonHtml(')}
         ${slice(item, 'function specialFeaturesHtml(')}
         this.fns = { trailerButtonHtml, specialFeaturesHtml };`,
        context,
    );
    return context.fns;
}

test('the Trailer button plays a local trailer in the player', () => {
    const { trailerButtonHtml } = itemSandbox();
    const html = trailerButtonHtml(
        { Name: 'Alpha', RemoteTrailers: [{ Url: 'https://www.youtube.com/watch?v=x' }] },
        [{ Id: 't1', Name: 'Alpha-trailer' }],
    );
    assert.match(html, /^<button[^>]*data-action="play" data-id="t1" data-stream-type="Videos"/);
    assert.match(html, /data-from-start="true"/, 'a trailer starts from the beginning');
    assert.match(html, /<i:Film> detail\.trailer/);
    assert.doesNotMatch(html, /youtube/, 'the local trailer wins');
});

test('without a local trailer it opens the remote one in a new tab, and http(s) only', () => {
    const { trailerButtonHtml } = itemSandbox();
    const html = trailerButtonHtml({ RemoteTrailers: [{ Url: 'javascript:alert(1)' }, { Url: 'https://www.youtube.com/watch?v=a&b=1', Name: 'Official' }] }, []);
    assert.match(html, /^<a [^>]*href="https:\/\/www\.youtube\.com\/watch\?v=a&amp;b=1" target="_blank" rel="noopener noreferrer"/);
    assert.match(html, /title="Official"/);
    assert.equal(trailerButtonHtml({ RemoteTrailers: [{ Url: 'javascript:alert(1)' }] }, []), '');
    assert.equal(trailerButtonHtml({}, []), '', 'no trailer, no button');
});

test('the Special Features row plays each feature and labels its kind', () => {
    const { specialFeaturesHtml } = itemSandbox();
    assert.equal(specialFeaturesHtml([]), '');
    const html = specialFeaturesHtml([
        { Id: 'f1', Name: 'Making <Of>', ExtraType: 'Featurette', RunTimeTicks: 6e9 },
        { Id: 'f2', Name: 'Cut', ExtraType: 'DeletedScene', ImageTags: { Primary: 'tag' } },
        { Id: 'f3', Name: 'Misc', ExtraType: 'Unknown' },
    ]);
    assert.match(html, /detail\.special_features/);
    assert.equal(html.match(/data-action="play"/g).length, 3);
    assert.match(html, /data-id="f1"[^>]*data-title="Making &lt;Of&gt;"/);
    assert.match(html, /detail\.extra_featurette · 10m/);
    assert.match(html, /detail\.extra_deleted_scene/);
    assert.match(html, /\/Items\/f2\/Images\/Primary/);
    assert.match(html, /data-id="f3"[\s\S]*?<p class="text-\[10px\] text-white\/50"><\/p>/, 'no label for Unknown');
});

test('the item page asks for extras only when the item has some, and plays the theme song', () => {
    const render = slice(item, 'async function renderDetail(');
    assert.match(render, /item\.LocalTrailerCount > 0 \? API\.getLocalTrailers\(id\)/);
    assert.match(render, /\['Movie', 'Series', 'Season', 'Episode'\]\.includes\(type\)/);
    assert.match(render, /showsSpecials && item\.SpecialFeatureCount > 0 \? API\.getSpecialFeatures\(id\)/);
    assert.match(render, /trailerButtonHtml\(item,/);
    assert.match(render, /specialFeaturesHtml\(/);
    assert.match(render, /playThemeSong\(id, \(\) => currentDetailId === itemId && API\.token === session\)/);
    assert.match(api, /getLocalTrailers\(id\) \{ return this\.cachedGet\(`\/Items\/\$\{id\}\/LocalTrailers`\)/);
    assert.match(api, /getSpecialFeatures\(id\) \{ return this\.cachedGet\(`\/Items\/\$\{id\}\/SpecialFeatures`\)/);
    assert.match(api, /getThemeSongs\(id\) \{ return this\.get\(`\/Items\/\$\{id\}\/ThemeSongs\?inheritFromParent=true`\)/);
});

// theme-media.js without its import and export lines, with a fake Audio and settings
function themeSandbox({ enabled = true, songs = { OwnerId: 'own1', Items: [{ Id: 's1' }] } } = {}) {
    const log = [];
    const audios = [];
    class FakeAudio {
        constructor(url) { this.url = url; this.paused = true; audios.push(this); }
        play() { this.paused = false; log.push(['play', this.url]); return Promise.resolve(); }
        pause() { this.paused = true; log.push(['pause', this.url]); }
        removeAttribute(name) { if (name === 'src') this.url = null; }
        load() {}
    }
    const context = vm.createContext({
        Audio: FakeAudio,
        userSetting: (name, fallback) => (name === 'enableThemeSongs' ? String(enabled) : fallback),
        API: {
            streamUrl: (id, type) => `/${type}/${id}/stream`,
            getThemeSongs: id => { log.push(['fetch', id]); return Promise.resolve(typeof songs === 'function' ? songs(id) : songs); },
        },
    });
    const code = theme.split('\n').filter(line => !/^(import|export) /.test(line)).join('\n');
    vm.runInContext(`${code}\nthis.fns = { themeSongsEnabled, playThemeSong, stopThemeSong };`, context);
    return { ...context.fns, log, audios };
}

test('theme songs are off unless the user turns them on', async () => {
    const f = themeSandbox({ enabled: false });
    assert.equal(f.themeSongsEnabled(), false);
    await f.playThemeSong('m1');
    assert.deepEqual(f.log, [], 'nothing is fetched');
    assert.match(theme, /userSetting\('enableThemeSongs', 'false'\) === 'true'/, "Jellyfin Web's default, off");
});

test('a theme song plays quietly in a loop, keeps playing for the same owner and stops', async () => {
    const f = themeSandbox();
    await f.playThemeSong('m1');
    assert.equal(f.audios.length, 1);
    const [audio] = f.audios;
    assert.equal(audio.url, '/Audio/s1/stream');
    assert.ok(audio.volume > 0 && audio.volume <= 0.5, 'quietly');
    assert.equal(audio.loop, true);
    assert.equal(audio.paused, false);
    await f.playThemeSong('season1');
    assert.equal(f.audios.length, 1, 'the same owner (a season inheriting its series) keeps the song');
    f.stopThemeSong();
    assert.equal(audio.paused, true);
    assert.equal(audio.url, null, 'the stream is let go');
});

test('a page with another or no theme replaces or stops it; a page left meanwhile plays nothing', async () => {
    let owner = 'a';
    const f = themeSandbox({ songs: () => (owner ? { OwnerId: owner, Items: [{ Id: `song-${owner}` }] } : { OwnerId: 'x', Items: [] }) });
    await f.playThemeSong('i1');
    owner = 'b';
    await f.playThemeSong('i2');
    assert.equal(f.audios.length, 2);
    assert.equal(f.audios[0].paused, true, 'the old song stopped');
    assert.equal(f.audios[1].paused, false);
    owner = null;
    await f.playThemeSong('i3');
    assert.equal(f.audios[1].paused, true, 'a page without a theme song stops it');
    owner = 'c';
    await f.playThemeSong('i4', () => false);
    assert.equal(f.audios.length, 2, 'the page was left before the songs loaded');
});

test('playback that starts while the theme song loads keeps it from starting over the video', async () => {
    const f = themeSandbox();
    const loading = f.playThemeSong('m1');
    f.stopThemeSong(); // playItem's stop, before the theme songs answered
    await loading;
    assert.equal(f.audios.length, 0, 'no song starts after the stop');
    await f.playThemeSong('m1');
    assert.equal(f.audios.length, 1, 'a later page plays it again');
});

test('leaving item pages or starting playback stops the theme song', () => {
    const navigate = slice(router, 'function navigate(');
    assert.match(navigate, /if \(page !== 'detail'\) stopThemeSong\(\);/);
    assert.match(router, /import \{ stopThemeSong \} from '\.\/theme-media\.js';/);
    const playItem = slice(player, 'async function playItem(');
    assert.match(playItem, /stopPlayback\(\);\s*stopThemeSong\(\);/);
    assert.match(app, /import '\.\/modules\/theme-media\.js';/);
});

test('the display settings have "Play theme songs", saved as enableThemeSongs', () => {
    const display = slice(settings, 'function displaySettingsHtml(');
    assert.match(display, /t\('settings\.play_theme_songs'\), 'id="pref-theme-songs" data-pref="enableThemeSongs" onchange="changeUserPref\(this\)"', userSetting\('enableThemeSongs', 'false'\) === 'true'\)/);
});

test('every new string is translated in English and German', () => {
    const keys = [
        'detail.trailer', 'detail.special_features', 'detail.extra_behind_the_scenes', 'detail.extra_deleted_scene',
        'detail.extra_interview', 'detail.extra_scene', 'detail.extra_sample', 'detail.extra_short',
        'detail.extra_featurette', 'detail.extra_clip', 'settings.play_theme_songs',
    ];
    const get = (table, key) => key.split('.').reduce((o, k) => o?.[k], table);
    for (const key of keys) {
        assert.ok(get(en, key), `en has ${key}`);
        assert.ok(get(de, key), `de has ${key}`);
    }
    assert.ok(/EXTRA_TYPE_KEYS/.test(item));
});
