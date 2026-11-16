// The music player's lyrics: a panel beside the now-playing bar that shows the playing song's lyrics from
// /Audio/{id}/Lyrics. Timed lines (any line with a Start, in ticks) highlight the line being sung, which
// scrolls into view, and a click seeks there; untimed lyrics are plain lines.
import { $, escapeHtml } from './util.js';

const TICKS_PER_SECOND = 1e7;

// The index of the line sung at positionTicks: the last timed line that has started, or -1 before the first.
// Lines without a Start never count.
function activeLyricIndex(lines, positionTicks) {
    let active = -1;
    lines.forEach((line, i) => {
        if (Number.isFinite(line?.Start) && line.Start <= positionTicks) active = i;
    });
    return active;
}

// Whether the lyrics are timed: any line has a Start
const isSyncedLyrics = lines => lines.some(line => Number.isFinite(line?.Start));

// The panel's lines: a timed line is a button that seeks to it, anything else is plain text
function lyricsLinesHtml(lines) {
    const synced = isSyncedLyrics(lines);
    return lines.map((line, i) => {
        const text = escapeHtml(line?.Text || '');
        if (synced && Number.isFinite(line?.Start)) {
            return `<button type="button" class="jf-lyric-line" data-action="seek-lyric" data-start="${line.Start}" data-lyric-index="${i}">${text}</button>`;
        }
        return `<p class="jf-lyric-line jf-lyric-plain">${text}</p>`;
    }).join('');
}

// The track whose lyrics the panel shows (null without one), its lyrics once loaded ({ itemId, lines, synced }),
// whether the panel is open, and the line last highlighted
let track = null;
let lyrics = null;
let loadSeq = 0;
let open = false;
let highlighted = -1;

function renderLyrics() {
    const body = $('lyrics-body');
    if (!body) return;
    highlighted = -1;
    if (!lyrics) body.innerHTML = `<p class="text-sm text-white/50">${t('common.loading')}</p>`;
    else if (!lyrics.lines.length) body.innerHTML = `<p class="text-sm text-white/50">${t('player.no_lyrics')}</p>`;
    else body.innerHTML = lyricsLinesHtml(lyrics.lines);
}

// Fetches the lyrics of a song; a song without lyrics (404) or a failed request shows No lyrics
async function loadLyrics(itemId) {
    const seq = ++loadSeq;
    lyrics = null;
    renderLyrics();
    let data = null;
    try {
        data = await API.get(`/Audio/${encodeURIComponent(itemId)}/Lyrics`);
    } catch (e) {
        console.warn('Could not load the lyrics:', e);
    }
    if (seq !== loadSeq || itemId !== track) return;
    const lines = Array.isArray(data?.Lyrics) ? data.Lyrics : [];
    lyrics = { itemId, lines, synced: isSyncedLyrics(lines) };
    renderLyrics();
}

// Opens or closes the panel (toggles without an argument); it shows the playing song's lyrics
function toggleLyrics(show = !open) {
    open = !!show && !!track;
    const panel = $('lyrics-panel');
    if (panel) panel.hidden = !open;
    $('now-playing-lyrics')?.setAttribute('aria-pressed', String(open));
    if (open && lyrics?.itemId !== track) loadLyrics(track);
}

// The song the player plays now (null for none). Its lyrics button shows unless the server says it has no
// lyrics (hasLyrics false); a new song drops the lyrics shown and loads them when the panel is open.
function setLyricsTrack(itemId, hasLyrics = undefined) {
    const button = $('now-playing-lyrics');
    if (button) button.hidden = !itemId || hasLyrics === false;
    if (itemId === track) return;
    track = itemId || null;
    lyrics = null;
    if (!track) {
        toggleLyrics(false);
        return;
    }
    if (open) loadLyrics(track);
}

// Marks the line sung at positionTicks in synced lyrics, and scrolls it into view when it changes
function highlightLyrics(positionTicks) {
    if (!open || !lyrics?.synced || lyrics.itemId !== track) return;
    const index = activeLyricIndex(lyrics.lines, positionTicks);
    if (index === highlighted) return;
    highlighted = index;
    const body = $('lyrics-body');
    if (!body) return;
    body.querySelectorAll('.jf-lyric-line.active').forEach(line => line.classList.remove('active'));
    const line = body.querySelector(`[data-lyric-index="${index}"]`);
    if (!line) return;
    line.classList.add('active');
    line.scrollIntoView({ block: 'center', behavior: 'smooth' });
}

export { activeLyricIndex, isSyncedLyrics, lyricsLinesHtml, setLyricsTrack, toggleLyrics, highlightLyrics, TICKS_PER_SECOND };
