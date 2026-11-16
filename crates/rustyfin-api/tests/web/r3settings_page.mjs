// R3 settings: the user settings page saves each setting where Jellyfin keeps it. The playback and
// subtitle fields of Jellyfin's UserConfiguration go to /Users/{id}/Configuration with the other
// fields kept; Jellyfin Web's userSettings (bitrate, burn-in, subtitle appearance, home sections,
// date format, backdrops) go to the CustomPrefs of the "usersettings" display preferences. A failed
// save puts the control back. Runs modules/user-settings.js and modules/settings.js in a vm sandbox.
// Usage: node r3settings_page.mjs (run by api_tests.rs r3settings_page).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const source = [read('modules/playerprefs.js'), read('modules/user-settings.js'), read('modules/settings.js')].join('\n')
    .replace(/^import \{[^}]*\} from .*$/gm, '')
    .replace(/^import .*$/gm, '')
    .replace(/^export \{[^}]*\};$/gm, '');
const PREFS_URL = '/DisplayPreferences/usersettings?userId=u1&client=emby';

const plain = v => JSON.parse(JSON.stringify(v));

function sandbox({ config = {}, prefs = {}, post } = {}) {
    const log = [];
    const keys = new Set();
    let me = { Id: 'u1', Name: 'Ann', Configuration: config };
    let stored = { Id: 'u1-usersettings', SortBy: 'SortName', CustomPrefs: prefs };
    const context = vm.createContext({
        t: (key, opts) => {
            keys.add(key);
            return opts ? `${key}|${JSON.stringify(opts)}` : key;
        },
        uiLocale: () => 'en-GB',
        escapeHtml: s => String(s),
        $: () => null,
        toast: (msg, kind) => log.push(['toast', msg, kind || 'info']),
        showOidcLinkResult() {},
        oidcCardHtml: () => '',
        document: { documentElement: { classList: { toggle() {} } }, getElementById: () => null },
        window: {},
        Intl,
        Date,
        API: {
            userId: 'u1',
            token: 'tok',
            getMe: async () => structuredClone(me),
            get: async url => {
                assert.equal(url, PREFS_URL);
                return structuredClone(stored);
            },
            post: async (url, body) => {
                log.push(['post', url, structuredClone(body)]);
                if (post) return post(url, body);
                if (url === PREFS_URL) stored = structuredClone(body);
                else me = { ...me, Configuration: structuredClone(body) };
                return {};
            },
        },
    });
    vm.runInContext(`${source}
        this.fns = { loadUserSettings, userSetting, changeUserConfig, changeUserPref, changeHomeSection, changeSubtitleAppearance,
            changeLatestLibrary, homeSettingsHtml, playbackSettingsHtml, subtitleSettingsHtml, displaySettingsHtml };`, context);
    return { fns: context.fns, log, keys, posts: () => plain(log.filter(e => e[0] === 'post')) };
}

const select = (value, dataset) => ({ type: 'select-one', value, disabled: false, dataset: { saved: 'old', ...dataset } });
const toggle = (checked, dataset) => ({ type: 'checkbox', checked, disabled: false, dataset: { saved: String(!checked), ...dataset } });

test('a UserConfiguration field saves over the current configuration, a switch as a boolean', async () => {
    const box = sandbox({ config: { SubtitleMode: 'Always', EnableNextEpisodeAutoPlay: false } });
    await box.fns.loadUserSettings();
    await box.fns.changeUserConfig(select('ger', { config: 'AudioLanguagePreference' }));
    await box.fns.changeUserConfig(toggle(false, { config: 'RememberSubtitleSelections' }));
    assert.deepEqual(box.posts(), [
        ['post', '/Users/u1/Configuration', { SubtitleMode: 'Always', EnableNextEpisodeAutoPlay: false, AudioLanguagePreference: 'ger' }],
        ['post', '/Users/u1/Configuration', { SubtitleMode: 'Always', EnableNextEpisodeAutoPlay: false, AudioLanguagePreference: 'ger', RememberSubtitleSelections: false }],
    ]);
    assert.deepEqual(box.log.at(-1), ['toast', 'settings.saved', 'info']);
});

test('a userSettings value saves into the usersettings CustomPrefs, keeping the others; empty removes it', async () => {
    const box = sandbox({ prefs: { homesection0: 'resume' } });
    await box.fns.loadUserSettings();
    await box.fns.changeUserPref(select('4000000', { pref: 'maxStreamingBitrate' }));
    let [, url, body] = box.posts().at(-1);
    assert.equal(url, PREFS_URL);
    assert.equal(body.SortBy, 'SortName', 'the rest of the display preferences is sent back');
    assert.deepEqual(body.CustomPrefs, { homesection0: 'resume', maxStreamingBitrate: '4000000' });
    assert.equal(box.fns.userSetting('maxStreamingBitrate'), '4000000');

    await box.fns.changeUserPref(select('', { pref: 'maxStreamingBitrate' }));
    [, , body] = box.posts().at(-1);
    assert.deepEqual(body.CustomPrefs, { homesection0: 'resume' });

    await box.fns.changeUserPref(toggle(false, { pref: 'enableBackdrops' }));
    assert.equal(box.fns.userSetting('enableBackdrops'), 'false');
});

