// The player's chapters: a mark on the seek bar at each chapter start after the first, a chapters menu (two or
// more chapters) that seeks and marks the playing chapter, and the chapter's name over the seek preview.
// Runs player.js's chapter functions in a vm sandbox.
// Usage: node r3playercore_chapters.mjs (run by api_tests.rs r3playercore_chapters).
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

function sandbox() {
    const elements = {};
    const seeks = [];
    const make = () => ({
        hidden: false, open: false, children: [], style: {}, attrs: {}, className: '', textContent: '',
        replaceChildren(...c) { this.children = c; },
        append(...c) { this.children.push(...c); },
        setAttribute(k, v) { this.attrs[k] = v; },
    });
    const context = vm.createContext({
        $: id => (elements[id] ||= make()),
        t: (k, o) => (o ? `${k}|${JSON.stringify(o)}` : k),
        document: { createElement: make },
        formatDurationSeconds: s => `${Math.floor(s / 60)}:${String(Math.floor(s % 60)).padStart(2, '0')}`,
        seekTo: (session, seconds) => seeks.push(seconds),
        activePlayback: null,
    });
    vm.runInContext(`${['function chapterStart(', 'function chapterAt(', 'function renderChapters(', 'function renderChapterMenu(',
        'function playChapter('].map(slice).join('\n')}
        this.fns = { chapterAt, renderChapters, renderChapterMenu };
        this.setActive = s => { activePlayback = s; };`, context);
    return { context, elements, seeks };
}

const chapter = (Name, seconds) => ({ Name, StartPositionTicks: seconds * 1e7 });
const CHAPTERS = [chapter('Recap', 30), chapter('Intro', 0), chapter('', 90), chapter('Credits', 210)];
const session = (position = 0) => ({ chapters: CHAPTERS, total: () => 240, position: () => position });

test('the seek bar marks each chapter start after the first, in place', () => {
    const { context, elements } = sandbox();
    context.fns.renderChapters(session());
    assert.deepEqual(elements['player-chapter-marks'].children.map(m => m.style.left), ['12.5%', '37.5%', '87.5%']);
    assert.equal(elements['player-chapters'].hidden, false);
});

test('the menu lists the chapters in order, names the unnamed and seeks to a start', () => {
    const { context, elements, seeks } = sandbox();
    const s = session();
    context.setActive(s);
    context.fns.renderChapters(s);
    const items = elements['player-chapters-list'].children;
    assert.deepEqual(items.map(b => b.children.map(c => c.textContent)),
        [['Intro', '0:00'], ['Recap', '0:30'], ['player.chapter_number|{"number":3}', '1:30'], ['Credits', '3:30']]);
    elements['player-chapters'].open = true;
    items[2].onclick();
    assert.deepEqual(seeks, [90]);
    assert.equal(elements['player-chapters'].open, false, 'choosing closes the menu');
});

test('the menu marks the chapter playing now', () => {
    const { context, elements } = sandbox();
    const s = session(100);
    context.setActive(s);
    context.fns.renderChapters(s);
    context.fns.renderChapterMenu(true);
    assert.deepEqual(elements['player-chapters-list'].children.map(b => b.attrs['aria-current']), ['false', 'false', 'true', 'false']);
});

test('one chapter or none: no menu and no marks', () => {
    const { context, elements } = sandbox();
    context.fns.renderChapters({ chapters: [chapter('Only', 0)], total: () => 240 });
    assert.equal(elements['player-chapters'].hidden, true);
    assert.equal(elements['player-chapter-marks'].children.length, 0);
    context.fns.renderChapters({ chapters: CHAPTERS, total: () => 0 });
    assert.equal(elements['player-chapter-marks'].children.length, 0, 'no length yet: no marks');
});

test('chapterAt finds the chapter playing at a time; the preview shows its name', () => {
    const { context } = sandbox();
    assert.equal(context.fns.chapterAt(CHAPTERS, 45).Name, 'Recap');
    assert.equal(context.fns.chapterAt(CHAPTERS, 0).Name, 'Intro');
    assert.equal(context.fns.chapterAt([], 10), null);
    assert.match(slice('function showTrickplay('), /chapterAt\(session\.chapters, seconds\)/);
    assert.match(html, /id="player-chapter-marks"/);
    assert.match(html, /id="player-chapters"[^>]*ontoggle="renderChapterMenu\(this\.open\)"/);
});
