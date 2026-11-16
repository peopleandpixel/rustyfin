// "Are you still watching?", as Jellyfin Web's stillWatching plugin: off by default; once on, before the next
// episode the player asks when the run has lasted the setting's time and the user has been idle that long or has
// played its number of episodes. Continue starts a new run; Stop closes the player.
// Loads playerprefs.js as a module and runs player.js's functions in a vm sandbox.
// Usage: node r3playercore_still_watching.mjs (run by api_tests.rs r3playercore_still_watching).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const playerJs = fs.readFileSync(new URL('modules/player.js', src), 'utf8');
const html = fs.readFileSync(new URL('index.html', src), 'utf8');

function slice(head) {
    const start = playerJs.indexOf(head);
    assert.ok(start >= 0, `player.js has ${head}`);
    return playerJs.slice(start, playerJs.indexOf('\n}\n', start) + 3);
}

const store = new Map();
globalThis.localStorage = {
    getItem: k => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: k => store.delete(k),
};
globalThis.API = { userId: 'u1' };
const prefs = await import(new URL('modules/playerprefs.js', src));
const H = 3600e3;

test('the setting is off by default and stored per user under Jellyfin Web\'s key', () => {
    store.clear();
    assert.equal(prefs.stillWatchingSetting(), 'disabled');
    prefs.setStillWatchingSetting('short');
    assert.equal(store.get('u1-stillWatchingPrompt'), 'short');
    prefs.setStillWatchingSetting('disabled');
    assert.equal(store.has('u1-stillWatchingPrompt'), false);
});

test('the rule: the run lasted the time, and idle that long or enough episodes played', () => {
    const at = (minutes, idleMinutes, played) => ({ now: minutes * 60e3, sessionStart: 0, lastInput: (minutes - idleMinutes) * 60e3, played });
    assert.equal(prefs.stillWatchingDue('disabled', at(600, 600, 20)), false);
    assert.equal(prefs.stillWatchingDue('short', at(59, 59, 5)), false, 'under an hour: never');
    assert.equal(prefs.stillWatchingDue('short', at(61, 1, 2)), true, 'two episodes in over an hour');
    assert.equal(prefs.stillWatchingDue('short', at(61, 1, 1)), false, 'one episode, active');
    assert.equal(prefs.stillWatchingDue('short', at(61, 60, 1)), true, 'idle for an hour');
    assert.equal(prefs.stillWatchingDue('default', at(91, 1, 3)), true);
    assert.equal(prefs.stillWatchingDue('veryLong', at(4 * 60 + 1, 1, 7)), false);
    assert.equal(prefs.STILL_WATCHING.long.duration, 2.5 * H);
});

function sandbox(setting = 'short') {
    const log = [];
    let resolveAsk;
    const dialog = { returnValue: '', listeners: {}, addEventListener(type, fn) { this.listeners[type] = fn; }, showModal() { log.push('ask'); } };
    const context = vm.createContext({
        $: id => (id === 'still-watching' ? dialog : id === 'video-player' ? { pause: () => log.push('pause') } : null),
        document: { addEventListener: () => {} },
        Date: { now: () => context.now },
        now: 0,
        stillWatchingSetting: () => setting,
        stillWatchingDue: prefs.stillWatchingDue,
        closePlayer: () => log.push('close'),
        episodeNav: null,
        playItem: (id, type, name, opts) => log.push(`play ${id}`),
        episodeTrackCarry: () => ({}),
        playQueueStep: () => {},
    });
    vm.runInContext(`let watchRun = { start: Date.now(), played: new Set() }; let lastInput = Date.now(); let stillWatchingAsking = false;
        ${['function trackStillWatching(', 'function askStillWatching(', 'function playEpisode('].map(slice).join('\n')}
        this.fns = { trackStillWatching, playEpisode };
        this.run = () => watchRun; this.input = t => { lastInput = t; };`, context);
    const answer = value => { dialog.returnValue = value; dialog.listeners.close(); };
    return { context, log, answer };
}

const EP = id => ({ Id: id, Type: 'Episode', Name: id });
const flush = () => new Promise(r => setImmediate(r));

test('a user-started episode begins a run; played-on ones count', () => {
    const { context } = sandbox();
    context.now = 1000;
    context.fns.trackStillWatching(EP('e1'), false);
    context.fns.trackStillWatching(EP('e2'), true);
    assert.equal(context.run().played.size, 2);
    assert.equal(context.run().start, 1000);
    context.fns.trackStillWatching({ Id: 'm', Type: 'Movie' }, false);
    assert.equal(context.run().played.size, 2, 'films leave the run alone');
    context.fns.trackStillWatching(EP('e9'), false);
    assert.equal(context.run().played.size, 1, 'a new run');
});

test('due: the player pauses and asks; Continue plays on with a new run', async () => {
    const { context, log, answer } = sandbox();
    context.fns.trackStillWatching(EP('e1'), false);
    context.fns.trackStillWatching(EP('e2'), true);
    context.now = 61 * 60e3;
    context.input(context.now);
    context.fns.playEpisode({ streamType: 'Videos' }, EP('e3'), { autoplay: true });
    assert.deepEqual(log, ['pause', 'ask']);
    context.fns.playEpisode({ streamType: 'Videos' }, EP('e3'));
    assert.deepEqual(log, ['pause', 'ask'], 'a second next while the prompt is open does nothing');
    answer('continue');
    await flush();
    assert.deepEqual(log, ['pause', 'ask', 'play e3']);
    assert.equal(context.run().start, context.now);
});

test('Stop (or Escape) closes the player; not due plays straight on', async () => {
    const { context, log, answer } = sandbox();
    context.fns.trackStillWatching(EP('e1'), false);
    context.now = 61 * 60e3;
    context.fns.playEpisode({ streamType: 'Videos' }, EP('e2'), { autoplay: true });
    answer('');
    await flush();
    assert.deepEqual(log, ['pause', 'ask', 'close'], 'idle for an hour: asked');
    const fresh = sandbox();
    fresh.context.fns.trackStillWatching(EP('e1'), false);
    fresh.context.now = 10 * 60e3;
    fresh.context.fns.playEpisode({ streamType: 'Videos' }, EP('e2'), { autoplay: true });
    assert.deepEqual(fresh.log, ['play e2']);
    const off = sandbox('disabled');
    off.context.now = 10 * H;
    off.context.fns.playEpisode({ streamType: 'Videos' }, EP('e2'), { autoplay: true });
    assert.deepEqual(off.log, ['play e2']);
});

test('a manual next (button, key, Play now) never asks', () => {
    const { context, log } = sandbox();
    context.fns.trackStillWatching(EP('e1'), false);
    context.now = 10 * H;
    context.fns.playEpisode({ streamType: 'Videos' }, EP('e2'));
    assert.deepEqual(log, ['play e2']);
    assert.match(slice('function upNextTick('), /playUpNextNow\(true\)/);
    assert.match(slice('function playUpNextNow('), /autoplay: autoplay === true/);
});

test('the wiring: playItem tracks the run, the menu and the dialog exist', () => {
    assert.match(slice('async function playItem('), /trackStillWatching\(item, !!carry\)/);
    assert.match(html, /id="player-still-watching"[^>]*changeStillWatching\(this\.value\)/);
    assert.match(html, /<dialog id="still-watching"/);
    for (const value of Object.keys(prefs.STILL_WATCHING)) assert.match(html, new RegExp(`<option value="${value}"`));
});
