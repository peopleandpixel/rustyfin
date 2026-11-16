// The user settings page and the server configuration form
import { $, toast, escapeHtml } from './util.js';
import { showOidcLinkResult, oidcCardHtml } from './auth.js';
import {
    HOME_SECTIONS, homeSlots, homeSlotValues, loadUserSettings, userSetting, saveUserSettings, saveUserConfiguration, showsInLatest, avatarUrl,
} from './user-settings.js';

// --- Settings Page Renderer ---
async function renderSettings(data) {
    showOidcLinkResult(data);
    const page = $('page-settings');
    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        const [me, config, ssoInfo, ssoAccount, cultures, libraries] = await Promise.all([
            API.getMe().catch(() => ({})),
            API.getConfig().catch(() => null),
            API.oidcInfo().catch(() => null),
            API.oidcAccount(API.userId).catch(() => null),
            API.get('/Localization/Cultures').catch(() => []),
            API.getLibraries().catch(() => []),
            loadUserSettings({ force: true }).catch(() => null),
        ]);

        const name = me.Name || me.name || API.userName || '';
        const id = me.Id || me.id || API.userId || '';

        let serverCfgHtml = '';
        if (API.isAdmin && config) {
            serverCfgHtml = `
            <div class="jf-settings-card">
                <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                    <span class="w-2 h-2 rounded-full bg-[#00a4dc]"></span>
                    ${t('settings.server_config')}
                </h3>
                <form onsubmit="handleSaveConfig(event)" autocomplete="off" class="space-y-4">
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">${t('settings.server_name')}</label>
                            <input id="cfg-server-name" class="jf-input w-full" value="${escapeHtml(config.ServerName || config.server_name || 'Rustyfin')}">
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">${t('wizard.metadata_language')}</label>
                            <select id="cfg-meta-lang" class="jf-input w-full">
                                ${['de', 'en', 'fr', 'es', 'it', 'ja', 'zh'].map(l => `<option value="${l}" ${(config.PreferredMetadataLanguage || config.preferred_metadata_language) === l ? 'selected' : ''}>${l.toUpperCase()}</option>`).join('')}
                            </select>
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">${t('settings.http_port')}</label>
                            <input id="cfg-port" type="number" class="jf-input w-full" value="${config.PublicPort || config.public_port || 8096}">
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">${t('settings.api_key', { service: 'TMDb' })}</label>
                            <input id="cfg-tmdb" type="password" autocomplete="new-password" autocapitalize="off" spellcheck="false" data-1p-ignore data-lpignore="true" class="jf-input w-full" placeholder="${escapeHtml(config.TmdbApiKeySet ? t('settings.key_saved') : t('common.optional'))}">
                            ${config.TmdbApiKeySet ? `<label class="flex items-center gap-2 mt-1 text-xs text-white/70"><input id="cfg-tmdb-clear" type="checkbox" class="checkbox checkbox-xs"> ${t('settings.key_clear')}</label>` : ''}
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">${t('settings.api_key', { service: 'OMDb' })}</label>
                            <input id="cfg-omdb" type="password" autocomplete="new-password" autocapitalize="off" spellcheck="false" data-1p-ignore data-lpignore="true" class="jf-input w-full" placeholder="${escapeHtml(config.OmdbApiKeySet ? t('settings.key_saved') : t('common.optional'))}">
                            ${config.OmdbApiKeySet ? `<label class="flex items-center gap-2 mt-1 text-xs text-white/70"><input id="cfg-omdb-clear" type="checkbox" class="checkbox checkbox-xs"> ${t('settings.key_clear')}</label>` : ''}
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">${t('settings.api_key', { service: 'Fanart.tv' })}</label>
                            <input id="cfg-fanart" type="password" autocomplete="new-password" autocapitalize="off" spellcheck="false" data-1p-ignore data-lpignore="true" class="jf-input w-full" placeholder="${escapeHtml(config.FanartApiKeySet ? t('settings.key_saved') : t('common.optional'))}">
                            ${config.FanartApiKeySet ? `<label class="flex items-center gap-2 mt-1 text-xs text-white/70"><input id="cfg-fanart-clear" type="checkbox" class="checkbox checkbox-xs"> ${t('settings.key_clear')}</label>` : ''}
                        </div>
                    </div>
                    <label class="flex items-start gap-2 text-xs text-white/70">
                        <input id="cfg-refuse-links" type="checkbox" class="checkbox checkbox-xs mt-0.5" ${config.RefuseLinksOutOfLibraries ? 'checked' : ''}>
                        <span><span class="font-semibold text-white/80">${t('settings.refuse_links_out')}</span><br>${t('settings.refuse_links_out_help')}</span>
                    </label>
                    <div class="pt-2">
                        <button type="submit" class="jf-btn-primary text-xs">${t('settings.save_config')}</button>
                    </div>
                </form>
            </div>`;
        }

        // Without the account (its request failed) the current password stays required.
        const hasPassword = ssoAccount?.HasPassword ?? true;
        const ssoHtml = (ssoInfo?.Enabled || ssoAccount?.Linked) ? oidcCardHtml(Boolean(ssoInfo?.Enabled), ssoAccount) : '';

        page.innerHTML = `
        <div class="max-w-3xl mx-auto jf-page">
            <h1 class="text-2xl lg:text-3xl font-extrabold text-white mb-6">${t('settings.title')}</h1>

            <!-- User Profile card -->
            <div class="jf-settings-card flex items-center gap-4">
                <div class="w-16 h-16 rounded-2xl bg-gradient-to-tr from-[#00a4dc] to-[#38bdf8] flex items-center justify-center text-2xl font-bold text-white shadow-xl overflow-hidden shrink-0">
                    ${me.PrimaryImageTag
                        ? `<img id="settings-avatar" src="${escapeHtml(avatarUrl(me.PrimaryImageTag))}" alt="" class="w-full h-full object-cover">`
                        : escapeHtml(name[0]?.toUpperCase() || 'U')}
                </div>
                <div>
                    <h2 class="text-lg font-bold text-white">${escapeHtml(name)}</h2>
                    <p class="text-xs text-white/40">${t('settings.user_id', { id: escapeHtml(id) })}</p>
                    <span class="inline-block mt-1 text-[10px] font-bold px-2 py-0.5 rounded bg-[#00a4dc]/20 text-[#38bdf8] uppercase">
                        ${API.isAdmin ? t('common.administrator') : t('settings.standard_user')}
                    </span>
                    <div class="flex flex-wrap gap-2 mt-2">
                        <label class="jf-btn-ghost !py-1 !px-3 text-xs cursor-pointer">
                            ${t('settings.profile_image_choose')}
                            <input id="settings-avatar-file" type="file" accept="image/png,image/jpeg,image/gif,image/webp" class="hidden" onchange="chooseProfileImage(this)">
                        </label>
                        ${me.PrimaryImageTag ? `<button type="button" class="jf-btn-ghost !py-1 !px-3 text-xs" onclick="deleteProfileImage()">${t('settings.profile_image_delete')}</button>` : ''}
                    </div>
                </div>
            </div>

            <!-- Display language card -->
            <div class="jf-settings-card">
                <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                    <span class="w-2 h-2 rounded-full bg-emerald-400"></span>
                    ${t('common.display_language')}
                </h3>
                <select id="pref-ui-lang" class="jf-input w-full sm:w-64" onchange="changeUiLanguage(this.value)">
                    ${uiLanguageOptions().map(o => `<option value="${o.value}" ${o.value === getUiLanguage() ? 'selected' : ''}>${escapeHtml(o.label)}</option>`).join('')}
                </select>
                <p class="text-xs text-white/40 mt-2">${t('settings.display_language_hint')}</p>
                ${displaySettingsHtml()}
            </div>

            ${homeSettingsHtml(me.Configuration || {}, libraries)}

            ${playbackSettingsHtml(me.Configuration || {}, cultures)}

            ${subtitleSettingsHtml(me.Configuration || {}, cultures)}

            <!-- Change Password card -->
            <div class="jf-settings-card">
                <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                    <span class="w-2 h-2 rounded-full bg-amber-400"></span>
                    ${t('settings.change_password')}
                </h3>
                <form id="change-password-form" class="space-y-3">
                    <input type="password" id="pw-current" class="jf-input w-full" placeholder="${escapeHtml(t('settings.current_password'))}" ${hasPassword ? 'required' : ''}>
                    ${hasPassword ? '' : `<p class="text-xs text-white/40">${t('oidc.no_current_password')}</p>`}
                    <input type="password" id="pw-new" class="jf-input w-full" placeholder="${escapeHtml(t('settings.new_password'))}" required>
                    <input type="password" id="pw-confirm" class="jf-input w-full" placeholder="${escapeHtml(t('settings.confirm_new_password'))}" required>
                    <button type="submit" class="jf-btn-primary text-xs">${t('settings.save_password')}</button>
                    <span id="pw-msg" class="text-xs ml-3 hidden"></span>
                </form>
            </div>

            ${ssoHtml}

            ${serverCfgHtml}
        </div>`;
        $('change-password-form')?.addEventListener('submit', e => handleChangePassword(e, id));
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

// Saves Jellyfin's EnableNextEpisodeAutoPlay into the user's configuration, keeping its other settings
async function changeNextEpisodeAutoPlay(input) {
    const enabled = input.checked;
    input.disabled = true;
    try {
        const me = await API.getMe();
        await API.post(`/Users/${me.Id}/Configuration`, { ...(me.Configuration || {}), EnableNextEpisodeAutoPlay: enabled });
        toast(t('settings.next_episode_autoplay_saved'));
    } catch (e) {
        input.checked = !enabled;
        toast(e.message, 'error');
    } finally {
        input.disabled = false;
    }
}

// --- Display, home screen, playback and subtitle settings ---
// The UserConfiguration fields save into /Users/{id}/Configuration; the others are Jellyfin Web's
// userSettings keys, saved in the "usersettings" display preferences (user-settings.js). Each
// control saves when it changes, as the autoplay switch does.

// Jellyfin Web's maximum streaming bitrates, in bits per second ('' is Auto)
const MAX_BITRATES = [
    120e6, 110e6, 100e6, 90e6, 80e6, 70e6, 60e6, 50e6, 40e6, 30e6, 25e6, 20e6, 15e6, 10e6, 8e6, 6e6, 5e6, 4e6,
    3e6, 2e6, 1.5e6, 1e6, 720e3, 420e3,
];
// Date and time formats offered besides the display language's own ('')
const DATE_LOCALES = [
    'en-US', 'en-GB', 'de-DE', 'fr-FR', 'es-ES', 'it-IT', 'nl-NL', 'pl-PL', 'pt-BR', 'ru-RU', 'sv-SE', 'tr-TR',
    'ja-JP', 'ko-KR', 'zh-CN',
];
const SUBTITLE_MODES = ['Default', 'Smart', 'OnlyForced', 'Always', 'None'];
const SUBTITLE_BURN_IN = ['', 'onlyimageformats', 'allcomplexformats', 'all'];
// Jellyfin Web's subtitle appearance, kept as JSON under its localplayersubtitleappearance3 key
const SUBTITLE_APPEARANCE_KEY = 'localplayersubtitleappearance3';
const SUBTITLE_APPEARANCE = {
    textSize: ['smaller', 'small', '', 'large', 'larger', 'extralarge'],
    font: ['', 'typewriter', 'print', 'console', 'cursive', 'casual', 'smallcaps'],
    textColor: ['#ffffff', '#d3d3d3', '#808080', '#ffff00', '#008000', '#00ffff', '#0000ff', '#ff00ff', '#ff0000', '#000000'],
    textBackground: ['transparent', '#000000', 'rgba(0, 0, 0, 0.5)'],
    dropShadow: ['', 'none', 'raised', 'depressed', 'uniform'],
};
// The translation key names of the values that are colours
const SETTING_VALUE_NAMES = {
    '#ffffff': 'white', '#d3d3d3': 'lightgray', '#808080': 'gray', '#ffff00': 'yellow', '#008000': 'green',
    '#00ffff': 'cyan', '#0000ff': 'blue', '#ff00ff': 'magenta', '#ff0000': 'red', '#000000': 'black',
    'transparent': 'transparent', 'rgba(0, 0, 0, 0.5)': 'semiblack',
};

function settingsCard(title, dot, body) {
    return `
            <div class="jf-settings-card">
                <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                    <span class="w-2 h-2 rounded-full ${dot}"></span>
                    ${title}
                </h3>
                <div class="space-y-3">${body}</div>
            </div>`;
}

function selectHtml(label, attrs, options, value) {
    const opts = options.map(o => `<option value="${escapeHtml(o.value)}" ${o.value === value ? 'selected' : ''}>${escapeHtml(o.label)}</option>`).join('');
    return `
                <label class="block">
                    <span class="block text-xs font-semibold text-white/70 mb-1">${label}</span>
                    <select class="jf-input w-full sm:w-72" ${attrs} data-saved="${escapeHtml(value)}">${opts}</select>
                </label>`;
}

function toggleHtml(label, attrs, checked) {
    return `
                <label class="flex items-center gap-3 text-sm text-white/80 cursor-pointer">
                    <input type="checkbox" class="toggle toggle-sm toggle-primary" ${attrs} data-saved="${checked}" ${checked ? 'checked' : ''}>
                    ${label}
                </label>`;
}

// The languages for a preference: none, then each culture by its three-letter code
function languageOptions(cultures) {
    const list = (Array.isArray(cultures) ? cultures : [])
        .filter(c => c.ThreeLetterISOLanguageName)
        .map(c => ({ value: c.ThreeLetterISOLanguageName, label: c.DisplayName || c.Name || c.ThreeLetterISOLanguageName }))
        .sort((a, b) => a.label.localeCompare(b.label));
    return [{ value: '', label: t('settings.any_language') }, ...list];
}

function bitrateLabel(bits) {
    return bits >= 1e6 ? `${bits / 1e6} Mbps` : `${bits / 1e3} kbps`;
}

// A locale's name with today's date in its short form, e.g. "English (United Kingdom): 15/11/2026"
function dateLocaleLabel(tag) {
    let name = tag;
    try {
        name = new Intl.DisplayNames([uiLocale()], { type: 'language' }).of(tag) || tag;
    } catch {}
    return `${name}: ${new Date().toLocaleDateString(tag)}`;
}

function displaySettingsHtml() {
    const dateOptions = [{ value: '', label: t('settings.date_format_auto') }, ...DATE_LOCALES.map(tag => ({ value: tag, label: dateLocaleLabel(tag) }))];
    return selectHtml(t('settings.date_format'), 'id="pref-datetimelocale" data-pref="datetimelocale" onchange="changeUserPref(this)"', dateOptions, userSetting('datetimelocale', ''))
        + toggleHtml(t('settings.show_backdrops'), 'id="pref-backdrops" data-pref="enableBackdrops" onchange="changeUserPref(this)"', userSetting('enableBackdrops', 'true') !== 'false');
}

function homeSettingsHtml(config, libraries) {
    const names = {
        smalllibrarytiles: t('home.my_media'),
        resume: t('home.continue_watching'),
        resumeaudio: t('home.continue_listening'),
        nextup: t('home.next_up'),
        latestmedia: t('settings.home_latest'),
    };
    const options = [...HOME_SECTIONS.map(s => ({ value: s, label: names[s] })), { value: 'none', label: t('settings.home_none') }];
    // Jellyfin Web's ten slots. A slot holding a section Rustyfin lacks keeps it as an option, so
    // saving another slot writes it back unchanged.
    const sections = homeSlots().map((value, i) => selectHtml(
        t('settings.home_section', { n: i + 1 }),
        `data-home-slot="${i}" onchange="changeHomeSection(this)"`,
        options.some(o => o.value === value) ? options : [...options, { value, label: t('settings.home_jellyfin_only', { id: value }) }],
        value,
    )).join('');
    const libs = (libraries || []).map(lib => {
        const libId = lib.Id || lib.id;
        return toggleHtml(escapeHtml(lib.Name || lib.name || ''), `data-latest-library="${escapeHtml(libId)}" onchange="changeLatestLibrary(this)"`, showsInLatest(libId, config));
    }).join('');
    return settingsCard(t('settings.home'), 'bg-violet-400', sections + (libs ? `
                <p class="text-xs font-semibold text-white/70 pt-2">${t('settings.home_show_in_latest')}</p>${libs}` : ''));
}

function playbackSettingsHtml(config, cultures) {
    const bitrates = [{ value: '', label: t('settings.bitrate_auto') }, ...MAX_BITRATES.map(b => ({ value: String(b), label: bitrateLabel(b) }))];
    return settingsCard(t('settings.playback'), 'bg-sky-400',
        selectHtml(t('settings.audio_language'), 'id="pref-audio-language" data-config="AudioLanguagePreference" onchange="changeUserConfig(this)"', languageOptions(cultures), config.AudioLanguagePreference || '')
        + toggleHtml(t('settings.play_default_audio'), 'data-config="PlayDefaultAudioTrack" onchange="changeUserConfig(this)"', config.PlayDefaultAudioTrack !== false)
        + selectHtml(t('settings.max_bitrate'), 'id="pref-max-bitrate" data-pref="maxStreamingBitrate" onchange="changeUserPref(this)"', bitrates, userSetting('maxStreamingBitrate', ''))
        + `
                <label class="flex items-center gap-3 text-sm text-white/80 cursor-pointer">
                    <input id="pref-next-episode-autoplay" type="checkbox" class="toggle toggle-sm toggle-primary" ${config.EnableNextEpisodeAutoPlay !== false ? 'checked' : ''} onchange="changeNextEpisodeAutoPlay(this)">
                    ${t('settings.next_episode_autoplay')}
                </label>`
        + toggleHtml(t('settings.remember_audio'), 'data-config="RememberAudioSelections" onchange="changeUserConfig(this)"', config.RememberAudioSelections !== false)
        + toggleHtml(t('settings.remember_subtitles'), 'data-config="RememberSubtitleSelections" onchange="changeUserConfig(this)"', config.RememberSubtitleSelections !== false));
}

// The saved subtitle appearance; a broken value counts as none
function subtitleAppearance() {
    try {
        const value = JSON.parse(userSetting(SUBTITLE_APPEARANCE_KEY, '{}'));
        return value && typeof value === 'object' ? value : {};
    } catch {
        return {};
    }
}

function subtitleSettingsHtml(config, cultures) {
    const option = (group, value) => ({ value, label: t(`settings.subtitle_${group}_${SETTING_VALUE_NAMES[value] || value || 'default'}`) });
    const appearance = subtitleAppearance();
    const appearanceSelects = Object.entries(SUBTITLE_APPEARANCE).map(([field, values]) => selectHtml(
        t(`settings.subtitle_${field}`),
        `data-appearance="${field}" onchange="changeSubtitleAppearance(this)"`,
        values.map(v => option(field, v)),
        appearance[field] ?? (field === 'textColor' ? '#ffffff' : field === 'textBackground' ? 'transparent' : ''),
    )).join('');
    return settingsCard(t('settings.subtitles'), 'bg-rose-400',
        selectHtml(t('settings.subtitle_mode'), 'id="pref-subtitle-mode" data-config="SubtitleMode" onchange="changeUserConfig(this)"', SUBTITLE_MODES.map(m => option('mode', m)), config.SubtitleMode || 'Default')
        + selectHtml(t('settings.subtitle_language'), 'id="pref-subtitle-language" data-config="SubtitleLanguagePreference" onchange="changeUserConfig(this)"', languageOptions(cultures), config.SubtitleLanguagePreference || '')
        + selectHtml(t('settings.subtitle_burn_in'), 'id="pref-subtitle-burn-in" data-pref="subtitleburnin" onchange="changeUserPref(this)"', SUBTITLE_BURN_IN.map(b => option('burnin', b)), userSetting('subtitleburnin', ''))
        + `
                <p class="text-xs font-semibold text-white/70 pt-2">${t('settings.subtitle_appearance')}</p>` + appearanceSelects);
}

function controlValue(input) {
    return input.type === 'checkbox' ? String(input.checked) : input.value;
}

// Saves one control with save(value); a failed save puts the control back and says why
async function saveControl(input, save) {
    const value = controlValue(input);
    input.disabled = true;
    try {
        await save(value);
        input.dataset.saved = value;
        toast(t('settings.saved'));
    } catch (e) {
        if (input.type === 'checkbox') input.checked = input.dataset.saved === 'true';
        else input.value = input.dataset.saved ?? '';
        toast(e.message, 'error');
    } finally {
        input.disabled = false;
    }
}

// A UserConfiguration field: a switch saves true or false, a list its value
function changeUserConfig(input) {
    const field = input.dataset.config;
    return saveControl(input, value => saveUserConfiguration({ [field]: input.type === 'checkbox' ? value === 'true' : value }));
}

// A userSettings value, stored as a string as Jellyfin Web stores it
function changeUserPref(input) {
    return saveControl(input, value => saveUserSettings({ [input.dataset.pref]: value }));
}

function changeSubtitleAppearance(input) {
    return saveControl(input, value => saveUserSettings({
        [SUBTITLE_APPEARANCE_KEY]: JSON.stringify({ ...subtitleAppearance(), [input.dataset.appearance]: value }),
    }));
}

// A home section slot: every slot is saved, as Jellyfin Web saves them
function changeHomeSection(input) {
    return saveControl(input, value => saveUserSettings(homeSlotValues(Number(input.dataset.homeSlot), value)));
}

// A library's "Show in Latest" switch: off adds it to UserConfiguration.LatestItemsExcludes
function changeLatestLibrary(input) {
    const libId = input.dataset.latestLibrary;
    return saveControl(input, async value => {
        const me = await API.getMe();
        const key = id => String(id).toLowerCase().replace(/-/g, '');
        const others = (me.Configuration?.LatestItemsExcludes || []).filter(id => key(id) !== key(libId));
        await saveUserConfiguration({ LatestItemsExcludes: value === 'true' ? others : [...others, libId] });
    });
}

// Uploads the chosen picture base64-encoded, as Jellyfin Web sends it to /UserImage
async function chooseProfileImage(input) {
    const file = input.files?.[0];
    if (!file) return;
    try {
        const dataUrl = await new Promise((resolve, reject) => {
            const reader = new FileReader();
            reader.onload = () => resolve(String(reader.result));
            reader.onerror = () => reject(reader.error);
            reader.readAsDataURL(file);
        });
        const path = `/UserImage?userId=${encodeURIComponent(API.userId)}`;
        const res = await fetch(API.base + path, {
            method: 'POST',
            headers: { ...API.headers(), 'Content-Type': file.type || 'image/jpeg' },
            body: dataUrl.slice(dataUrl.indexOf(',') + 1),
        });
        if (!res.ok) throw await API._requestError('POST', path, res);
        toast(t('settings.profile_image_saved'));
    } catch (e) {
        toast(e.message, 'error');
    }
    await loadUserSettings({ force: true }).catch(() => null);
    renderSettings();
}

async function deleteProfileImage() {
    try {
        await API.del(`/UserImage?userId=${encodeURIComponent(API.userId)}`);
        toast(t('settings.profile_image_deleted'));
    } catch (e) {
        toast(e.message, 'error');
    }
    await loadUserSettings({ force: true }).catch(() => null);
    renderSettings();
}

// Pages keep the language they were rendered in, so reload to switch all of them
async function changeUiLanguage(lng) {
    await setUiLanguage(lng);
    location.reload();
}

async function handleChangePassword(e, userId) {
    e.preventDefault();
    const cur = $('pw-current').value;
    const n = $('pw-new').value;
    const c = $('pw-confirm').value;
    if (n !== c) {
        toast(t('settings.passwords_mismatch'), 'error');
        return;
    }
    try {
        await API.changePassword(userId, cur, n);
        toast(t('settings.password_changed'));
        $('pw-current').value = '';
        $('pw-new').value = '';
        $('pw-confirm').value = '';
    } catch (err) {
        toast(err.message, 'error');
    }
}

// A provider key field: the trimmed typed value, "" to clear the saved key, or undefined to leave it alone.
function providerKeyField(value, clear) {
    const typed = typeof value === 'string' ? value.trim() : '';
    if (typed) return typed;
    if (clear) return '';
    return undefined;
}

async function handleSaveConfig(e) {
    e.preventDefault();
    const body = {
        ServerName: $('cfg-server-name')?.value || 'Rustyfin',
        PreferredMetadataLanguage: $('cfg-meta-lang')?.value || 'de',
        PublicPort: parseInt($('cfg-port')?.value) || 8096,
        RefuseLinksOutOfLibraries: Boolean($('cfg-refuse-links')?.checked),
        IsStartupWizardCompleted: true
    };
    const keys = {
        TmdbApiKey: providerKeyField($('cfg-tmdb')?.value, $('cfg-tmdb-clear')?.checked),
        OmdbApiKey: providerKeyField($('cfg-omdb')?.value, $('cfg-omdb-clear')?.checked),
        FanartApiKey: providerKeyField($('cfg-fanart')?.value, $('cfg-fanart-clear')?.checked)
    };
    for (const [name, value] of Object.entries(keys)) {
        if (value !== undefined) body[name] = value;
    }
    try {
        await API.updateConfig(body);
        toast(t('settings.server_config_updated'));
        renderSettings();
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- Admin Dashboard Renderer ---
// AccountStatus.UserId has hyphens and a user's Id may not, so account ids are compared without case and hyphens.
function oidcAccountKey(id) { return String(id).toLowerCase().replace(/-/g, ''); }

export {
    renderSettings, changeNextEpisodeAutoPlay, changeUiLanguage, providerKeyField, handleSaveConfig, oidcAccountKey,
    changeUserConfig, changeUserPref, changeHomeSection, changeSubtitleAppearance, changeLatestLibrary, chooseProfileImage,
    deleteProfileImage, homeSettingsHtml, playbackSettingsHtml, subtitleSettingsHtml, displaySettingsHtml,
};
