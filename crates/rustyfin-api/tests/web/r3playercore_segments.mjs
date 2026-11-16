// The player's media segment actions, per type as Jellyfin Web keeps them (segmentTypeAction__<type> per user on
// the device): Skip seeks past a segment once per playback, AskToSkip shows the Skip button, None does nothing.
// Loads playerprefs.js as a module and runs player.js's segment functions in a vm sandbox.
// Usage: node r3playercore_segments.mjs (run by api_tests.rs r3playercore_segments).
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
function decl(name) {
    const m = new RegExp(`^const ${name} = [\\s\\S]*?;$`, 'm').exec(playerJs);
    assert.ok(m, `player.js declares ${name}`);
    return m[0];
}

const store = new Map();
globalThis.localStorage = {
    getItem: k => (store.has(k) ? store.get(k) : null),
    setItem: (k, v) => store.set(k, String(v)),
    removeItem: k => store.delete(k),
};
globalThis.API = { userId: 'u1' };
const prefs = await import(new URL('modules/playerprefs.js', src));

test('defaults and storage follow Jellyfin Web', () => {
    store.clear();
    assert.deepEqual(prefs.SEGMENT_TYPES.map(prefs.segmentAction), ['AskToSkip', 'AskToSkip', 'None', 'None', 'None']);
    prefs.setSegmentAction('Recap', 'Skip');
    assert.equal(store.get('u1-segmentTypeAction__Recap'), 'Skip');
    assert.equal(prefs.segmentAction('Recap'), 'Skip');
    prefs.setSegmentAction('Recap', 'Bogus');
    prefs.setSegmentAction('Nonsense', 'Skip');
    assert.equal(prefs.segmentAction('Recap'), 'Skip', 'an unknown action is ignored');
    assert.equal(store.has('u1-segmentTypeAction__Nonsense'), false);
});

const seg = (Type, start, end) => ({ Type, StartTicks: start * 1e7, EndTicks: end * 1e7 });
const SEGMENTS = [seg('Recap', 0, 30), seg('Intro', 30, 60), seg('Commercial', 100, 130), seg('Outro', 210, 240)];

function sandbox() {
    const elements = {};
    const seeks = [];
    const osd = [];
    const context = vm.createContext({
        $: id => (elements[id] ||= { hidden: true, textContent: '', value: '', classList: { add() {}, remove() {} } }),
        t: (k, o) => (o ? `${k}|${JSON.stringify(o)}` : k),
        setTimeout: () => 1,
        clearTimeout: () => {},
        seekTo: (s, to) => seeks.push(to),
        showPlayerOsd: text => osd.push(text),
        SEGMENT_TYPES: prefs.SEGMENT_TYPES,
        segmentAction: prefs.segmentAction,
        setSegmentAction: prefs.setSegmentAction,
        activePlayback: null,
        episodeNav: null,
    });
    vm.runInContext(`${decl('SKIP_PROMPT_MS')} let skipPrompt = null; ${decl('segmentStart')} ${decl('segmentEnd')}
        ${['function skippableSegment(', 'async function checkSkipSegment(', 'function showSkipSegment(', 'function hideSkipSegment(',
        'function segmentTypeName(', 'function renderSegmentSettings(', 'function changeSegmentAction('].map(slice).join('\n')}
        this.fns = { skippableSegment, checkSkipSegment, renderSegmentSettings, changeSegmentAction };
        this.setActive = s => { activePlayback = s; };`, context);
    return { context, elements, seeks, osd };
}

const session = position => ({ streamType: 'Videos', segmentsLoad: Promise.resolve(SEGMENTS), position: () => position, total: () => 240 });

test('only types with an action are offered', () => {
    store.clear();
    const { context } = sandbox();
    assert.equal(context.fns.skippableSegment(SEGMENTS, 10, 240, false), null, 'recaps: nothing by default');
    assert.equal(context.fns.skippableSegment(SEGMENTS, 40, 240, false).Type, 'Intro');
    prefs.setSegmentAction('Intro', 'None');
    prefs.setSegmentAction('Commercial', 'AskToSkip');
    assert.equal(context.fns.skippableSegment(SEGMENTS, 40, 240, false), null);
    assert.equal(context.fns.skippableSegment(SEGMENTS, 110, 240, false).Type, 'Commercial');
});

test('AskToSkip shows the button with the type\'s name', async () => {
    store.clear();
    const { context, elements, seeks } = sandbox();
    const s = session(40);
    context.setActive(s);
    await context.fns.checkSkipSegment(s);
    assert.equal(elements['player-skip-segment'].hidden, false);
    assert.equal(elements['player-skip-segment'].textContent, 'player.skip_segment|{"type":"player.segment_intro"}');
    assert.deepEqual(seeks, []);
});

test('Skip seeks past the segment once per playback and says so', async () => {
    store.clear();
    prefs.setSegmentAction('Recap', 'Skip');
    const { context, elements, seeks, osd } = sandbox();
    const s = session(2);
    context.setActive(s);
    await context.fns.checkSkipSegment(s);
    assert.deepEqual(seeks, [30]);
    assert.deepEqual(osd, ['player.segment_skipped|{"type":"player.segment_recap"}']);
    assert.equal(elements['player-skip-segment'].hidden, true, 'no button');
    // Back in the recap (the user sought there): it plays
    await context.fns.checkSkipSegment(s);
    assert.deepEqual(seeks, [30]);
});

test('the menu shows and changes each type\'s action', () => {
    store.clear();
    const { context, elements } = sandbox();
    context.fns.renderSegmentSettings();
    assert.equal(elements['player-segment-intro'].value, 'AskToSkip');
    assert.equal(elements['player-segment-commercial'].value, 'None');
    context.fns.changeSegmentAction('Outro', 'Skip');
    assert.equal(prefs.segmentAction('Outro'), 'Skip');
    for (const type of prefs.SEGMENT_TYPES) {
        assert.match(html, new RegExp(`id="player-segment-${type.toLowerCase()}"[^>]*changeSegmentAction\\('${type}', this\\.value\\)`));
    }
});
