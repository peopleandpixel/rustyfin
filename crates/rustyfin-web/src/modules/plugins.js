// Plugin configuration and provider priority dialogs
import { $, show, hide, toast, escapeHtml } from './util.js';
import { providerKeyField } from './settings.js';
import { renderAdmin, showAdminTab } from './admin.js';

// The JSON editor saves a merge patch: a top-level field the user deleted is sent as null,
// and PluginId (added by GET) is never sent back.
function pluginConfigPatch(original, edited) {
    const isObject = v => v !== null && typeof v === 'object' && !Array.isArray(v);
    if (!isObject(edited)) return edited;
    const patch = { ...edited };
    delete patch.PluginId;
    if (isObject(original)) {
        for (const key of Object.keys(original)) {
            if (key !== 'PluginId' && !Object.hasOwn(edited, key)) patch[key] = null;
        }
    }
    return patch;
}

// --- Plugin Configuration Modal ---
let currentPluginConfigId = null;
let currentPluginConfigOriginal = null;
let currentPluginConfigProvider = false;
let currentPluginPriorityId = null;
const PROVIDER_PLUGIN_NAMES = { tmdb: 'TMDb', omdb: 'OMDb', fanart: 'Fanart.tv' };

// TMDb's unaired/missing episode options, keyed by the server's config names. min is set for numbers.
const EPISODE_OPTIONS = [
    { key: 'ImportUnairedEpisodes', label: 'admin.tmdb_import_unaired', desc: 'admin.tmdb_import_unaired_desc', def: false },
    { key: 'ImportMissingEpisodes', label: 'admin.tmdb_import_missing', desc: 'admin.tmdb_import_missing_desc', def: false },
    { key: 'ImportSpecials', label: 'admin.tmdb_import_specials', desc: 'admin.tmdb_import_specials_desc', def: false },
    { key: 'MissingEpisodeRefreshIntervalDays', label: 'admin.tmdb_refresh_days', desc: 'admin.tmdb_refresh_days_desc', def: 7, min: 1, max: 365 },
    { key: 'UpcomingEpisodeGracePeriodDays', label: 'admin.tmdb_grace_days', desc: 'admin.tmdb_grace_days_desc', def: 7, min: 0, max: 365 },
];

// Builds the episode options under the key field. The TV libraries come from /Library/VirtualFolders.
async function renderEpisodeOptions(config) {
    let libs = null;
    try {
        libs = (await API.get('/Library/VirtualFolders')).filter(l => l.CollectionType === 'tvshows');
    } catch { /* no library list: saving then leaves the stored choice alone */ }
    const enabled = Array.isArray(config.EnabledMissingEpisodeLibraries) ? config.EnabledMissingEpisodeLibraries : [];
    const options = EPISODE_OPTIONS.map(o => {
        const text = `<span class="block text-white/80">${escapeHtml(t(o.label))}</span><span class="block text-white/40">${escapeHtml(t(o.desc))}</span>`;
        if (o.min === undefined) {
            return `<label class="flex items-start gap-2 text-xs text-white/70"><input type="checkbox" data-cfg="${o.key}" class="checkbox checkbox-xs mt-0.5"${config[o.key] === true ? ' checked' : ''}><span>${text}</span></label>`;
        }
        return `<div class="text-xs"><div class="mb-1">${text}</div><input type="number" data-cfg="${o.key}" min="${o.min}" max="${o.max}" value="${config[o.key] ?? o.def}" class="jf-input w-full"></div>`;
    }).join('');
    let libsHtml = '';
    if (libs) {
        const rows = libs.length
            ? libs.map(l => `<label class="flex items-center gap-2 text-xs text-white/70"><input type="checkbox" data-lib="${escapeHtml(l.ItemId)}" class="checkbox checkbox-xs"${enabled.includes(l.ItemId) ? ' checked' : ''}><span>${escapeHtml(l.Name)}</span></label>`).join('')
            : `<p class="text-xs text-white/40">${escapeHtml(t('admin.tmdb_no_libraries'))}</p>`;
        libsHtml = `<div class="space-y-1.5"><div class="text-xs text-white/80">${escapeHtml(t('admin.tmdb_libraries'))}</div><div class="text-xs text-white/40">${escapeHtml(t('admin.tmdb_libraries_desc'))}</div>${rows}</div>`;
    }
    const box = $('plugin-config-episodes');
    box.innerHTML = options + libsHtml;
    box.dataset.libsOk = libs ? '1' : '';
    show('plugin-config-episodes');
}

// The episode options as config values. The library list is sent only when it loaded.
function readEpisodeOptions() {
    const box = $('plugin-config-episodes');
    const out = {};
    for (const o of EPISODE_OPTIONS) {
        const input = box.querySelector(`[data-cfg="${o.key}"]`);
        if (o.min === undefined) {
            out[o.key] = input.checked;
        } else {
            const n = Math.round(Number(input.value || o.def));
            out[o.key] = Math.min(o.max, Math.max(o.min, Number.isFinite(n) ? n : o.def));
        }
    }
    if (box.dataset.libsOk) {
        out.EnabledMissingEpisodeLibraries = [...box.querySelectorAll('[data-lib]:checked')].map(el => el.dataset.lib);
    }
    return out;
}

