// The signed-in user's settings: Jellyfin's UserConfiguration (/Users/{id}/Configuration) and the
// settings Jellyfin Web keeps in its userSettings, saved here as the CustomPrefs of the
// "usersettings" display preferences (client "emby") so they follow the account to every browser.
// Both are loaded once per user and kept; a save updates the copy.
import { deviceSetting, setDeviceSetting } from './playerprefs.js';

// The home sections Rustyfin has, by Jellyfin Web's ids
const HOME_SECTIONS = ['smalllibrarytiles', 'resume', 'resumeaudio', 'nextup', 'latestmedia'];
// Jellyfin Web's ten home section slots (homesection0..9) and each one's default (its DEFAULT_SECTIONS)
const HOME_SLOT_DEFAULTS = [
    'smalllibrarytiles', 'resume', 'resumeaudio', 'resumebook', 'livetv', 'nextup', 'latestmedia', 'none', 'none', 'none',
];

// The settings Jellyfin Web keeps only on the device, which the player reads there (playerprefs.js):
// true for those kept per user on the device. The device's value wins, as in Jellyfin Web; the
// account's fills a device that has none, and a save here writes both.
// The screensaver and its idle time are kept per user on the device too (screensaver.js): Jellyfin Web's
// backdrop screensaver, its default idle time and the idle times the Display settings offer, in seconds.
const SCREENSAVER_BACKDROP = 'backdropscreensaver';
const DEFAULT_IDLE_SECONDS = 180;
const SCREENSAVER_TIMES = [60, 180, 300, 600, 900, 1800];
const DEVICE_SETTINGS = {
    maxStreamingBitrate: false, subtitleburnin: false, localplayersubtitleappearance3: true, screensaver: true,
    screensaverTime: true,
};

let loaded = null; // { userId, config, prefs }
let loading = null;

function prefsPath(userId) {
    return `/DisplayPreferences/usersettings?userId=${encodeURIComponent(userId)}&client=emby`;
}

// The user's configuration and display preferences; a failed request leaves Jellyfin's defaults
function loadUserSettings({ force = false } = {}) {
    const userId = API.userId;
    if (!force && loaded?.userId === userId) return Promise.resolve(loaded);
    if (!force && loading?.userId === userId) return loading.promise;
    const promise = Promise.all([
        API.getMe().catch(() => null),
        API.get(prefsPath(userId)).catch(() => null),
    ]).then(([me, prefs]) => {
        const result = {
            userId,
            config: me?.Configuration || {},
            prefs: { ...(prefs || {}), CustomPrefs: { ...(prefs?.CustomPrefs || {}) } },
        };
        if (API.userId === userId) {
            loaded = result;
            for (const [name, perUser] of Object.entries(DEVICE_SETTINGS)) {
                const saved = result.prefs.CustomPrefs[name];
                if (saved != null && saved !== '' && deviceSetting(name, perUser) === null) setDeviceSetting(name, saved, perUser);
            }
            applyUserSettings();
            showAvatar(me?.PrimaryImageTag || null);
        }
        return result;
    }).finally(() => {
        if (loading?.promise === promise) loading = null;
    });
    loading = { userId, promise };
    return promise;
}

// A setting as stored (a string): the device's for a device setting that it has, else the account's
// CustomPrefs value; fallback before the settings load or when unset
function userSetting(name, fallback = null) {
    if (name in DEVICE_SETTINGS) {
        const device = deviceSetting(name, DEVICE_SETTINGS[name]);
        if (device !== null && device !== '') return device;
    }
    const value = loaded?.userId === API.userId ? loaded.prefs.CustomPrefs[name] : undefined;
    return value == null ? fallback : value;
}

// The user's Jellyfin UserConfiguration, as last loaded or saved
function userConfiguration() {
    return loaded?.userId === API.userId ? loaded.config : {};
}

