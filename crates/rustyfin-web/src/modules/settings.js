// The user settings page and the server configuration form
import { $, toast, escapeHtml } from './util.js';
import { showOidcLinkResult, oidcCardHtml } from './auth.js';

// --- Settings Page Renderer ---
async function renderSettings(data) {
    showOidcLinkResult(data);
    const page = $('page-settings');
    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        const [me, config, ssoInfo, ssoAccount] = await Promise.all([
            API.getMe().catch(() => ({})),
            API.getConfig().catch(() => null),
            API.oidcInfo().catch(() => null),
            API.oidcAccount(API.userId).catch(() => null)
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
                <div class="w-16 h-16 rounded-2xl bg-gradient-to-tr from-[#00a4dc] to-[#38bdf8] flex items-center justify-center text-2xl font-bold text-white shadow-xl">
                    ${escapeHtml(name[0]?.toUpperCase() || 'U')}
                </div>
                <div>
                    <h2 class="text-lg font-bold text-white">${escapeHtml(name)}</h2>
                    <p class="text-xs text-white/40">${t('settings.user_id', { id: escapeHtml(id) })}</p>
                    <span class="inline-block mt-1 text-[10px] font-bold px-2 py-0.5 rounded bg-[#00a4dc]/20 text-[#38bdf8] uppercase">
                        ${API.isAdmin ? t('common.administrator') : t('settings.standard_user')}
                    </span>
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
            </div>

            <!-- Playback card -->
            <div class="jf-settings-card">
                <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                    <span class="w-2 h-2 rounded-full bg-sky-400"></span>
                    ${t('settings.playback')}
                </h3>
                <label class="flex items-center gap-3 text-sm text-white/80 cursor-pointer">
                    <input id="pref-next-episode-autoplay" type="checkbox" class="toggle toggle-sm toggle-primary" ${me.Configuration?.EnableNextEpisodeAutoPlay !== false ? 'checked' : ''} onchange="changeNextEpisodeAutoPlay(this)">
                    ${t('settings.next_episode_autoplay')}
                </label>
            </div>

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
};
