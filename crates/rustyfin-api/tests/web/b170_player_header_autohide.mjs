// B170: the player's top bar (title, back and close) hides and shows with the bottom controls.
// Reads index.html and input.css next to app.js as text: the top bar must take the same
// opacity and transition rules as the controls, and carry no opacity or transition of its own.
// Usage: node b170_player_header_autohide.mjs PATH/TO/app.js (run by api_tests.rs b170_player_header_autohide).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';

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
    assert.equal(controls['.X opacity'], '0', 'controls start hidden');
    assert.equal(controls['.jf-player-container:hover .X opacity'], '1', 'controls show on hover');
    assert.equal(controls['.X.visible opacity'], '1', 'controls show with .visible');
    assert.deepEqual(topbar, controls);
});
