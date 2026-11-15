// B170: the player's top bar (title, back and close) hides and shows with the bottom controls,
// after 3 s without input while the video plays, and the cursor hides with them.
// Reads index.html and input.css next to app.js as text: the top bar must take the same
// opacity and transition rules as the controls, and carry no opacity or transition of its own.
// Runs playerAutoHide in a vm sandbox with a fake clock and a fake player.
// Usage: node b170_player_header_autohide.mjs PATH/TO/app.js (run by api_tests.rs b170_player_header_autohide).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

const appPath = process.argv[2];
const srcDir = path.dirname(appPath);
const app = fs.readFileSync(appPath, 'utf8');
const html = fs.readFileSync(path.join(srcDir, 'index.html'), 'utf8');
const css = fs.readFileSync(path.join(srcDir, 'input.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');

// Innermost CSS blocks: [selectors, { property: value }].
function rules() {
    const out = [];
    for (const m of css.matchAll(/([^{}]+)\{([^{}]*)\}/g)) {
        const selectors = m[1].split(',').map(s => s.trim().replace(/\s+/g, ' ')).filter(Boolean);
        const decls = {};
        for (const d of m[2].split(';')) {
            const i = d.indexOf(':');
            if (i > 0) decls[d.slice(0, i).trim()] = d.slice(i + 1).trim();
        }
        out.push([selectors, decls]);
    }
    return out;
}

// The opacity and transition each selector gets, keyed by selector.
function hideRules(cls) {
    const found = {};
    for (const [selectors, decls] of rules()) {
        for (const sel of selectors) {
            if (!sel.includes(`.${cls}`)) continue;
            for (const prop of ['opacity', 'transition']) {
                if (prop in decls) found[`${sel.replaceAll(cls, 'X')} ${prop}`] = decls[prop];
            }
        }
    }
    return found;
}

function elementClasses(id) {
    const m = html.match(new RegExp(`<div id="${id}" class="([^"]*)"`));
    assert.ok(m, `index.html has #${id}`);
    return m[1].split(/\s+/);
}

test('the player opens as a modal dialog, so its own top bar is the header over the video', () => {
    assert.match(app, /modal\.showModal\(\)/);
    const container = html.indexOf('jf-player-container');
    assert.ok(container > html.indexOf('id="player-modal"'), 'container inside #player-modal');
    assert.ok(html.indexOf('id="player-topbar"') > container, 'top bar inside the container');
    assert.ok(html.indexOf('id="player-controls"') > container, 'controls inside the container');
});

test('the top bar carries the shared auto-hide class and no opacity or transition utilities', () => {
    const classes = elementClasses('player-topbar');
    assert.ok(classes.includes('jf-player-topbar'), classes.join(' '));
    const own = classes.filter(c => /^(opacity-|transition|duration-|ease-|delay-)/.test(c));
    assert.deepEqual(own, []);
    assert.ok(elementClasses('player-controls').includes('jf-player-controls'));
});

test('the top bar hides, shows and fades exactly like the controls', () => {
    const controls = hideRules('jf-player-controls');
    const topbar = hideRules('jf-player-topbar');
    assert.equal(controls['.X opacity'], undefined, 'controls show by default');
    assert.equal(controls['.X transition'], 'opacity 0.3s ease');
    assert.equal(controls['.jf-player-idle .X opacity'], '0', 'controls hide when idle');
    assert.deepEqual(topbar, controls);
});

test('the cursor hides on the idle player only, and the video area routes clicks through playerAreaClick', () => {
    const cursor = rules().find(([selectors, decls]) => decls.cursor === 'none' && selectors.includes('.jf-player-container.jf-player-idle'));
    assert.ok(cursor, 'cursor: none rule for .jf-player-container.jf-player-idle');
    assert.ok(cursor[0].every(s => s.startsWith('.jf-player-container.jf-player-idle')), cursor[0].join(', '));
    assert.match(html, /onclick="playerAreaClick\(event\)"/);
    assert.match(app, /playerBars = playerAutoHide\(\$\('player-modal'\), \$\('video-player'\)\)/);
    assert.match(app, /addEventListener\('close', \(\) => \{\s*stopPlayback\(\);\s*playerBars\?\.reset\(\);/);
});

function slice(head) {
    const start = app.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return app.slice(start, app.indexOf('\n}\n', start) + 3);
}

class ClassList {
    constructor() { this.set = new Set(); }
    add(c) { this.set.add(c); }
    remove(c) { this.set.delete(c); }
    contains(c) { return this.set.has(c); }
}

function target() {
    const handlers = {};
    return {
        handlers,
        addEventListener(type, fn) { (handlers[type] ||= []).push(fn); },
        fire(type, e = {}) { for (const fn of handlers[type] || []) fn({ type, ...e }); },
    };
}

// A fake clock, a playing video and a player whose bars are not hovered
function sandbox() {
    let now = 0;
    let nextId = 1;
    const timers = new Map();
    const state = { barHovered: false, menuExpanded: false };
    const container = Object.assign(target(), {
        classList: new ClassList(),
        querySelector(sel) {
            if (state.barHovered && sel.includes(':hover')) return {};
            if (state.menuExpanded && sel.includes('[aria-expanded="true"]')) return {};
            return null;
        },
    });
    const modal = { open: true, querySelector: sel => (sel === '.jf-player-container' ? container : null) };
    const video = Object.assign(target(), { paused: false, ended: false, seeking: false, readyState: 4 });
    const doc = target();
    const context = vm.createContext({
        document: doc,
        setTimeout: (fn, ms) => { timers.set(nextId, { fn, at: now + ms }); return nextId++; },
        clearTimeout: id => timers.delete(id),
    });
    vm.runInContext(slice('function playerAutoHide('), context);
    const bars = context.playerAutoHide(modal, video);
    const tick = ms => {
        const end = now + ms;
        for (;;) {
            const due = [...timers].filter(([, t]) => t.at <= end).sort((a, b) => a[1].at - b[1].at)[0];
            if (!due) break;
            timers.delete(due[0]);
            now = due[1].at;
            due[1].fn();
        }
        now = end;
    };
    const idle = () => container.classList.contains('jf-player-idle');
    return { bars, container, modal, video, doc, state, tick, idle, pending: () => timers.size };
}

const video = { target: { closest: () => null, tagName: 'DIV' } };
const select = { target: { closest: () => ({}), tagName: 'SELECT' } };

test('while playing, the bars hide 3 s after the last input, not before', () => {
    const s = sandbox();
    s.video.fire('playing');
    s.tick(2999);
    assert.equal(s.idle(), false);
    s.tick(1);
    assert.equal(s.idle(), true);
});

test('pointer move, mouse down, focus and key input show the bars and restart the timer', () => {
    const s = sandbox();
    const inputs = [
        () => s.container.fire('pointermove', { pointerType: 'mouse', ...video }),
        () => s.container.fire('pointerdown', { pointerType: 'mouse', ...video }),
        () => s.container.fire('focusin', video),
        () => s.doc.fire('keydown', { key: 'a' }),
    ];
    for (const input of inputs) {
        s.video.fire('playing');
        s.tick(3000);
        assert.equal(s.idle(), true);
        input();
        assert.equal(s.idle(), false, String(input));
        s.tick(2000);
        input();
        s.tick(2999);
        assert.equal(s.idle(), false, 'timer restarted');
        s.tick(1);
        assert.equal(s.idle(), true);
    }
});

test('pause, end, seek and buffering show the bars; paused, they stay', () => {
    for (const [event, field] of [['pause', 'paused'], ['ended', 'ended'], ['seeking', 'seeking'], ['waiting', 'readyState']]) {
        const s = sandbox();
        s.video.fire('playing');
        s.tick(3000);
        assert.equal(s.idle(), true);
        s.video[field] = field === 'readyState' ? 2 : true;
        s.video.fire(event);
        assert.equal(s.idle(), false, event);
        s.tick(60000);
        assert.equal(s.idle(), false, `${event}: still visible`);
        s.container.fire('pointerleave', { pointerType: 'mouse' });
        assert.equal(s.idle(), false, `${event}: visible after leaving`);
    }
});

test('an open menu or a hovered bar keeps the bars until it closes', () => {
    const s = sandbox();
    s.video.fire('playing');
    s.container.fire('pointerdown', { pointerType: 'mouse', ...select });
    s.tick(10000);
    assert.equal(s.idle(), false, 'select open');
    s.container.fire('change', select);
    s.tick(3000);
    assert.equal(s.idle(), true, 'hides after the choice');

    s.state.menuExpanded = true;
    s.container.fire('pointermove', { pointerType: 'mouse', ...video });
    s.tick(10000);
    assert.equal(s.idle(), false, 'aria-expanded menu open');
    s.state.menuExpanded = false;

    s.state.barHovered = true;
    s.container.fire('pointermove', { pointerType: 'mouse', ...select });
    s.tick(10000);
    assert.equal(s.idle(), false, 'pointer over a bar');
});

test('leaving the player while playing hides the bars at once', () => {
    const s = sandbox();
    s.video.fire('playing');
    s.container.fire('pointerleave', { pointerType: 'mouse' });
    assert.equal(s.idle(), true);
});

test('a tap toggles the bars without playing or pausing; a click does not', () => {
    const s = sandbox();
    s.video.fire('playing');
    s.container.fire('pointerdown', { pointerType: 'touch', ...video });
    assert.equal(s.bars.tap({ pointerType: 'touch' }), true);
    assert.equal(s.idle(), true, 'tap hides visible bars');
    assert.equal(s.bars.tap({ pointerType: 'touch' }), true);
    assert.equal(s.idle(), false, 'tap shows hidden bars');
    s.tick(3000);
    assert.equal(s.idle(), true, 'and they hide again after 3 s');
    s.video.paused = true;
    s.video.fire('pause');
    s.bars.tap({ pointerType: 'touch' });
    assert.equal(s.idle(), false, 'paused: a tap keeps them');
    s.container.fire('pointerdown', { pointerType: 'mouse', ...video });
    assert.equal(s.bars.tap({ pointerType: 'mouse' }), false);
});

test('reset on close shows the bars and leaves no timer; keys after close do nothing', () => {
    const s = sandbox();
    s.video.fire('playing');
    s.tick(3000);
    s.container.fire('pointermove', { pointerType: 'mouse', ...video });
    assert.ok(s.pending() > 0);
    s.modal.open = false;
    s.bars.reset();
    assert.equal(s.idle(), false);
    assert.equal(s.pending(), 0);
    s.doc.fire('keydown', { key: 'a' });
    assert.equal(s.pending(), 0);
});