async function openPluginConfig(pluginId) {
    currentPluginConfigId = pluginId;
    const dlg = $('plugin-config-modal');
    const err = $('plugin-config-error');
    if (!dlg) return;
    hide(err);
    let config = {};
    try {
        config = await API.getPluginConfig(pluginId) ?? {};
    } catch {
        config = {};
    }
    currentPluginConfigProvider = Object.hasOwn(PROVIDER_PLUGIN_NAMES, pluginId);
    if (currentPluginConfigProvider) {
        // The stored key never reaches the DOM: only whether one is saved, and the status.
        const keySet = config.api_key_set === true;
        currentPluginConfigOriginal = '{}';
        $('plugin-config-textarea').value = '{}';
        $('plugin-config-key').placeholder = t(keySet ? 'settings.key_saved' : 'common.optional');
        $('plugin-config-key-label').textContent = t('settings.api_key', { service: PROVIDER_PLUGIN_NAMES[pluginId] ?? pluginId });
        keySet ? show('plugin-config-key-clear-row') : hide('plugin-config-key-clear-row');
        let status = config.status;
        if (!status) {
            try {
                status = (await API.getPlugins()).find(p => p.Id === pluginId)?.Status;
            } catch { /* leave unchecked */ }
        }
        $('plugin-config-enabled').checked = status === 'Active';
        if (pluginId === 'tmdb') await renderEpisodeOptions(config);
        else hide('plugin-config-episodes');
        show('plugin-config-provider');
        hide('plugin-config-json-block');
    } else {
        currentPluginConfigOriginal = JSON.stringify(config, null, 2);
        $('plugin-config-textarea').value = currentPluginConfigOriginal;
        hide('plugin-config-provider');
        show('plugin-config-json-block');
    }
    dlg.classList.add('modal-open');
    if (typeof dlg.showModal === 'function') {
        try { dlg.showModal(); } catch { dlg.setAttribute('open', ''); }
    } else {
        dlg.setAttribute('open', '');
    }
}

function closePluginConfig() {
    const dlg = $('plugin-config-modal');
    if (dlg) {
        dlg.classList.remove('modal-open');
        if (typeof dlg.close === 'function') {
            try { dlg.close(); } catch { dlg.removeAttribute('open'); }
        } else {
            dlg.removeAttribute('open');
        }
    }
    currentPluginConfigId = null;
    currentPluginConfigOriginal = null;
    currentPluginConfigProvider = false;
    $('plugin-config-key').value = '';
    $('plugin-config-key-clear').checked = false;
    $('plugin-config-enabled').checked = false;
}

async function savePluginConfig() {
    const err = $('plugin-config-error');
    const textarea = $('plugin-config-textarea');
    let parsed;
    if (currentPluginConfigProvider) {
        parsed = { status: $('plugin-config-enabled').checked ? 'Active' : 'Disabled' };
        if (currentPluginConfigId === 'tmdb') Object.assign(parsed, readEpisodeOptions());
        const key = providerKeyField($('plugin-config-key').value, $('plugin-config-key-clear').checked);
        if (key === '') parsed.api_key = null;
        else if (key !== undefined) parsed.api_key = key;
    } else {
        try {
            parsed = pluginConfigPatch(JSON.parse(currentPluginConfigOriginal || '{}'), JSON.parse(textarea.value));
        } catch (e) {
            err.textContent = t('admin.plugin_config_failed', { message: 'Invalid JSON' });
            show(err);
            return;
        }
    }
    hide(err);
    try {
        await API.setPluginConfig(currentPluginConfigId, parsed);
        toast(t('admin.plugin_config_saved'));
        closePluginConfig();
        API.invalidateCache('/Plugins');
        await renderAdmin();
        const tab = document.querySelector('[data-action="admin-tab"][data-tab="admin-plugins"]');
        if (tab) showAdminTab(tab, 'admin-plugins');
    } catch (e) {
        err.textContent = t('admin.plugin_config_failed', { message: e.message });
        show(err);
    }
}

// --- Plugin Priority Modal ---
function openPluginPriority(pluginId) {
    currentPluginPriorityId = pluginId;
    const dlg = $('plugin-priority-modal');
    const err = $('plugin-priority-error');
    if (!dlg) return;
    hide(err);
    // Find the current priority from the plugin list
    const display = document.querySelector(`.plugin-priority-display[data-plugin-id="${pluginId}"]`);
    $('plugin-priority-input').value = display ? display.textContent : '100';
    dlg.classList.add('modal-open');
    if (typeof dlg.showModal === 'function') {
        try { dlg.showModal(); } catch { dlg.setAttribute('open', ''); }
    } else {
        dlg.setAttribute('open', '');
    }
}

function closePluginPriority() {
    const dlg = $('plugin-priority-modal');
    if (dlg) {
        dlg.classList.remove('modal-open');
        if (typeof dlg.close === 'function') {
            try { dlg.close(); } catch { dlg.removeAttribute('open'); }
        } else {
            dlg.removeAttribute('open');
        }
    }
    currentPluginPriorityId = null;
}

async function savePluginPriority() {
    const err = $('plugin-priority-error');
    const input = $('plugin-priority-input');
    const priority = parseInt(input.value, 10);
    if (isNaN(priority) || priority < 0 || priority > 999) {
        err.textContent = t('admin.plugin_priority_failed', { message: 'Value must be 0–999' });
        show(err);
        return;
    }
    hide(err);
    try {
        await API.setPluginPriority(currentPluginPriorityId, priority);
        toast(t('admin.plugin_priority_saved'));
        closePluginPriority();
        API.invalidateCache('/Plugins');
        await renderAdmin();
        const tab = document.querySelector('[data-action="admin-tab"][data-tab="admin-plugins"]');
        if (tab) showAdminTab(tab, 'admin-plugins');
    } catch (e) {
        err.textContent = t('admin.plugin_priority_failed', { message: e.message });
        show(err);
    }
}

export {
    openPluginConfig, closePluginConfig, savePluginConfig, openPluginPriority, closePluginPriority, savePluginPriority,
};