// Saves CustomPrefs values (strings; null or '' removes one), keeping the other settings
async function saveUserSettings(values) {
    const settings = await loadUserSettings();
    const customPrefs = { ...settings.prefs.CustomPrefs };
    for (const [name, value] of Object.entries(values)) {
        if (value == null || value === '') delete customPrefs[name];
        else customPrefs[name] = String(value);
    }
    const prefs = { ...settings.prefs, CustomPrefs: customPrefs };
    await API.post(prefsPath(settings.userId), prefs);
    settings.prefs = prefs;
    for (const [name, value] of Object.entries(values)) {
        if (name in DEVICE_SETTINGS) setDeviceSetting(name, value, DEVICE_SETTINGS[name]);
    }
    applyUserSettings();
}

// Saves UserConfiguration fields over the current configuration, as Jellyfin Web does
async function saveUserConfiguration(fields) {
    const settings = await loadUserSettings();
    const me = await API.getMe();
    const config = { ...(me.Configuration || {}), ...fields };
    await API.post(`/Users/${me.Id}/Configuration`, config);
    settings.config = config;
}

// Each of the ten slots' section as Jellyfin Web reads it: the saved one, else the slot's default
// ('folders', an old id, is the first slot's default)
function homeSlots(get = userSetting) {
    return HOME_SLOT_DEFAULTS.map((fallback, i) => {
        const value = get(`homesection${i}`) || fallback;
        return value === 'folders' ? HOME_SLOT_DEFAULTS[0] : value;
    });
}

// The home sections to show, in slot order. Sections Rustyfin lacks (Live TV, Continue Reading,
// library buttons, recordings) and 'none' are skipped, and a section set twice shows once. Only when
// no slot was ever saved are Rustyfin's sections that no default slot names added at the end.
function homeSections(get = userSetting) {
    const sections = [];
    for (const value of homeSlots(get)) {
        if (HOME_SECTIONS.includes(value) && !sections.includes(value)) sections.push(value);
    }
    const saved = HOME_SLOT_DEFAULTS.some((_, i) => get(`homesection${i}`));
    if (!saved) for (const section of HOME_SECTIONS) if (!sections.includes(section)) sections.push(section);
    return sections;
}

// Sets slot `index` to `section` and returns every slot, as Jellyfin Web saves all ten at once
function homeSlotValues(index, section, get = userSetting) {
    const slots = homeSlots(get);
    slots[index] = section;
    return Object.fromEntries(slots.map((value, i) => [`homesection${i}`, value]));
}

// Whether a library's newest items get a "Recently Added" row (UserConfiguration.LatestItemsExcludes)
function showsInLatest(libraryId, config = userConfiguration()) {
    const key = id => String(id).toLowerCase().replace(/-/g, '');
    return !(config.LatestItemsExcludes || []).some(id => key(id) === key(libraryId));
}

// Jellyfin Web's enableBackdrops. Rustyfin has always shown them, so it stays on unless turned off
function showBackdrops() {
    return userSetting('enableBackdrops', 'true') !== 'false';
}

// The URL of the signed-in user's profile picture with this tag (Jellyfin's /UserImage)
function avatarUrl(tag) {
    return `${API.base}/UserImage?userId=${encodeURIComponent(API.userId)}&tag=${encodeURIComponent(tag)}`;
}

// Shows the profile picture in the header, or the initial when there is none (tag null)
function showAvatar(tag) {
    const img = document.getElementById('nav-avatar-img');
    const letter = document.getElementById('nav-avatar-letter');
    if (!img || !letter) return;
    if (tag) img.src = avatarUrl(tag);
    else img.removeAttribute('src');
    img.hidden = !tag;
    letter.hidden = !!tag;
}

// Applies the settings that change the whole page: backdrops and the date and time locale
function applyUserSettings() {
    document.documentElement.classList.toggle('rf-no-backdrops', !showBackdrops());
    window.setDateTimeLocale?.(userSetting('datetimelocale', ''));
}

export {
    HOME_SECTIONS, HOME_SLOT_DEFAULTS, homeSlots, homeSlotValues, loadUserSettings, userSetting, userConfiguration, saveUserSettings, saveUserConfiguration,
    homeSections, showsInLatest, showBackdrops, applyUserSettings, avatarUrl, showAvatar, SCREENSAVER_BACKDROP,
    DEFAULT_IDLE_SECONDS, SCREENSAVER_TIMES,
};
