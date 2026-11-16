// The player's preferences and the rules that read them: Jellyfin's default audio and subtitle choice,
// the subtitle look, media segment actions, quality steps, aspect ratio and the still-watching prompt.
// Device settings live in localStorage under Jellyfin Web's keys; the user's are prefixed with their id.
// No imports and no DOM, so tests load this module as it is.

// --- Device and user storage ---
function storage() {
    return typeof localStorage !== 'undefined' ? localStorage : null;
}

function readSetting(key, fallback = null) {
    try {
        const value = storage()?.getItem(key);
        return value === null || value === undefined ? fallback : value;
    } catch {
        return fallback;
    }
}

function writeSetting(key, value) {
    try {
        if (value === null || value === undefined || value === '') storage()?.removeItem(key);
        else storage()?.setItem(key, String(value));
    } catch {
        // Private mode or a full store: the choice lasts until the page reloads
    }
}

// Jellyfin Web keeps its user settings per device, under `<user id>-<name>`
function userKey(name) {
    const id = typeof API !== 'undefined' ? API.userId : '';
    return id ? `${id}-${name}` : name;
}

// --- Default tracks (Jellyfin's MediaStreamSelector) ---
// ISO 639-2 has a bibliographic and a terminology code for some languages; media and settings use either
const LANGUAGE_ALIASES = {
    fre: 'fra', ger: 'deu', dut: 'nld', chi: 'zho', cze: 'ces', gre: 'ell', per: 'fas', rum: 'ron', slo: 'slk',
    alb: 'sqi', arm: 'hye', baq: 'eus', bur: 'mya', geo: 'kat', ice: 'isl', mac: 'mkd', mao: 'mri', may: 'msa',
    tib: 'bod', wel: 'cym',
};
function languageCode(code) {
    const c = (code || '').trim().toLowerCase();
    return LANGUAGE_ALIASES[c] || c;
}
function sameLanguage(a, b) {
    return !!a && !!b && languageCode(a) === languageCode(b);
}

const isType = (s, type) => (s.CodecType || s.Type || '').toLowerCase() === type;

// The preferences that pick tracks, from Jellyfin's UserConfiguration
function trackPrefs(config) {
    const c = config || {};
    return {
        audioLanguage: c.AudioLanguagePreference || '',
        subtitleLanguage: c.SubtitleLanguagePreference || '',
        subtitleMode: c.SubtitleMode || 'Default',
        playDefaultAudio: c.PlayDefaultAudioTrack !== false,
        rememberAudio: c.RememberAudioSelections !== false,
        rememberSubtitles: c.RememberSubtitleSelections !== false,
    };
}

// Audio: the default-flagged track when PlayDefaultAudioTrack is on, else the preferred language first
function defaultAudioIndex(streams, prefs) {
    const audio = streams.filter(s => isType(s, 'audio'));
    const preferred = s => (sameLanguage(s.Language, prefs?.audioLanguage) ? 0 : 1);
    const sorted = [...audio].sort((a, b) => preferred(a) - preferred(b) || (b.IsDefault ? 1 : 0) - (a.IsDefault ? 1 : 0)
        || (a.IsExternal ? 1 : 0) - (b.IsExternal ? 1 : 0) || a.Index - b.Index);
    if (prefs?.playDefaultAudio !== false) {
        const flagged = sorted.find(s => s.IsDefault);
        if (flagged) return flagged.Index;
    }
    return sorted[0]?.Index ?? null;
}

