// R3 users 2: Jellyfin Web's backdrop screensaver: off by default, a per-user Display setting with an idle time,
// shown after that long with no input, never while anything plays, and gone at the first input. Runs
// modules/screensaver.js in a vm sandbox with a fake clock and DOM.
// Usage: node r3users2_screensaver.mjs (run by api_tests.rs r3users2_screensaver).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = source => source.replace(/^import [^;]*;$/gms, '').replace(/^export \{[^}]*\};$/gms, '');

function sandbox({ settings = {}, items = [], playing = null } = {}) {
    const clock = { now: 1_000_000 };
    const state = { playing, modalOpen: false, media: [], visibility: 'visible', loggedIn: true };
    const intervals = [];
    const listeners = {};
    const appended = [];
    const requests = [];
    const node = () => {
        const el = {
            className: '', innerHTML: '', id: '', removed: false, attrs: {},
            setAttribute: (k, v) => { el.attrs[k] = v; },
            appendChild: child => { appended.push(child); return child; },
            querySelector: () => ({ addEventListener() {} }),
            querySelectorAll: () => [],
            remove: () => { el.removed = true; },
        };
        return el;
    };
    const body = node();
    body.appendChild = child => { appended.push(child); return child; };
    const context = vm.createContext({
        URLSearchParams,
        Number,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/"/g, '&quot;'),
        userSetting: (name, fallback = null) => (name in settings ? settings[name] : fallback),
        SCREENSAVER_BACKDROP: 'backdropscreensaver',
        DEFAULT_IDLE_SECONDS: 180,
        nowPlaying: () => state.playing,
        Date: { now: () => clock.now },
        setInterval: (fn, ms) => { intervals.push({ fn, ms }); return intervals.length; },
        clearInterval: () => {},
        setTimeout: () => 0,
        document: {
            body,
            get visibilityState() { return state.visibility; },
            createElement: node,
            getElementById: id => (id === 'player-modal' ? { open: state.modalOpen } : null),
            querySelectorAll: () => state.media,
            addEventListener: (type, fn) => { (listeners[type] ||= []).push(fn); },
        },
        API: {
            userId: 'u1',
            isLoggedIn: () => state.loggedIn,
            imageUrl: (id, type, opts) => `/Items/${id}/Images/${type}?tag=${opts.tag}`,
            get: async path => { requests.push(path); return { Items: items }; },
        },
    });
    vm.runInContext(`${strip(read('modules/screensaver.js'))}
        this.fns = { idleSeconds, isPlaying, screensaverDue, showScreensaver, hideScreensaver, onInput, backdropItemsPath,
            isShown: () => shown !== null };`, context);
    const check = intervals.find(i => i.ms === 5000).fn;
    return { fns: context.fns, clock, state, listeners, appended, requests, check };
}

const ON = { screensaver: 'backdropscreensaver', screensaverTime: '60' };
const ITEMS = [{ Id: 'm1', Name: 'Alpha', BackdropImageTags: ['b1'] }, { Id: 'm2', Name: 'Beta', BackdropImageTags: ['b2'] }];

test('off by default: no screensaver however long the page is idle', () => {
    const box = sandbox();
    box.clock.now += 3600_000;
    assert.equal(box.fns.screensaverDue(), false);
    assert.equal(box.fns.idleSeconds(), 180, "Jellyfin Web's default idle time");
});

test('on: due after the idle time, not before', () => {
    const box = sandbox({ settings: ON });
    box.clock.now += 59_000;
    assert.equal(box.fns.screensaverDue(), false);
    box.clock.now += 1_000;
    assert.equal(box.fns.screensaverDue(), true);
});

test('never while anything plays, with the player open, signed out or in a hidden tab', () => {
    const cases = [
        s => { s.playing = { itemId: 'm1', streamType: 'Audio' }; },
        s => { s.modalOpen = true; },
        s => { s.media = [{ paused: false, ended: false }]; },
        s => { s.loggedIn = false; },
        s => { s.visibility = 'hidden'; },
    ];
    for (const change of cases) {
        const box = sandbox({ settings: ON });
        box.clock.now += 120_000;
        change(box.state);
        assert.equal(box.fns.screensaverDue(), false, change.toString());
    }
});

test('it shows random backdrops of movies, shows and artists, and the first input closes it', async () => {
    const box = sandbox({ settings: ON, items: ITEMS });
    box.clock.now += 61_000;
    box.check();
    await new Promise(resolve => setTimeout(resolve, 0));
    assert.equal(box.fns.isShown(), true);
    const url = new URL(box.requests[0], 'http://x');
    assert.equal(url.pathname, '/Users/u1/Items');
    assert.equal(url.searchParams.get('ImageTypes'), 'Backdrop');
    assert.equal(url.searchParams.get('SortBy'), 'Random');
    assert.equal(url.searchParams.get('IncludeItemTypes'), 'Movie,Series,MusicArtist');
    const overlay = box.appended.find(e => e.id === 'screensaver');
    assert.ok(overlay);
    assert.match(box.appended.find(e => e.className === 'jf-screensaver-slide').innerHTML, /Images\/Backdrop\/0\?tag=b1/);

    const key = { type: 'keydown', prevented: false, preventDefault() { this.prevented = true; }, stopPropagation() {} };
    box.fns.onInput(key);
    assert.equal(box.fns.isShown(), false);
    assert.ok(overlay.removed);
    assert.ok(key.prevented, 'the key that wakes the page does nothing else');
});

test('playback starting while it shows closes it; no backdrops means none', async () => {
    const box = sandbox({ settings: ON, items: ITEMS });
    box.clock.now += 61_000;
    await box.fns.showScreensaver();
    assert.equal(box.fns.isShown(), true);
    box.state.playing = { itemId: 'm1' };
    box.check();
    assert.equal(box.fns.isShown(), false);

    const empty = sandbox({ settings: ON, items: [] });
    empty.clock.now += 61_000;
    await empty.fns.showScreensaver();
    assert.equal(empty.fns.isShown(), false);
});

test('the Display settings offer it, off by default, kept per user on the device, in en and de', () => {
    const settings = read('modules/settings.js');
    assert.match(settings, /data-pref="screensaver"/);
    assert.match(settings, /data-pref="screensaverTime"/);
    assert.match(settings, /userSetting\('screensaver', 'none'\)/);
    const userSettings = read('modules/user-settings.js');
    assert.match(userSettings, /screensaver: true,/);
    assert.match(userSettings, /screensaverTime: true,/);
    assert.match(read('app.js'), /import '\.\/modules\/screensaver\.js';/);
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`)).settings;
        for (const key of ['screensaver', 'screensaver_none', 'screensaver_backdrop', 'screensaver_time', 'screensaver_minutes']) {
            assert.equal(typeof strings[key], 'string', `${lang}: settings.${key}`);
        }
    }
});
