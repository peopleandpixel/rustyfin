// The player's default audio and subtitle tracks from the user's Jellyfin preferences (AudioLanguagePreference,
// SubtitleLanguagePreference, SubtitleMode, PlayDefaultAudioTrack, Remember*Selections), as Jellyfin's
// MediaStreamSelector picks them. Loads playerprefs.js as a module and checks playItem's wiring as text.
// Usage: node r3playercore_tracks.mjs (run by api_tests.rs r3playercore_tracks).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const prefs = await import(new URL('modules/playerprefs.js', src));
const playerJs = fs.readFileSync(new URL('modules/player.js', src), 'utf8');

const stream = (Index, type, extra = {}) => ({ Index, CodecType: type, ...extra });
const STREAMS = [
    stream(0, 'video'),
    stream(1, 'audio', { Language: 'eng', IsDefault: true }),
    stream(2, 'audio', { Language: 'fre' }),
    stream(3, 'subtitle', { Language: 'eng' }),
    stream(4, 'subtitle', { Language: 'eng', IsForced: true }),
    stream(5, 'subtitle', { Language: 'ger' }),
    stream(6, 'subtitle', { Language: 'fre', IsDefault: true }),
];
const audio = i => STREAMS.find(s => s.Index === i);
const config = c => prefs.trackPrefs(c);

test('preferences default as Jellyfin UserConfiguration does', () => {
    assert.deepEqual(prefs.trackPrefs(undefined), {
        audioLanguage: '', subtitleLanguage: '', subtitleMode: 'Default', playDefaultAudio: true, rememberAudio: true, rememberSubtitles: true,
    });
});

test('audio: the default track wins while PlayDefaultAudioTrack is on, else the preferred language', () => {
    assert.equal(prefs.defaultAudioIndex(STREAMS, config({ AudioLanguagePreference: 'fra' })), 1);
    assert.equal(prefs.defaultAudioIndex(STREAMS, config({ AudioLanguagePreference: 'fra', PlayDefaultAudioTrack: false })), 2,
        'fra and fre name the same language');
    assert.equal(prefs.defaultAudioIndex(STREAMS, config({ PlayDefaultAudioTrack: false })), 1, 'no preference: the default flag');
    assert.equal(prefs.defaultAudioIndex([stream(1, 'audio'), stream(2, 'audio', { Language: 'jpn' })], config({ AudioLanguagePreference: 'jpn' })), 2);
    assert.equal(prefs.defaultAudioIndex([stream(0, 'video')], config({})), null);
});

test('SubtitleMode Default: external, forced or default flags; preferred language first among them', () => {
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(1), config({})), 4, 'forced in the audio language');
    assert.equal(prefs.defaultSubtitleIndex([...STREAMS, stream(7, 'subtitle', { Language: 'spa', IsExternal: true })], audio(1), config({})), 7,
        'external first');
    assert.equal(prefs.defaultSubtitleIndex([stream(3, 'subtitle', { Language: 'eng' })], audio(1), config({})), -1, 'no flags: off');
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(1), config({ SubtitleLanguagePreference: 'fre' })), 6);
});

test('SubtitleMode Always: a full track in the preferred language, else as Default', () => {
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(1), config({ SubtitleMode: 'Always', SubtitleLanguagePreference: 'eng' })), 3,
        'the forced English track is passed over');
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(1), config({ SubtitleMode: 'Always', SubtitleLanguagePreference: 'deu' })), 5);
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(1), config({ SubtitleMode: 'Always', SubtitleLanguagePreference: 'jpn' })), 4);
});

test('SubtitleMode OnlyForced and None', () => {
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(2), config({ SubtitleMode: 'OnlyForced' })), 4);
    assert.equal(prefs.defaultSubtitleIndex(STREAMS.filter(s => !s.IsForced), audio(1), config({ SubtitleMode: 'OnlyForced' })), -1);
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(1), config({ SubtitleMode: 'None' })), -1);
});

test('SubtitleMode Smart: subtitles when the audio is in another language, else forced only', () => {
    const smart = config({ SubtitleMode: 'Smart', SubtitleLanguagePreference: 'eng' });
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(2), smart), 3, 'French audio: English subtitles');
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(1), smart), 4, 'English audio: the forced track');
    assert.equal(prefs.defaultSubtitleIndex(STREAMS, audio(2), config({ SubtitleMode: 'Smart', SubtitleLanguagePreference: 'jpn' })), -1);
});

test('remembered tracks win only while the user keeps them', () => {
    const source = { MediaStreams: STREAMS, DefaultAudioStreamIndex: 2, DefaultSubtitleStreamIndex: -1 };
    assert.deepEqual(prefs.defaultTracks(source, config({})), { audioIndex: 2, subtitleIndex: -1, subtitleFromServer: true });
    assert.deepEqual(prefs.defaultTracks(source, config({ RememberAudioSelections: false, RememberSubtitleSelections: false })),
        { audioIndex: 1, subtitleIndex: 4, subtitleFromServer: false });
    assert.deepEqual(prefs.defaultTracks({ ...source, DefaultAudioStreamIndex: 9, DefaultSubtitleStreamIndex: 8 }, config({})),
        { audioIndex: 1, subtitleIndex: 4, subtitleFromServer: false }, 'a remembered index the source lacks is ignored');
    assert.deepEqual(prefs.defaultTracks({ ...source, DefaultAudioStreamIndex: null, DefaultSubtitleStreamIndex: null },
        config({ AudioLanguagePreference: 'fre', PlayDefaultAudioTrack: false, SubtitleMode: 'Smart', SubtitleLanguagePreference: 'eng' })),
    { audioIndex: 2, subtitleIndex: 3, subtitleFromServer: false }, 'the subtitle follows the chosen audio');
});

test('playItem reads the configuration and starts from these defaults', () => {
    const play = playerJs.slice(playerJs.indexOf('async function playItem('));
    assert.match(play, /API\.getMe\(\)/);
    assert.match(play, /const prefs = trackPrefs\(me\?\.Configuration\)/);
    assert.match(play, /const defaults = defaultTracks\(source, prefs\)/);
    assert.match(playerJs, /from '\.\/playerprefs\.js'/);
});
