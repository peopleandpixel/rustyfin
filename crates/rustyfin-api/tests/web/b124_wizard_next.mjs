// B124: wizard Enter. A second Next (or Enter) while a step is still checking must not move twice.
// Runs app.js's `let wiz…` state lines and its wizNext in a vm sandbox with stubbed helpers.
// Usage: node b124_wizard_next.mjs PATH/TO/app.js (run by api_tests.rs b124_wizard_next_runs_once).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');

// A top-level function of app.js, from its head to its closing brace.
function topLevel(head) {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    const rest = source.slice(start);
    return rest.slice(0, rest.indexOf('\n}\n') + 2);
}

const state = source.split('\n').filter(line => /^let wiz\w+ = /.test(line)).join('\n');

function sandbox() {
    const context = vm.createContext({ console, setTimeout, Promise });
    vm.runInContext(state, context);
    vm.runInContext(`
        var moves = [];
        var leaveThrows = false;
        // One fake element stands for every node wizNext looks up (fields, steps, buttons)
        var el = {
            disabled: false,
            value: 'admin',
            querySelector() { return el; },
            querySelectorAll() { return [el]; },
            closest() { return el; },
            classList: { contains() { return false; }, toggle() {}, add() {}, remove() {} },
        };
        var document = { querySelector() { return el; }, querySelectorAll() { return [el]; } };
        function $() { return el; }
        function show() {}
        function hide() {}
        function t(key) { return key; }
        var API = { validatePath: async () => {} };
        async function wizLeaveIfDone() {
            await new Promise(resolve => setTimeout(resolve, 20));
            if (leaveThrows) throw new Error('network down');
            return false;
        }
        async function wizCheckStep() { return true; }
        function wizStepIndexOf() { return 99; }
        function wizLastStep() { return 8; }
        function wizMove(delta) { return wizStep + delta; }
        function wizSetStep(s) { wizStep = s; moves.push(s); }
    `, context);
    vm.runInContext(topLevel('async function wizNext() {'), context);
    return context;
}

// The sandbox's moves as an array of this realm, so deepEqual compares values only
function moves(c) { return [...vm.runInContext('moves', c)]; }

test('two Next calls in flight move one step', async () => {
    const c = sandbox();
    const first = vm.runInContext('wizNext()', c);
    const second = vm.runInContext('wizNext()', c);
    assert.equal(c.el.disabled, true, 'the primary button is disabled while Next runs');
    await Promise.all([first, second]);
    assert.deepEqual(moves(c), [1]);
    assert.equal(vm.runInContext('wizStep', c), 1);
    assert.equal(c.el.disabled, false, 'the primary button is enabled again');
    await vm.runInContext('wizNext()', c);
    assert.deepEqual(moves(c), [1, 2], 'a later Next moves again');
});

test('a Next that throws releases the guard', async () => {
    const c = sandbox();
    vm.runInContext('leaveThrows = true', c);
    await assert.rejects(vm.runInContext('wizNext()', c));
    assert.equal(c.el.disabled, false);
    vm.runInContext('leaveThrows = false', c);
    await vm.runInContext('wizNext()', c);
    assert.deepEqual(moves(c), [1]);
});
