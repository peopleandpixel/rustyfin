// B152: BlurHash previews. Runs app.js's decodeBlurhash in a vm sandbox.
// Usage: node b152_blurhash.mjs PATH/TO/app.js (run by api_tests.rs b152_blurhash_decoder).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
const HEAD = 'function decodeBlurhash(hash, w, h) {';
const start = source.indexOf(HEAD);
assert.ok(start >= 0, `app.js has ${HEAD}`);
const end = source.indexOf('\n}\n', start) + 3;
const digitsLine = source.split('\n').find(l => l.startsWith('const BLURHASH_DIGITS ='));
assert.ok(digitsLine, 'app.js has BLURHASH_DIGITS');
const context = vm.createContext({});
vm.runInContext(`${digitsLine.replace('const ', 'var ')}\n${source.slice(start, end)}`, context);
const decode = (...args) => context.decodeBlurhash(...args);

const DIGITS = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~';
const base83 = (n, len) => Array.from({ length: len }, (_, i) => DIGITS[Math.floor(n / 83 ** (len - 1 - i)) % 83]).join('');

test('a one-component hash decodes to its flat colour', () => {
    for (const [r, g, b] of [[255, 0, 0], [12, 200, 99], [0, 0, 0]]) {
        const hash = '00' + base83((r << 16) | (g << 8) | b, 4);
        const px = decode(hash, 4, 6);
        assert.equal(px.length, 4 * 6 * 4);
        for (let o = 0; o < px.length; o += 4) {
            assert.deepEqual([px[o], px[o + 1], px[o + 2], px[o + 3]], [r, g, b, 255], hash);
        }
    }
});

test('the reference hash decodes to opaque pixels that vary across the picture', () => {
    const px = decode('LEHV6nWB2yk8pyo0adR*.7kCMdnj', 20, 30);
    assert.equal(px.length, 20 * 30 * 4);
    for (let o = 3; o < px.length; o += 4) assert.equal(px[o], 255);
    const first = Array.from(px.slice(0, 3));
    const last = Array.from(px.slice(-4, -1));
    assert.notDeepEqual(first, last, 'a 4x3 hash is not flat');
});

test('the average colour is the hash DC component', () => {
    const start = source.indexOf('function blurhashAverage(hash) {');
    assert.ok(start >= 0, 'app.js has blurhashAverage');
    vm.runInContext(source.slice(start, source.indexOf('\n}\n', start) + 3), context);
    assert.equal(context.blurhashAverage('00' + base83((12 << 16) | (200 << 8) | 99, 4)), 'rgb(12, 200, 99)');
    // A many-component hash: its DC digits (2-5) alone, whatever follows
    assert.equal(context.blurhashAverage('L0' + base83(0x102030, 4) + 'WB2yk8pyo0adR*.7kCMdnj'), 'rgb(16, 32, 48)');
    for (const bad of [null, 'L', '00~~~~', '00!AAA']) assert.equal(context.blurhashAverage(bad), null, String(bad));
});

test('a BlurHash becomes a bitmap data URL (no canvas): the size asked, its colours, kept per hash', () => {
    const start = source.indexOf('const blurhashUrls = new Map();');
    assert.ok(start >= 0, 'app.js has blurhashImageUrl');
    const ctx = vm.createContext({ btoa, Map, decodeBlurhash: context.decodeBlurhash });
    vm.runInContext(source.slice(start, source.indexOf('\n}\n', start) + 3).replace('const blurhashUrls', 'var blurhashUrls'), ctx);
    const hash = '00' + base83((12 << 16) | (200 << 8) | 99, 4);
    const url = ctx.blurhashImageUrl(hash, 20, 30);
    assert.match(url, /^data:image\/bmp;base64,/);
    const bmp = Buffer.from(url.split(',')[1], 'base64');
    assert.equal(bmp.toString('latin1', 0, 2), 'BM');
    assert.equal(bmp.readUInt32LE(2), bmp.length);
    assert.equal(bmp.readInt32LE(18), 20);
    assert.equal(bmp.readInt32LE(22), 30);
    assert.equal(bmp.readUInt16LE(28), 24);
    const row = (20 * 3 + 3) & ~3;
    assert.equal(bmp.length, 54 + row * 30);
    // Every pixel is the flat colour, stored blue, green, red
    for (let y = 0; y < 30; y++) {
        for (let x = 0; x < 20; x++) {
            const o = 54 + y * row + x * 3;
            assert.deepEqual([bmp[o + 2], bmp[o + 1], bmp[o]], [12, 200, 99]);
        }
    }
    assert.equal(ctx.blurhashImageUrl(hash, 20, 30), url, 'the same string again');
    assert.equal(ctx.blurhashImageUrl('bad', 20, 30), null);
});

test('a malformed hash gives null', () => {
    for (const hash of [null, undefined, '', 'L', 'LEHV6nWB2yk8pyo0adR*.7kCMdn', 'LEHV6nWB2yk8pyo0adR*.7kCMdnj!', '00"AAAA']) {
        assert.equal(decode(hash, 4, 4), null, String(hash));
    }
});
