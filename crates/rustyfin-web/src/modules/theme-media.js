// Theme songs on item pages, as Jellyfin Web's themeMediaPlayer: with the user setting "Play theme songs"
// (Jellyfin Web's enableThemeSongs, off unless turned on), an item page plays its theme song, or its season's
// or series' (inheritFromParent), quietly and in a loop. Leaving the page or starting playback stops it; moving
// between pages with the same theme, such as a series and its seasons, keeps it playing.
import { userSetting } from './user-settings.js';

const THEME_VOLUME = 0.3;

let theme = null; // { ownerId, audio }
// Each stop counts up, so a theme song still loading when playback starts (or the page is left) never starts
let stops = 0;

function themeSongsEnabled() {
    return userSetting('enableThemeSongs', 'false') === 'true';
}

function stopThemeSong() {
    stops++;
    if (!theme) return;
    const { audio } = theme;
    theme = null;
    audio.pause();
    audio.removeAttribute('src');
    audio.load();
}

// Plays the theme song of the item page `itemId`; stillHere() says whether that page is still shown once the
// theme songs have loaded
async function playThemeSong(itemId, stillHere = () => true) {
    if (!themeSongsEnabled()) {
        stopThemeSong();
        return;
    }
    const stopsBefore = stops;
    let result;
    try {
        result = await API.getThemeSongs(itemId);
    } catch {
        return;
    }
    if (!stillHere() || stops !== stopsBefore) return;
    const song = (result?.Items || [])[0];
    if (!song) {
        stopThemeSong();
        return;
    }
    const ownerId = result.OwnerId || itemId;
    if (theme?.ownerId === ownerId) return;
    stopThemeSong();
    const audio = new Audio(API.streamUrl(song.Id, 'Audio'));
    audio.volume = THEME_VOLUME;
    audio.loop = true;
    theme = { ownerId, audio };
    // A browser that blocks autoplay before the first click leaves it silent
    audio.play().catch(() => {});
}

export { themeSongsEnabled, playThemeSong, stopThemeSong };
