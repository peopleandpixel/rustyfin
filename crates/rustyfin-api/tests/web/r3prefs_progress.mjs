// Progress reports as Jellyfin Web sends them: every 10 s while the item is open, paused too, and on pause
// and unpause, each with IsPaused from the player, so the server doesn't stop a paused session (and its
// transcode) after its idle limit. Runs player.js's reportSessionProgress in a vm sandbox.
// Usage: node r3prefs_progress.mjs (run by api_tests.rs r3prefs_progress).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const playerJs = fs.readFileSync(new URL('modules/player.js', src), 'utf8');

function slice(head) {
    const start = playerJs.indexOf(head);
    assert.ok(start >= 0, `player.js has ${head}`);
    return playerJs.slice(start, playerJs.indexOf('\n}\n', start) + 3);
}

function sandbox() {
    const reports = [];
    const video = { paused: false, ended: false };
    const context = vm.createContext({
        $: id => (id === 'video-player' ? video : null),
        API: { reportProgress: (...args) => { reports.push(args); return Promise.resolve(); } },
    });
    vm.runInContext(`var activePlayback = null; ${slice('function reportSessionProgress(')}
        this.setActive = s => { activePlayback = s; };`, context);
    return { context, video, reports };
}

const session = () => ({ itemId: 'i1', position: () => 12.5, reportFields: () => ({ PlaySessionId: 'p1' }) });

test('a report carries the position and whether the player is paused', () => {
    const { context, video, reports } = sandbox();
    const s = session();
    context.setActive(s);
    context.reportSessionProgress(s);
    video.paused = true;
    context.reportSessionProgress(s);
    assert.deepEqual(JSON.parse(JSON.stringify(reports)), [
        ['i1', 125000000, false, { PlaySessionId: 'p1' }],
        ['i1', 125000000, true, { PlaySessionId: 'p1' }],
    ]);
    assert.equal(s.reportedPaused, true, 'the next play is an unpause');
});

test('nothing is reported for an ended item or a playback that is no longer active', () => {
    const { context, video, reports } = sandbox();
    const s = session();
    context.reportSessionProgress(s);
    context.setActive(s);
    video.ended = true;
    context.reportSessionProgress(s);
    assert.deepEqual(reports, []);
});

test('the player reports every 10 s, paused or not, and on pause and unpause', () => {
    assert.match(playerJs, /const PROGRESS_REPORT_MS = 10000;/);
    assert.match(playerJs, /setInterval\(\(\) => reportSessionProgress\(session\), PROGRESS_REPORT_MS\)/);
    const play = /video\.onplay = \(\) => \{[\s\S]*?\n {4}\};/.exec(playerJs)?.[0] || '';
    assert.match(play, /if \(session\.reportedPaused\) reportSessionProgress\(session\);/);
    const pause = /video\.onpause = \(\) => \{[\s\S]*?\n {4}\};/.exec(playerJs)?.[0] || '';
    assert.match(pause, /if \(!video\.ended\) reportSessionProgress\(session\);/);
    assert.doesNotMatch(playerJs, /reportProgress\([^)]*, false,/, 'no report hard-codes IsPaused');
});