// Subtitles by SubtitleMode: Default takes an external, forced or default track; Always a full track in the
// preferred language, else as Default; OnlyForced a forced one; Smart a preferred-language track (a full one
// first) when the audio is in another language, else a forced one; None none. -1 is off.
function defaultSubtitleIndex(streams, audio, prefs) {
    const mode = prefs?.subtitleMode || 'Default';
    if (mode === 'None') return -1;
    const audioLanguage = audio?.Language || '';
    const wanted = s => sameLanguage(s.Language, prefs?.subtitleLanguage);
    const flag = v => (v ? 1 : 0);
    const sorted = streams.filter(s => isType(s, 'subtitle')).sort((a, b) => flag(wanted(b)) - flag(wanted(a))
        || flag(b.IsExternal) - flag(a.IsExternal)
        || flag(b.IsForced && sameLanguage(b.Language, audioLanguage)) - flag(a.IsForced && sameLanguage(a.Language, audioLanguage))
        || flag(b.IsForced) - flag(a.IsForced)
        || flag(b.IsDefault) - flag(a.IsDefault)
        || a.Index - b.Index);
    const flagged = s => s.IsExternal || s.IsForced || s.IsDefault;
    let pick;
    if (mode === 'Always') pick = sorted.find(s => !s.IsForced && wanted(s)) || sorted.find(flagged);
    else if (mode === 'OnlyForced') pick = sorted.find(s => s.IsForced);
    else if (mode === 'Smart') {
        pick = sameLanguage(audioLanguage, prefs?.subtitleLanguage)
            ? sorted.find(s => s.IsForced)
            : sorted.find(s => !s.IsForced && wanted(s)) || sorted.find(wanted);
    }
    else pick = sorted.find(flagged);
    return pick ? pick.Index : -1;
}

// The tracks a source starts with: the server's remembered ones where the user keeps them, else the rules above
function defaultTracks(source, prefs) {
    const streams = source?.MediaStreams || [];
    const audio = streams.filter(s => isType(s, 'audio'));
    const subs = streams.filter(s => isType(s, 'subtitle'));
    const remembered = source?.DefaultAudioStreamIndex;
    const audioIndex = prefs?.rememberAudio !== false && audio.some(s => s.Index === remembered)
        ? remembered
        : defaultAudioIndex(streams, prefs);
    const chosenAudio = audio.find(s => s.Index === audioIndex) || null;
    const rememberedSub = source?.DefaultSubtitleStreamIndex;
    const subtitleFromServer = prefs?.rememberSubtitles !== false
        && (rememberedSub === -1 || subs.some(s => s.Index === rememberedSub));
    const subtitleIndex = subtitleFromServer ? rememberedSub : defaultSubtitleIndex(streams, chosenAudio, prefs);
    return { audioIndex, subtitleIndex, subtitleFromServer };
}

// --- Subtitle appearance (Jellyfin Web's localplayersubtitleappearance3, per user on this device) ---
const APPEARANCE_KEY = 'localplayersubtitleappearance3';
const APPEARANCE_DEFAULTS = { textSize: '', textWeight: 'normal', font: '', textColor: '#ffffff', dropShadow: '', textBackground: 'transparent', verticalPosition: -3 };
const SUBTITLE_SIZES = { smaller: '0.8em', small: '1em', '': '1.36em', medium: '1.36em', large: '1.72em', larger: '2em', extralarge: '2.2em' };
const SUBTITLE_FONTS = {
    '': '-apple-system,BlinkMacSystemFont,Segoe UI,Roboto,Helvetica,Arial,sans-serif',
    typewriter: '"Courier New",monospace',
    print: 'Georgia,Times New Roman,Arial,Helvetica,serif',
    console: 'Consolas,Lucida Console,Menlo,Monaco,monospace',
    cursive: 'Lucida Handwriting,Brush Script MT,Segoe Script,cursive,sans-serif',
    casual: 'Gabriola,Segoe Print,Comic Sans MS,Chalkboard,sans-serif',
    smallcaps: 'Copperplate Gothic,Copperplate,sans-serif',
};
const SUBTITLE_SHADOWS = {
    '': '#000000 0px 0px 7px',
    none: 'none',
    raised: '-0.04em -0.04em #fff, 0px -0.04em #fff, -0.04em 0px #fff, 0.04em 0.04em #000, 0px 0.04em #000, 0.04em 0px #000',
    depressed: '0.04em 0.04em #fff, 0px 0.04em #fff, 0.04em 0px #fff, -0.04em -0.04em #000, 0px -0.04em #000, -0.04em 0px #000',
    uniform: '#000 0px 0.05em, #000 0px -0.05em, #000 0.05em 0px, #000 -0.05em 0px, #000 0.04em 0.04em, #000 -0.04em 0.04em, #000 0.04em -0.04em, #000 -0.04em -0.04em',
};

