// The player's Playback info overlay, after Jellyfin Web's player stats: the play method, the transcode reasons
// in Jellyfin's names, the quality limit, resolution, dropped frames and buffer, then the original media.
// Runs player.js's stats functions in a vm sandbox.
// Usage: node r3playercore_stats.mjs (run by api_tests.rs r3playercore_stats).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const playerJs = fs.readFileSync(new URL('modules/player.js', src), 'utf8');
const html = fs.readFileSync(new URL('index.html', src), 'utf8');
const prefs = await import(new URL('modules/playerprefs.js', src));

function slice(head) {
    const start = playerJs.indexOf(head);
    assert.ok(start >= 0, `player.js has ${head}`);
    return playerJs.slice(start, playerJs.indexOf('\n}\n', start) + 3);
}
function decl(name) {
    const m = new RegExp(`^const ${name} = [\\s\\S]*?;$`, 'm').exec(playerJs);
    assert.ok(m, `player.js declares ${name}`);
    return m[0];
}

const context = vm.createContext({ t: (k, o) => (o ? `${k}|${JSON.stringify(o)}` : k), bitrateLabel: prefs.bitrateLabel });
vm.runInContext(`${decl('formatBitrate')}
    ${['function isStreamType(', 'function findStream(', 'function transcodeReasons(', 'function bufferedAhead(', 'function playbackStats('].map(slice).join('\n')}
    this.fns = { transcodeReasons, playbackStats };`, context);
const { transcodeReasons, playbackStats } = context.fns;
const plain = v => JSON.parse(JSON.stringify(v));

const CODECS = { video: ['h264', 'vp9'], audio: ['aac', 'opus'] };
const session = (extra = {}) => ({
    playMethod: 'Transcode',
    mediaSourceId: 's1',
    sources: [{ Id: 's1', Container: 'mkv', Size: 147711725, Bitrate: 4788547 }],
    streams: [
        { Index: 0, CodecType: 'video', Codec: 'h264', Profile: 'Main', Width: 1480, Height: 1080, BitDepth: 8 },
        { Index: 1, CodecType: 'audio', Codec: 'ac3', Language: 'eng', ChannelLayout: 'mono', BitRate: 320000 },
        { Index: 5, CodecType: 'subtitle', Codec: 'PGSSUB', Language: 'eng' },
    ],
    audioIndex: 1,
    subtitleIndex: -1,
    burnedIn: false,
    ...extra,
});

test('the transcode reasons the client can tell, in Jellyfin\'s names', () => {
    assert.deepEqual(plain(transcodeReasons(session(), CODECS, 0)), ['AudioCodecNotSupported']);
    assert.deepEqual(plain(transcodeReasons(session({ burnedIn: true, subtitleIndex: 5 }), CODECS, 3e6)),
        ['AudioCodecNotSupported', 'SubtitleCodecNotSupported', 'ContainerBitrateExceedsLimit']);
    const avi = session({ sources: [{ Id: 's1', Container: 'avi' }] });
    avi.streams[0].Codec = 'mpeg4';
    assert.deepEqual(plain(transcodeReasons(avi, { video: ['h264'], audio: ['ac3'] }, 0)), ['ContainerNotSupported', 'VideoCodecNotSupported']);
    assert.deepEqual(plain(transcodeReasons(session({ playMethod: 'DirectPlay' }), CODECS, 0)), [], 'no transcode, no reasons');
});

test('the overlay\'s rows: playback first, then the original media; empty rows are left out', () => {
    const video = {
        videoWidth: 1480, videoHeight: 1080, currentTime: 10,
        buffered: { length: 1, start: () => 0, end: () => 13.1 },
        getVideoPlaybackQuality: () => ({ droppedVideoFrames: 2, totalVideoFrames: 500 }),
    };
    const stats = plain(playbackStats(session({ burnedIn: true, subtitleIndex: 5 }), video, { codecs: CODECS, limit: 3e6, hls: { levels: [{ bitrate: 2950000 }], currentLevel: 0 } }));
    assert.deepEqual(stats, [
        ['player.playback_info', [
            ['player.stats_player', 'HTML video (hls.js)'],
            ['player.stats_play_method', 'player.hls_transcode'],
            ['player.stats_reasons', 'player.reason_AudioCodecNotSupported, player.reason_SubtitleCodecNotSupported, player.reason_ContainerBitrateExceedsLimit'],
            ['player.quality', '3 Mbps'],
            ['player.stats_stream_bitrate', '2.95 Mbps'],
            ['player.stats_resolution', '1480×1080'],
            ['player.stats_dropped', '2 / 500'],
            ['player.stats_buffer', '3.1 s'],
        ]],
        ['player.stats_original', [
            ['player.stats_container', 'MKV'],
            ['player.stats_size', '140.9 MiB'],
            ['player.stats_bitrate', '4.79 Mbps'],
            ['player.stats_video', 'H264 · Main · 1480×1080 · 8 bit'],
            ['player.stats_audio', 'AC3 · eng · mono · 320 kbps'],
            ['player.stats_subtitles', 'PGSSUB · eng · player.stats_burned_in'],
        ]],
    ]);
    const direct = plain(playbackStats(session({ playMethod: 'DirectPlay' }), { videoWidth: 0, buffered: { length: 0 } }, { codecs: CODECS, limit: 0 }));
    assert.deepEqual(direct[0][1].map(([k]) => k), ['player.stats_player', 'player.stats_play_method', 'player.quality', 'player.stats_buffer']);
});

test('the overlay is in the player and the settings menu opens it', () => {
    assert.match(html, /id="player-stats" class="jf-player-stats" hidden/);
    assert.match(html, /id="player-stats-toggle"[^>]*onclick="togglePlaybackStats\(\)"/);
    assert.match(slice('function onPlayerClosed('), /togglePlaybackStats\(false\)/);
});
