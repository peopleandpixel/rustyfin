// The web client's item page: the detail page's version, audio and subtitle choice and a chapter's start reach the
// player through playItem's `selection` option. Runs playItem's track and start choice (from the start
// position to the default tracks) in a vm sandbox with the player's own helpers, playWithResumeChoice and
// the router's play action with stubs.
// Usage: node r3item_play.mjs (run by api_tests.rs r3item_play).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const player = fs.readFileSync(new URL('modules/player.js', src), 'utf8');
const router = fs.readFileSync(new URL('modules/router.js', src), 'utf8');

function slice(text, head, end = '\n}\n') {
    const start = text.indexOf(head);
    assert.ok(start >= 0, `the client has ${head}`);
    return text.slice(start, text.indexOf(end, start) + end.length);
}

// playItem's choice of start, source and tracks, as one function of its inputs
const play = slice(player, 'async function playItem(');
const choiceStart = play.indexOf('    const startTicks = ');
const choiceEnd = play.indexOf('    // The session owns this playback');
assert.ok(choiceStart > 0 && choiceEnd > choiceStart, 'playItem has its track choice block');
const CHOICE = play.slice(choiceStart, choiceEnd);

// The track rules (playerprefs.js) with no user preferences: Jellyfin's defaults
const prefsModule = await import(new URL('modules/playerprefs.js', src));
const context = vm.createContext({
    defaultTracks: prefsModule.defaultTracks,
    preferredSubtitleIndex: prefsModule.defaultSubtitleIndex,
    prefs: prefsModule.trackPrefs(null),
});
vm.runInContext(
    `${['function resumePositionTicks(', 'function isStreamType(', 'function defaultSubtitleIndex(', 'function carriedTracks('].map(h => slice(player, h)).join('\n')}
     this.choose = (item, info, { streamType = 'Videos', fromStart = false, carry = null, selection = null } = {}) => {
         ${CHOICE}
         return { startTicks, sourceId: source?.Id ?? null, audioIndex, subtitleIndex };
     };`,
    context,
);
const choose = (...args) => JSON.parse(JSON.stringify(context.choose(...args)));

// PlaybackInfo's streams carry Jellyfin's Type and Rustyfin's lower case CodecType, which the player reads
const stream = (Index, Type, extra = {}) => ({ Index, Type, CodecType: Type.toLowerCase(), ...extra });
const source = (Id, extra = {}) => ({
    Id,
    MediaStreams: [
        stream(0, 'Video'),
        stream(1, 'Audio', { Language: 'eng', IsDefault: true }),
        stream(2, 'Audio', { Language: 'ger' }),
        stream(3, 'Subtitle', { Language: 'eng' }),
    ],
    DefaultAudioStreamIndex: 1,
    DefaultSubtitleStreamIndex: -1,
    ...extra,
});
const INFO = { MediaSources: [source('v1080'), source('v720')] };
const ITEM = { RunTimeTicks: 1e10, UserData: { PlaybackPositionTicks: 2e9 } };

test('without a selection playItem keeps its defaults', () => {
    assert.deepEqual(choose(ITEM, INFO), { startTicks: 2e9, sourceId: 'v1080', audioIndex: 1, subtitleIndex: -1 });
    assert.deepEqual(choose(ITEM, INFO, { fromStart: true }).startTicks, 0);
});

test('the selection picks the version, audio and subtitle track', () => {
    assert.deepEqual(choose(ITEM, INFO, { selection: { mediaSourceId: 'v720', audioIndex: 2, subtitleIndex: 3 } }),
        { startTicks: 2e9, sourceId: 'v720', audioIndex: 2, subtitleIndex: 3 });
    assert.deepEqual(choose(ITEM, { MediaSources: [source('a', { DefaultSubtitleStreamIndex: 3 })] }, { selection: { subtitleIndex: -1 } }).subtitleIndex,
        -1, 'Off wins over the server default');
});

test('a selection that does not fit the item falls back to the defaults', () => {
    assert.deepEqual(choose(ITEM, INFO, { selection: { mediaSourceId: 'gone', audioIndex: 9, subtitleIndex: 7 } }),
        { startTicks: 2e9, sourceId: 'v1080', audioIndex: 1, subtitleIndex: -1 });
    assert.deepEqual(choose(ITEM, null, { selection: { audioIndex: 2 } }),
        { startTicks: 2e9, sourceId: null, audioIndex: null, subtitleIndex: null }, 'no PlaybackInfo, no tracks');
});

