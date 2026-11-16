// The web client's Universal call: it sends no PlaySessionId of its own, so the server gives each
// load a new one, and the player keeps the id the server answers with to stop that load's
// transcode. Runs API.getUniversalVideoUrl in a vm sandbox with a stubbed request.
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

async function universalQuery(params) {
    const requested = [];
    const context = vm.createContext({
        URLSearchParams,
        localStorage: { getItem: () => 'device-1' },
        requested,
    });
    vm.runInContext(
        `var API = {
            playableCodecs() { return { video: ['h264', 'hevc'], audio: ['aac'] }; },
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

test("the player keeps the server's PlaySessionId", async () => {
    const { data } = await universalQuery({});
    assert.equal(data.PlaySessionId, 'server-id');
    assert.match(player, /playSessionId = data\.PlaySessionId \|\| null;/);
});
