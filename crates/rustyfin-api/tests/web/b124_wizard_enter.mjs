// B124: wizard Enter. Enter in a wizard field clicks the active step's primary button.
// Runs app.js's wizEnterKey and the line that registers it in a vm sandbox, then sends fake keydown
// events through the listener that line registered.
// Usage: node b124_wizard_enter.mjs PATH/TO/app.js (run by api_tests.rs b124_wizard_enter_listener).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
const HEAD = 'function wizEnterKey(e) {';
const start = source.indexOf(HEAD);
assert.ok(start >= 0, `app.js has ${HEAD}`);
const end = source.indexOf('\n}\n', start) + 3;
// The function and the statement right after it, which registers it
const registration = source.slice(end).split('\n')[0];

const listeners = [];
const context = vm.createContext({
    document: { addEventListener: (type, fn) => listeners.push({ type, fn }) },
});
vm.runInContext(source.slice(start, end) + registration, context);

function keydown(event) {
    const bound = listeners.filter(l => l.type === 'keydown');
    assert.equal(bound.length, 1, `one keydown listener is registered (after wizEnterKey: ${registration})`);
    bound[0].fn(event);
}

// A wizard with an active step holding a primary button; `inStep` false puts the target outside it
function scene({ tagName = 'INPUT', type = 'text', inStep = true, inWizard = true } = {}) {
    const button = { clicks: 0, disabled: false, click() { if (!this.disabled) this.clicks++; } };
    const step = { querySelector: sel => (sel.includes('jf-btn-primary') ? button : null) };
    const target = {
        tagName,
        type,
        closest(sel) {
            if (sel.includes('jf-wizard-step')) return inStep ? step : null;
            if (sel.includes('setup-wizard')) return inWizard ? {} : null;
            return null;
        },
    };
    return { button, target };
}

function enter(target, extra = {}) {
    const event = {
        key: 'Enter', keyCode: 13, target, defaultPrevented: false, isComposing: false, repeat: false,
        shiftKey: false, ctrlKey: false, altKey: false, metaKey: false,
        prevented: false,
        preventDefault() { this.prevented = true; },
        ...extra,
    };
    keydown(event);
    return event;
}

test('Enter in a text, password or select field clicks the primary button', () => {
    for (const [tagName, type] of [['INPUT', 'text'], ['INPUT', 'password'], ['INPUT', 'radio'], ['INPUT', 'checkbox'], ['SELECT', 'select-one']]) {
        const { button, target } = scene({ tagName, type });
        const event = enter(target);
        assert.equal(button.clicks, 1, `${tagName} ${type}`);
        assert.equal(event.prevented, true, `${tagName} ${type} prevents the default`);
    }
});

test('a disabled primary button is not clicked', () => {
    const { button, target } = scene();
    button.disabled = true;
    enter(target);
    assert.equal(button.clicks, 0);
});

test('Enter is left alone where it is not the wizard\'s', () => {
    const cases = [
        ['another key', scene(), { key: 'a', keyCode: 65 }],
        ['already handled', scene(), { defaultPrevented: true }],
        ['IME composition', scene(), { isComposing: true }],
        ['IME keyCode 229', scene(), { keyCode: 229 }],
        ['Shift', scene(), { shiftKey: true }],
        ['Ctrl', scene(), { ctrlKey: true }],
        ['Alt', scene(), { altKey: true }],
        ['Meta', scene(), { metaKey: true }],
        ['a button', scene({ tagName: 'BUTTON', type: 'submit' }), {}],
        ['an input button', scene({ tagName: 'INPUT', type: 'button' }), {}],
        ['a link', scene({ tagName: 'A', type: undefined }), {}],
        ['a textarea', scene({ tagName: 'TEXTAREA', type: 'textarea' }), {}],
        ['outside the active step', scene({ inStep: false, inWizard: false }), {}],
    ];
    for (const [name, { button, target }, extra] of cases) {
        const event = enter(target, extra);
        assert.equal(button.clicks, 0, `${name}: no click`);
        assert.equal(event.prevented, false, `${name}: default kept`);
    }
});

test('a held-down Enter (key repeat) neither clicks nor reaches a wizard button', () => {
    for (const opts of [{}, { tagName: 'BUTTON', type: 'submit' }]) {
        const { button, target } = scene(opts);
        const event = enter(target, { repeat: true });
        assert.equal(button.clicks, 0, `${target.tagName}: no click`);
        assert.equal(event.prevented, true, `${target.tagName}: the repeat is swallowed, so a focused Finish isn't pressed`);
    }
    // Outside the wizard a repeat is not the wizard's business
    const { button, target } = scene({ inStep: false, inWizard: false });
    const event = enter(target, { repeat: true });
    assert.equal(button.clicks, 0);
    assert.equal(event.prevented, false);
});
