// R3 settings: the settings Jellyfin Web keeps on the device (maximum streaming bitrate, subtitle
// burn-in, subtitle appearance) reach the player. The device's value wins, the account's value fills a
// device that has none, and a save on the settings page writes both. The player burns in the text
// subtitles the burn-in setting names and asks Universal for SubtitleMethod=Encode.
// Runs modules/playerprefs.js and modules/user-settings.js in a vm sandbox with localStorage stubbed.
// Usage: node r3settings_device.mjs (run by api_tests.rs r3settings_device).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = s => s.replace(/^import .*$/gm, '').replace(/^export \{[^}]*\};$/gm, '');
const source = strip([read('modules/playerprefs.js'), read('modules/user-settings.js')].join('\n'));

function storage(initial = {}) {
    const map = new Map(Object.entries(initial));
    return {
        map,
        getItem: k => (map.has(k) ? map.get(k) : null),
        setItem: (k, v) => map.set(k, String(v)),
        removeItem: k => map.delete(k),
    };
}

function sandbox({ device = {}, prefs = {} } = {}) {
    const localStorage = storage(device);
    const posts = [];
    const context = vm.createContext({
        localStorage,
        document: { documentElement: { classList: { toggle() {} } }, getElementById: () => null },
        window: {},
        API: {
            userId: 'u1',
            getMe: async () => ({ Id: 'u1', Configuration: {} }),
            get: async () => ({ Id: 'u1-usersettings', CustomPrefs: { ...prefs } }),
            post: async (url, body) => posts.push([url, JSON.parse(JSON.stringify(body))]),
        },
    });
    vm.runInContext(`${source}
        this.fns = { loadUserSettings, userSetting, saveUserSettings, maxStreamingBitrate, subtitleAppearance,
            subtitleBurnIn, burnsInSubtitle };`, context);
    return { fns: context.fns, localStorage, posts };
}

test('the account\'s value fills a device that has none; the player reads it there', async () => {
    const box = sandbox({ prefs: { maxStreamingBitrate: '4000000', subtitleburnin: 'all', localplayersubtitleappearance3: '{"textSize":"large"}' } });
    await box.fns.loadUserSettings();
    assert.equal(box.fns.maxStreamingBitrate(), 4000000);
    assert.equal(box.fns.subtitleBurnIn(), 'all');
    assert.equal(box.fns.subtitleAppearance().textSize, 'large');
    assert.equal(box.localStorage.getItem('u1-localplayersubtitleappearance3'), '{"textSize":"large"}', 'appearance is per user on the device');
});

test('the device\'s own value wins over the account\'s, as in Jellyfin Web', async () => {
    const box = sandbox({ device: { maxStreamingBitrate: '8000000' }, prefs: { maxStreamingBitrate: '4000000' } });
    await box.fns.loadUserSettings();
    assert.equal(box.fns.maxStreamingBitrate(), 8000000);
    assert.equal(box.fns.userSetting('maxStreamingBitrate'), '8000000', 'the settings page shows what this device plays');
});

test('a save on the settings page writes the account and the device; Auto clears both', async () => {
    const box = sandbox({ device: { maxStreamingBitrate: '8000000' } });
    await box.fns.loadUserSettings();
    await box.fns.saveUserSettings({ maxStreamingBitrate: '3000000', subtitleburnin: 'allcomplexformats' });
    assert.equal(box.posts.at(-1)[1].CustomPrefs.maxStreamingBitrate, '3000000');
    assert.equal(box.fns.maxStreamingBitrate(), 3000000);
    assert.equal(box.fns.subtitleBurnIn(), 'allcomplexformats');
    await box.fns.saveUserSettings({ maxStreamingBitrate: '' });
    assert.equal(box.posts.at(-1)[1].CustomPrefs.maxStreamingBitrate, undefined);
    assert.equal(box.fns.maxStreamingBitrate(), 0);
});

test('burn-in follows Jellyfin Web\'s modes', () => {
    const { fns } = sandbox();
    const burns = (codec, image, mode) => fns.burnsInSubtitle(codec, image, mode);
    for (const mode of ['', 'onlyimageformats']) {
        assert.equal(burns('subrip', false, mode), false, mode);
        assert.equal(burns('ass', false, mode), false, mode);
        assert.equal(burns('hdmv_pgs_subtitle', true, mode), true, mode);
    }
    assert.equal(burns('ass', false, 'allcomplexformats'), true);
    assert.equal(burns('SSA', false, 'allcomplexformats'), true);
    assert.equal(burns('subrip', false, 'allcomplexformats'), false);
    assert.equal(burns('subrip', false, 'all'), true);
});

test('the player burns in what the setting names and asks Universal for SubtitleMethod=Encode', async () => {
    const player = read('modules/player.js');
    // Every burn-in decision goes through burnsIn, which adds the device's setting to the picture formats
    assert.ok(!/session\.burnedIn = [^\n]*isImageSubtitle/.test(player));
    assert.match(player, /function burnsIn\(codec\) \{\n\s+return burnsInSubtitle\(codec, isImageSubtitle\(codec\)\);/);
    assert.match(player, /subtitleMethod: session\.burnedIn \? 'Encode' : undefined/);
    assert.match(player, /!stream \|\| burnsIn\(stream\.Codec\)\) return;/, 'a burned-in text subtitle is not drawn twice');

    // api.js passes it on
    const api = read('api.js');
    const start = api.indexOf('    async getUniversalVideoUrl(');
    const body = api.slice(start, api.indexOf('\n    },', start) + 6).replace(/^    async /, 'async function ').replace(/,$/, '');
    const urls = [];
    const context = vm.createContext({
        URLSearchParams,
        localStorage: storage(),
        self: { playableCodecs: () => ({ video: ['h264'], audio: ['aac'] }), get: async url => { urls.push(url); return { Url: '/x' }; } },
    });
    vm.runInContext(`${body.replace('async function getUniversalVideoUrl(', 'async function getUniversalVideoUrl(')}
        this.call = p => getUniversalVideoUrl.call(self, 'i1', p);`, context);
    await context.call({ subtitleStreamIndex: 3, subtitleMethod: 'Encode' });
    await context.call({ subtitleStreamIndex: 3 });
    assert.match(urls[0], /SubtitleStreamIndex=3&SubtitleMethod=Encode/);
    assert.ok(!urls[1].includes('SubtitleMethod'));
});
