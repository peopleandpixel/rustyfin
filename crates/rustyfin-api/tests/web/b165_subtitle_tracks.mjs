// B165: subtitles stayed on screen while the subtitle menu said Off. A text track that the player
// didn't add (left from an earlier playback, or captions that hls.js found in the stream) is
// disabled, and the player's own track is disabled before it is removed.
// Runs app.js's subtitle track functions in a vm sandbox with a fake video element.
// Usage: node b165_subtitle_tracks.mjs PATH/TO/app.js (run by api_tests.rs b165_subtitle_tracks).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');

function slice(head) {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return source.slice(start, source.indexOf('\n}\n', start) + 3);
}

// A video element whose textTracks holds the tracks of its <track> children and any added directly
function fakeVideo() {
    const children = [];
    const video = {
        textTracks: [],
        appendChild(el) {
            el.parent = video;
            children.push(el);
            video.textTracks.push(el.track);
        },
        querySelectorAll(selector) {
            assert.equal(selector, 'track[data-player-subtitle]');
            return children.filter(el => el.dataset.playerSubtitle);
        },
        querySelector(selector) {
            return video.querySelectorAll(selector)[0] || null;
        },
        // A track that something else added, like hls.js's captions
        addTextTrack(label) {
            const track = { label, mode: 'showing' };
            video.textTracks.push(track);
            return track;
        },
    };
    video.removeChild = el => {
        children.splice(children.indexOf(el), 1);
        // A removed track leaves the list, but keeps its mode
        video.textTracks.splice(video.textTracks.indexOf(el.track), 1);
    };
    return video;
}

function sandbox() {
    const video = fakeVideo();
    const context = vm.createContext({
        $: id => (id === 'video-player' ? video : { textContent: '' }),
        t: key => key,
        API: { base: '', token: 'tok' },
        activePlayback: null,
        document: {
            createElement: () => {
                const el = {
                    dataset: {},
                    track: { label: '', mode: 'disabled' },
                    addEventListener() {},
                    remove() { el.parent.removeChild(el); },
                };
                return el;
            },
        },
    });
    vm.runInContext(
        `${slice('function isStreamType(')}
         ${slice('function findStream(')}
         ${slice('function isImageSubtitle(')}
         ${slice('function playerSubtitleTrack(')}
         ${slice('function disableOtherTextTracks(')}
         ${slice('function clearTextSubtitle(')}
         ${slice('function showTextSubtitle(')}
         this.clearTextSubtitle = clearTextSubtitle; this.showTextSubtitle = showTextSubtitle;
         this.disableOtherTextTracks = disableOtherTextTracks;`,
        context,
    );
    return { context, video };
}

const session = subtitleIndex => ({
    itemId: 'item-1',
    mediaSourceId: 'src-1',
    offset: 0,
    subtitleIndex,
    streams: [
        { Index: 2, CodecType: 'subtitle', Codec: 'subrip', Language: 'eng' },
        { Index: 4, CodecType: 'subtitle', Codec: 'dvd_subtitle', Language: 'eng' },
    ],
});

test('a chosen text subtitle shows alone', () => {
    const { context, video } = sandbox();
    const stale = video.addTextTrack('');
    context.showTextSubtitle(session(2));
    const own = video.querySelector('track[data-player-subtitle]').track;
    assert.equal(own.mode, 'showing');
    assert.equal(stale.mode, 'disabled');
});

test('Off disables the own track before removing it, and any other track', () => {
    const { context, video } = sandbox();
    context.showTextSubtitle(session(2));
    const own = video.querySelector('track[data-player-subtitle]').track;
    const captions = video.addTextTrack('CC1');
    context.showTextSubtitle(session(-1));
    assert.equal(own.mode, 'disabled');
    assert.equal(captions.mode, 'disabled');
    assert.equal(video.querySelector('track[data-player-subtitle]'), null);
});

test('a burned-in picture subtitle leaves no text track showing', () => {
    const { context, video } = sandbox();
    const stale = video.addTextTrack('');
    context.showTextSubtitle(session(4));
    assert.equal(video.querySelector('track[data-player-subtitle]'), null);
    assert.equal(stale.mode, 'disabled');
});

test('a track added during the playback is disabled, the own one kept', () => {
    const { context, video } = sandbox();
    context.showTextSubtitle(session(2));
    const own = video.querySelector('track[data-player-subtitle]').track;
    const late = video.addTextTrack('CC1');
    context.disableOtherTextTracks(video);
    assert.equal(late.mode, 'disabled');
    assert.equal(own.mode, 'showing');
});

test('the player turns off captions that hls.js would add, and watches for added tracks', () => {
    assert.match(slice('async function openStream('), /enableCEA708Captions: false/);
    assert.match(source, /video\.textTracks\.onaddtrack = \(\) => disableOtherTextTracks\(video\);/);
});