function subtitleAppearance() {
    let stored = {};
    try {
        stored = JSON.parse(readSetting(userKey(APPEARANCE_KEY), '{}')) || {};
    } catch {
        stored = {};
    }
    return { ...APPEARANCE_DEFAULTS, ...stored };
}

function saveSubtitleAppearance(appearance) {
    writeSetting(userKey(APPEARANCE_KEY), JSON.stringify({ ...subtitleAppearance(), ...appearance }));
}

// CSS for the subtitle text and its window, as Jellyfin Web's appearance helper builds it. The position counts
// lines of 1.35em: below zero from the bottom (-1 is the edge), from zero on from the top.
function subtitleCss(appearance) {
    const a = { ...APPEARANCE_DEFAULTS, ...appearance };
    const pos = parseInt(a.verticalPosition, 10);
    const position = Number.isFinite(pos) ? pos : APPEARANCE_DEFAULTS.verticalPosition;
    return {
        text: {
            fontSize: SUBTITLE_SIZES[a.textSize] || SUBTITLE_SIZES[''],
            fontWeight: a.textWeight === 'bold' ? 'bold' : 'normal',
            fontFamily: SUBTITLE_FONTS[a.font] || SUBTITLE_FONTS[''],
            fontVariant: a.font === 'smallcaps' ? 'small-caps' : 'normal',
            color: a.textColor || '#ffffff',
            textShadow: SUBTITLE_SHADOWS[a.dropShadow] ?? SUBTITLE_SHADOWS[''],
            backgroundColor: a.textBackground || 'transparent',
        },
        window: position < 0
            ? { top: 'auto', bottom: '0', marginTop: '0', marginBottom: `${Math.abs(position + 1) * 1.35}em` }
            : { top: '0', bottom: 'auto', marginTop: `${position * 1.35}em`, marginBottom: '0' },
    };
}

// --- Subtitle offset: G and H move it by 0.1 s within ±30 s, as Jellyfin Web's sync slider ---
const SUBTITLE_OFFSET_STEP = 0.1;
const SUBTITLE_OFFSET_LIMIT = 30;
function stepSubtitleOffset(offset, step) {
    const next = Math.round((offset + step * SUBTITLE_OFFSET_STEP) * 10) / 10;
    return Math.min(Math.max(next, -SUBTITLE_OFFSET_LIMIT), SUBTITLE_OFFSET_LIMIT);
}

// --- Media segment actions (Jellyfin Web's segmentTypeAction__<type>) ---
const SEGMENT_TYPES = ['Intro', 'Outro', 'Recap', 'Preview', 'Commercial'];
const SEGMENT_ACTIONS = ['Skip', 'AskToSkip', 'None'];
const SEGMENT_DEFAULTS = { Intro: 'AskToSkip', Outro: 'AskToSkip' };
function segmentAction(type) {
    const stored = readSetting(userKey(`segmentTypeAction__${type}`));
    return SEGMENT_ACTIONS.includes(stored) ? stored : (SEGMENT_DEFAULTS[type] || 'None');
}
function setSegmentAction(type, action) {
    if (SEGMENT_TYPES.includes(type) && SEGMENT_ACTIONS.includes(action)) writeSetting(userKey(`segmentTypeAction__${type}`), action);
}

