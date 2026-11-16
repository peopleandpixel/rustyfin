// The signed-in user's settings: Jellyfin's UserConfiguration (/Users/{id}/Configuration) and the
// settings Jellyfin Web keeps in its userSettings, saved here as the CustomPrefs of the
// "usersettings" display preferences (client "emby") so they follow the account to every browser.
// Both are loaded once per user and kept; a save updates the copy.

// Jellyfin Web's home section ids, in its default order where Rustyfin has the section
const HOME_SECTIONS = ['smalllibrarytiles', 'resume', 'resumeaudio', 'nextup', 'latestmedia'];

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

// A CustomPrefs value as stored (a string), or fallback before the settings load or when unset
function userSetting(name, fallback = null) {
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

// The home sections in the user's order, without hidden ones: Jellyfin Web's homesection0..n, where
// 'none' hides a slot and an unset slot keeps the default; a section set twice shows once
function homeSections(get = userSetting) {
    const sections = [];
    for (let i = 0; i < HOME_SECTIONS.length; i++) {
        const value = get(`homesection${i}`) || HOME_SECTIONS[i];
        if (HOME_SECTIONS.includes(value) && !sections.includes(value)) sections.push(value);
    }
    return sections;
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
    HOME_SECTIONS, loadUserSettings, userSetting, userConfiguration, saveUserSettings, saveUserConfiguration,
    homeSections, showsInLatest, showBackdrops, applyUserSettings, avatarUrl, showAvatar,
};
