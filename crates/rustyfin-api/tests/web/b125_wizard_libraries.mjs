// B125: wizard libraries. The manual library step's row rules and Finish's create/replace plan.
// Runs app.js's wizManualRowsCheck and wizLibraryPlan (pure functions) in a vm sandbox.
// Usage: node b125_wizard_libraries.mjs PATH/TO/app.js (run by api_tests.rs b125_wizard_library_rules).
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

const context = vm.createContext({});
vm.runInContext(topLevel('function wizManualRowsCheck(rows) {'), context);
vm.runInContext(topLevel('function wizLibraryPlan(created, wanted) {'), context);

// Calls a sandbox function with JSON arguments and answers its result as plain data of this realm.
function call(name, ...args) {
    const text = vm.runInContext(`JSON.stringify(${name}(...${JSON.stringify(args)}))`, context);
    return JSON.parse(text);
}

const row = (name, type, ...paths) => ({ name, type, paths });
const ok = (name, type, ...paths) => ({ library: { name, type, paths }, error: '' });
const bad = error => ({ library: null, error });

test('no rows and empty rows are no libraries', () => {
    assert.deepEqual(call('wizManualRowsCheck', []), []);
    assert.deepEqual(call('wizManualRowsCheck', [row(' ', 'Movies', '', '  ')]), [bad('')]);
    assert.deepEqual(call('wizManualRowsCheck', [row('', 'TvShows')]), [bad('')]);
});

test('a filled row is trimmed and its paths deduplicated', () => {
    assert.deepEqual(
        call('wizManualRowsCheck', [row(' Films ', 'Movies', ' /a ', '', '/a', '/b')]),
        [ok('Films', 'Movies', '/a', '/b')],
    );
});

test('a half-filled row is incomplete', () => {
    assert.deepEqual(
        call('wizManualRowsCheck', [row('Films', 'Movies', '', ' '), row('', 'Movies', '/a')]),
        [bad('wizard.library_incomplete'), bad('wizard.library_incomplete')],
    );
});

test('a name used by an earlier library is a duplicate', () => {
    assert.deepEqual(
        call('wizManualRowsCheck', [
            row('Films', 'Movies', '/a'),
            row('', 'Movies'),
            row(' films ', 'Mixed', '/b'),
            row('Shows', 'TvShows', '/t'),
        ]),
        [ok('Films', 'Movies', '/a'), bad(''), bad('wizard.library_name_duplicate'), ok('Shows', 'TvShows', '/t')],
    );
});

const films = { id: '1', name: 'Films', type: 'Movies', paths: ['/a'] };
const shows = { id: '2', name: 'Shows', type: 'TvShows', paths: ['/t', '/u'] };
const music = { id: '3', name: 'Music', type: 'Music', paths: ['/m'] };
const want = ({ name, type, paths }) => ({ name, type, paths });

test('unchanged libraries are kept, in any order', () => {
    assert.deepEqual(call('wizLibraryPlan', [films, shows], [want(shows), want(films)]), {
        keep: [films, shows],
        remove: [],
        create: [],
    });
});

test('nothing created yet: everything is created', () => {
    assert.deepEqual(call('wizLibraryPlan', [], [want(films), want(music)]), {
        keep: [],
        remove: [],
        create: [want(films), want(music)],
    });
});

test('nothing wanted: everything created is removed', () => {
    assert.deepEqual(call('wizLibraryPlan', [films, shows], []), { keep: [], remove: [films, shows], create: [] });
});

test('a changed path, type or name replaces the library', () => {
    const morePaths = { ...want(films), paths: ['/a', '/x'] };
    const otherOrder = { ...want(shows), paths: ['/u', '/t'] };
    const otherType = { ...want(music), type: 'Mixed' };
    const renamed = { ...want(films), name: 'Movies' };
    assert.deepEqual(call('wizLibraryPlan', [films, shows, music], [morePaths, otherOrder, otherType]), {
        keep: [],
        remove: [films, shows, music],
        create: [morePaths, otherOrder, otherType],
    });
    assert.deepEqual(call('wizLibraryPlan', [films, shows], [want(shows), renamed]), {
        keep: [shows],
        remove: [films],
        create: [renamed],
    });
});
