// The web client's Universal call: it sends no PlaySessionId of its own, so the server gives each
// load a new one, and the player keeps the id the server answers with to stop that load's
// transcode; and it states the HEVC bit depth the browser decodes, so the server can copy HEVC.
// Runs API.getUniversalVideoUrl in a vm sandbox with a stubbed request.
// Usage: node r3playback_session_id.mjs (run by api_tests.rs r3playback_web_session_id).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const api = fs.readFileSync(new URL('api.js', src), 'utf8');
const player = fs.readFileSync(new URL('modules/player.js', src), 'utf8');

function method(text, head) {
    const start = text.indexOf(head);
    assert.ok(start >= 0, `the client has ${head}`);
    const end = text.indexOf('\n    },\n', start);
    return text.slice(start, end) + '\n    }';
}

const universal = method(api, 'async getUniversalVideoUrl(');

async function universalQuery(params, codecs = { video: ['h264', 'hevc'], audio: ['aac'], hevcMain10: true }) {
    const requested = [];
    const context = vm.createContext({
        URLSearchParams,
        localStorage: { getItem: () => 'device-1' },
        requested,
        codecs,
    });
    vm.runInContext(
        `var API = {
            playableCodecs() { return codecs; },
            async get(path) { requested.push(path); return { Url: '/Videos/x/main.m3u8', PlaySessionId: 'server-id' }; },
            ${universal},
        };`,
        context,
    );
    const data = await vm.runInContext('API', context).getUniversalVideoUrl('item-1', params);
    assert.equal(requested.length, 1);
    return { data, query: new URL(requested[0], 'http://x').searchParams };
}

test('the Universal call sends no PlaySessionId', async () => {
    for (const params of [{}, { startTimeTicks: 100, audioStreamIndex: 1, subtitleStreamIndex: 4 }]) {
        const { query } = await universalQuery(params);
        for (const name of query.keys()) {
            assert.notEqual(name.toLowerCase(), 'playsessionid', `sent ${query.toString()}`);
        }
        assert.equal(query.get('VideoCodecs'), 'h264,hevc');
    }
});

test('the Universal call states the HEVC bit depth the browser decodes', async () => {
    const depth = async codecs => (await universalQuery({}, codecs)).query.get('hevc-videobitdepth');
    assert.equal(await depth({ video: ['h264', 'hevc'], audio: ['aac'], hevcMain10: true }), '10');
    assert.equal(await depth({ video: ['h264', 'hevc'], audio: ['aac'], hevcMain10: false }), '8');
    // A browser without HEVC states nothing about it
    assert.equal(await depth({ video: ['h264'], audio: ['aac'], hevcMain10: false }), null);
    // The level it states, conservatively at most 4.1
    const level = async codecs => (await universalQuery({}, codecs)).query.get('hevc-level');
    assert.equal(await level({ video: ['hevc'], audio: [], hevcMain10: true, hevcLevel: 123 }), '123');
    assert.equal(await level({ video: ['hevc'], audio: [], hevcMain10: true }), '93');
    assert.equal(await level({ video: ['h264'], audio: [], hevcLevel: 123 }), null);
    assert.match(api, /hevcLevel: \[123, 120, 93\]\.find\(/);
    const probe = api.slice(api.indexOf('playableCodecs() {'), api.indexOf('getUniversalVideoUrl('));
    assert.match(probe, /hevcMain10: canPlay\('video\/mp4; codecs="hvc1\.2\.4\./);
});

test("the player keeps the server's PlaySessionId", async () => {
    const { data } = await universalQuery({});
    assert.equal(data.PlaySessionId, 'server-id');
    assert.match(player, /playSessionId = data\.PlaySessionId \|\| null;/);
});
