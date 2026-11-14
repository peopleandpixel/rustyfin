// B142 F2: the plugin JSON editor saves a merge patch. A top-level field the user deleted is sent as
// null so the server removes it; PluginId (added by GET) is never sent back.
// Runs app.js's pluginConfigPatch in a vm sandbox.
// Usage: node b142_plugin_config_patch.mjs PATH/TO/app.js (run by api_tests.rs b142_plugin_config_patch).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
const HEAD = 'function pluginConfigPatch(';
const start = source.indexOf(HEAD);
assert.ok(start >= 0, `app.js has ${HEAD}`);
const end = source.indexOf('\n}\n', start) + 3;

const context = vm.createContext({});
vm.runInContext(source.slice(start, end) + '\nthis.pluginConfigPatch = pluginConfigPatch;', context);
// Plain-object copy, so deepEqual doesn't trip over the sandbox's prototypes
const patch = (original, edited) => JSON.parse(JSON.stringify(context.pluginConfigPatch(original, edited)));

test('a deleted field is sent as null', () => {
    assert.deepEqual(patch({ a: 1, b: 2 }, { a: 3 }), { a: 3, b: null });
});

test('kept and new fields are sent as edited', () => {
    assert.deepEqual(patch({ a: 1 }, { a: 1, c: { d: 2 } }), { a: 1, c: { d: 2 } });
});

test('PluginId is never sent', () => {
    assert.deepEqual(patch({ PluginId: 'x', a: 1 }, { PluginId: 'x', a: 1 }), { a: 1 });
    assert.deepEqual(patch({ PluginId: 'x', a: 1 }, { a: 1 }), { a: 1 });
});

test('a non-object edit is sent unchanged', () => {
    assert.deepEqual(patch({ a: 1 }, [1, 2]), [1, 2]);
    assert.equal(context.pluginConfigPatch({ a: 1 }, 5), 5);
});

test('a non-object original deletes nothing', () => {
    assert.deepEqual(patch(null, { a: 1 }), { a: 1 });
});