// --- Quality: Jellyfin Web's steps; 0 is Auto. The choice is the device's. ---
const BITRATE_STEPS = [120e6, 80e6, 60e6, 40e6, 20e6, 15e6, 10e6, 8e6, 6e6, 4e6, 3e6, 1.5e6, 720e3, 420e3];
const QUALITY_KEY = 'maxStreamingBitrate';

function bitrateLabel(bps) {
    return bps >= 1e6 ? `${bps / 1e6} Mbps` : `${bps / 1e3} kbps`;
}

// The steps up to the source's bitrate, and the one above it (which plays the source as it is), as Jellyfin Web
// lists them; HEVC and AV1 count at 1.5 times their rate up to 20 Mbps
function qualityOptions(videoBitrate, codec = '') {
    let reference = videoBitrate > 0 ? videoBitrate : 0;
    if (reference && ['hevc', 'h265', 'av1', 'vp9'].includes((codec || '').toLowerCase())) reference = Math.min(reference * 1.5, 20e6);
    if (!reference || reference >= BITRATE_STEPS[0]) return [0, ...BITRATE_STEPS];
    const above = BITRATE_STEPS.filter(b => b > reference).pop();
    return [0, ...(above ? [above] : []), ...BITRATE_STEPS.filter(b => b <= reference)];
}

function maxStreamingBitrate() {
    const value = Number(readSetting(QUALITY_KEY, '0'));
    return Number.isFinite(value) && value > 0 ? value : 0;
}
function setMaxStreamingBitrate(bps) {
    writeSetting(QUALITY_KEY, Number(bps) > 0 ? Number(bps) : null);
}

// --- Aspect ratio (Jellyfin Web's aspectRatio, per device): auto, cover (zoom) or fill (stretch) ---
const ASPECT_RATIOS = ['auto', 'cover', 'fill'];
function aspectRatio() {
    const value = readSetting('aspectRatio', 'auto');
    return ASPECT_RATIOS.includes(value) ? value : 'auto';
}
function setAspectRatio(value) {
    writeSetting('aspectRatio', ASPECT_RATIOS.includes(value) && value !== 'auto' ? value : null);
}

// --- Are you still watching? (Jellyfin Web's stillWatchingPrompt, off by default) ---
const HOUR = 3600e3;
const STILL_WATCHING = {
    short: { count: 2, duration: HOUR },
    default: { count: 3, duration: 1.5 * HOUR },
    long: { count: 5, duration: 2.5 * HOUR },
    veryLong: { count: 8, duration: 4 * HOUR },
};
function stillWatchingSetting() {
    const value = readSetting(userKey('stillWatchingPrompt'), 'disabled');
    return STILL_WATCHING[value] ? value : 'disabled';
}
function setStillWatchingSetting(value) {
    writeSetting(userKey('stillWatchingPrompt'), STILL_WATCHING[value] ? value : null);
}

// Before the next episode: ask once the run has lasted the setting's time and the user has been idle that
// long or has played its number of episodes. now, sessionStart and lastInput in ms.
function stillWatchingDue(setting, { now, sessionStart, lastInput, played }) {
    const rule = STILL_WATCHING[setting];
    if (!rule) return false;
    return now - sessionStart >= rule.duration && (now - lastInput >= rule.duration || played >= rule.count);
}

export {
    languageCode, sameLanguage, trackPrefs, defaultAudioIndex, defaultSubtitleIndex, defaultTracks,
    subtitleAppearance, saveSubtitleAppearance, subtitleCss, APPEARANCE_DEFAULTS, SUBTITLE_SIZES, SUBTITLE_FONTS,
    SUBTITLE_SHADOWS, stepSubtitleOffset, SEGMENT_TYPES, SEGMENT_ACTIONS, segmentAction, setSegmentAction,
    BITRATE_STEPS, bitrateLabel, qualityOptions, maxStreamingBitrate, setMaxStreamingBitrate,
    ASPECT_RATIOS, aspectRatio, setAspectRatio, STILL_WATCHING, stillWatchingSetting, setStillWatchingSetting,
    stillWatchingDue,
};