test('a home section slot saves all ten slots, as Jellyfin Web does', async () => {
    const box = sandbox({ prefs: { homesection3: 'livetv' } });
    await box.fns.loadUserSettings();
    await box.fns.changeHomeSection(select('latestmedia', { homeSlot: '0' }));
    const [, , body] = box.posts().at(-1);
    assert.deepEqual(body.CustomPrefs, {
        homesection0: 'latestmedia', homesection1: 'resume', homesection2: 'resumeaudio', homesection3: 'livetv',
        homesection4: 'livetv', homesection5: 'nextup', homesection6: 'latestmedia', homesection7: 'none',
        homesection8: 'none', homesection9: 'none',
    });
    const html = box.fns.homeSettingsHtml({}, []);
    assert.equal((html.match(/data-home-slot=/g) || []).length, 10);
    assert.match(html, /<option value="livetv" selected>settings\.home_jellyfin_only/);
});

test('subtitle appearance is one JSON value under Jellyfin Web\'s key', async () => {
    const box = sandbox({ prefs: { localplayersubtitleappearance3: '{"font":"print"}' } });
    await box.fns.loadUserSettings();
    await box.fns.changeSubtitleAppearance(select('large', { appearance: 'textSize' }));
    const [, , body] = box.posts().at(-1);
    assert.deepEqual(JSON.parse(body.CustomPrefs.localplayersubtitleappearance3), { font: 'print', textSize: 'large' });
});

test('a library\'s Recently Added switch edits LatestItemsExcludes', async () => {
    const box = sandbox({ config: { LatestItemsExcludes: ['other', 'films-1'] } });
    await box.fns.loadUserSettings();
    await box.fns.changeLatestLibrary(toggle(true, { latestLibrary: 'FILMS1' }));
    assert.deepEqual(box.posts().at(-1)[2].LatestItemsExcludes, ['other']);
    await box.fns.changeLatestLibrary(toggle(false, { latestLibrary: 'shows' }));
    assert.deepEqual(box.posts().at(-1)[2].LatestItemsExcludes, ['other', 'shows']);
});

test('a failed save puts the control back and says why', async () => {
    const box = sandbox({ post: () => Promise.reject(new Error('nope')) });
    await box.fns.loadUserSettings();
    const list = select('Always', { config: 'SubtitleMode', saved: 'Smart' });
    await box.fns.changeUserConfig(list);
    assert.equal(list.value, 'Smart');
    assert.equal(list.disabled, false);
    const sw = toggle(false, { pref: 'enableBackdrops' });
    await box.fns.changeUserPref(sw);
    assert.equal(sw.checked, true);
    assert.deepEqual(box.log.at(-1), ['toast', 'nope', 'error']);
});

test('the cards show the saved values', async () => {
    const config = { AudioLanguagePreference: 'ger', PlayDefaultAudioTrack: false, SubtitleMode: 'OnlyForced', LatestItemsExcludes: ['l2'] };
    const box = sandbox({ config, prefs: { maxStreamingBitrate: '8000000', subtitleburnin: 'all', homesection1: 'none', datetimelocale: 'de-DE' } });
    await box.fns.loadUserSettings();
    const cultures = [{ ThreeLetterISOLanguageName: 'ger', DisplayName: 'German' }, { ThreeLetterISOLanguageName: 'eng', DisplayName: 'English' }];
    const playback = box.fns.playbackSettingsHtml(config, cultures);
    assert.match(playback, /<option value="ger" selected>German<\/option>/);
    assert.match(playback, /<option value="8000000" selected>8 Mbps<\/option>/);
    assert.match(playback, /data-config="PlayDefaultAudioTrack"[^>]*data-saved="false" >/);
    const subtitles = box.fns.subtitleSettingsHtml(config, cultures);
    assert.match(subtitles, /<option value="OnlyForced" selected>/);
    assert.match(subtitles, /<option value="all" selected>/);
    const home = box.fns.homeSettingsHtml(config, [{ Id: 'l1', Name: 'Films' }, { Id: 'l2', Name: 'Shows' }]);
    assert.match(home, /data-home-slot="1"[^>]*>[^]*?<option value="none" selected>/);
    assert.match(home, /data-latest-library="l1"[^>]*data-saved="true" checked>/);
    assert.match(home, /data-latest-library="l2"[^>]*data-saved="false" >/);
    assert.match(box.fns.displaySettingsHtml(), /<option value="de-DE" selected>/);
});

test('every string the cards use is in English and German', async () => {
    const box = sandbox();
    await box.fns.loadUserSettings();
    box.fns.displaySettingsHtml();
    box.fns.homeSettingsHtml({}, [{ Id: 'l1', Name: 'Films' }]);
    box.fns.playbackSettingsHtml({}, []);
    box.fns.subtitleSettingsHtml({}, []);
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`));
        for (const key of box.keys) {
            const value = key.split('.').reduce((o, k) => o?.[k], strings);
            assert.equal(typeof value, 'string', `${lang}: ${key}`);
        }
    }
    assert.ok(box.keys.size > 60);
});
