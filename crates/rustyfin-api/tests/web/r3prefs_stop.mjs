// One stop for local playback: stopNowPlaying stops music and video, ends the queue and the now-playing
// bar and closes the player, and every stop or hand-off path uses it: a remote Stop, Play on another
// session, the Cast hand-off, the OS stop key and the native shell's stop.
// Usage: node r3prefs_stop.mjs (run by api_tests.rs r3prefs_stop).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, src), 'utf8');
const playerJs = read('modules/player.js');

function slice(text, head) {
    const start = text.indexOf(head);
    assert.ok(start >= 0, `has ${head}`);
    return text.slice(start, text.indexOf('\n}\n', start) + 3);
}

test('stopNowPlaying stops music, ends the queue and the bar, and closes the player', () => {
    const calls = [];
    const modal = { open: true, close() { this.open = false; calls.push('close'); } };
    const context = vm.createContext({
        $: id => (id === 'player-modal' ? modal : null),
        stopPlayback: () => calls.push('stop'),
        clearQueue: () => calls.push('queue'),
        resetEpisodeNav: () => {},
        clearMediaSession: () => calls.push('media'),
        updateNowPlaying: s => calls.push(['bar', s]),
    });
    vm.runInContext(`${slice(playerJs, 'function stopNowPlaying(')}\n${slice(playerJs, 'function endNowPlaying(')}`, context);
    context.stopNowPlaying();
    assert.deepEqual(JSON.parse(JSON.stringify(calls)), ['stop', 'queue', 'media', ['bar', null], 'close']);
    // With the player already closed (music under the bar), it still stops
    calls.length = 0;
    context.stopNowPlaying();
    assert.deepEqual(calls.slice(0, 1), ['stop']);
    assert.ok(!calls.includes('close'));
});

test('every stop and hand-off path stops local playback through stopNowPlaying', () => {
    const remote = read('modules/remote.js');
    assert.match(remote, /Stop: \(\) => stopNowPlaying\(\),/);
    assert.match(slice(read('modules/sessions.js'), 'async function playOnSession('), /stopNowPlaying\(\);/);
    assert.match(slice(read('modules/cast.js'), 'async function castToDevice('), /stopNowPlaying\(\);/);
    assert.match(slice(playerJs, 'function bindMediaSession('), /setMediaSessionHandler\('stop', \(\) => stopNowPlaying\(\)\)/);
    assert.match(read('modules/nativeshell.js'), /stop: \(\) => call\('stopNowPlaying'\),/);
    // None of them closes only the dialog, which leaves music playing
    for (const file of ['modules/remote.js', 'modules/sessions.js', 'modules/cast.js']) {
        assert.doesNotMatch(read(file), /closePlayer/, `${file} stops through stopNowPlaying`);
    }
});
