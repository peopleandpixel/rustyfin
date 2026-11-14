// B142: provider key fields. An empty field keeps the saved key; only the clear box sends "".
// Runs app.js's providerKeyField in a vm sandbox.
// Usage: node b142_provider_keys.mjs PATH/TO/app.js (run by api_tests.rs b142_provider_key_field).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
const HEAD = 'function providerKeyField(';
const start = source.indexOf(HEAD);
assert.ok(start >= 0, `app.js has ${HEAD}`);
const end = source.indexOf('\n}\n', start) + 3;

const context = vm.createContext({});
vm.runInContext(source.slice(start, end) + '\nthis.providerKeyField = providerKeyField;', context);
const field = context.providerKeyField;

test('an empty or blank field leaves the saved key alone', () => {
    assert.equal(field('', false), undefined);
    assert.equal(field('   ', false), undefined);
    assert.equal(field(undefined, false), undefined);
    assert.equal(field(null, false), undefined);
});

test('a typed key is sent trimmed', () => {
    assert.equal(field(' abc123 ', false), 'abc123');
});

test('the clear box sends an empty string', () => {
    assert.equal(field('', true), '');
    assert.equal(field('  ', true), '');
});

test('a typed key wins over the clear box', () => {
    assert.equal(field('abc123', true), 'abc123');
});