test("a chapter's start wins over the resume point and from-start", () => {
    assert.equal(choose(ITEM, INFO, { selection: { startTicks: 5e8 } }).startTicks, 5e8);
    assert.equal(choose(ITEM, INFO, { fromStart: true, selection: { startTicks: 5e8 } }).startTicks, 5e8);
    assert.equal(choose(ITEM, INFO, { selection: { startTicks: 0 } }).startTicks, 0, 'the first chapter starts at 0');
});

test('the detail page buttons pass the selection; Play still asks to resume; other play buttons are unchanged', () => {
    const action = slice(router, '    play: el => {', '\n    },\n');
    const calls = [];
    const run = (dataset, selection = { mediaSourceId: 'v720' }) => {
        calls.length = 0;
        const ctx = vm.createContext({
            playItem: (...args) => calls.push(['playItem', ...args]),
            playWithResumeChoice: (...args) => calls.push(['ask', ...args]),
            detailSelection: () => (selection ? { ...selection } : null),
        });
        vm.runInContext(`const actions = {${action}}; actions.play({ dataset: ${JSON.stringify(dataset)} });`, ctx);
        return JSON.parse(JSON.stringify(calls));
    };
    const base = { id: 'kilo', streamType: 'Videos', title: 'Kilo' };
    const chosen = { mediaSourceId: 'v720' };
    assert.deepEqual(run({ ...base, detailTracks: 'true' }), [['ask', 'kilo', 'Videos', 'Kilo', chosen]],
        "the page's Play button asks from a fresh resume point, with the choice");
    assert.deepEqual(run({ ...base, detailTracks: 'true', resume: 'true' }), [['playItem', 'kilo', 'Videos', 'Kilo', { selection: chosen }]]);
    assert.deepEqual(run({ ...base, detailTracks: 'true', fromStart: 'true' }),
        [['playItem', 'kilo', 'Videos', 'Kilo', { fromStart: true, selection: chosen }]]);
    assert.deepEqual(run({ ...base, detailTracks: 'true', startTicks: '100000000' }, null),
        [['playItem', 'kilo', 'Videos', 'Kilo', { selection: { startTicks: 100000000 } }]], 'a chapter starts where it starts');
    assert.deepEqual(run({ ...base, detailTracks: 'true', startTicks: '0' }),
        [['playItem', 'kilo', 'Videos', 'Kilo', { selection: { ...chosen, startTicks: 0 } }]]);
    assert.deepEqual(run(base), [['ask', 'kilo', 'Videos', 'Kilo', null]], 'a card still asks to resume');
    assert.deepEqual(run({ ...base, resume: 'true' }), [['playItem', 'kilo', 'Videos', 'Kilo', { selection: null }]]);
});

test('playWithResumeChoice reads the resume point uncached and passes the selection on', async () => {
    const run = async (item, answer) => {
        const calls = [];
        const ctx = vm.createContext({
            pendingStop: Promise.resolve(),
            console: { warn: () => {} },
            API: { get: path => { calls.push(['get', path]); return Promise.resolve(item); } },
            askResumeOrStart: (title, ticks) => { calls.push(['ask', title, ticks]); return Promise.resolve(answer); },
            playItem: (...args) => calls.push(['playItem', ...args]),
        });
        vm.runInContext(`${slice(player, 'function resumePositionTicks(')}
            ${slice(player, 'async function playWithResumeChoice(')}
            this.go = playWithResumeChoice;`, ctx);
        await ctx.go('kilo', 'Videos', 'Kilo', { mediaSourceId: 'v720' });
        return JSON.parse(JSON.stringify(calls));
    };
    const sel = { selection: { mediaSourceId: 'v720' } };
    assert.deepEqual(await run(ITEM, 'resume'), [
        ['get', '/Items/kilo'],
        ['ask', 'Kilo', 2e9],
        ['playItem', 'kilo', 'Videos', 'Kilo', sel],
    ]);
    assert.deepEqual((await run(ITEM, 'start')).at(-1), ['playItem', 'kilo', 'Videos', 'Kilo', { fromStart: true, ...sel }]);
    assert.deepEqual(await run({ RunTimeTicks: 1e10 }, ''), [['get', '/Items/kilo'], ['playItem', 'kilo', 'Videos', 'Kilo', sel]],
        'no resume point: plays at once');
    assert.equal((await run(ITEM, '')).length, 2, 'a dismissed question plays nothing');
});
