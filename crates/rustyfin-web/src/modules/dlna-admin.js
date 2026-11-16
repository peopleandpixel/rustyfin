// The admin dashboard's DLNA tab: the DLNA server switch (EnableDlnaServer) and the user DLNA
// devices browse and stream as (DlnaDefaultUserId). DLNA devices don't sign in, so they see that
// user's libraries and parental limits, and nothing until a user is chosen. A save posts back the
// whole server configuration the tab loaded, so the fields the tab doesn't show are kept.
import { $, show, hide, toast, escapeHtml } from './util.js';

const LABEL = 'block text-xs font-semibold text-white/70 mb-1.5';
const CHECKBOX = 'w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]';

// The server configuration the tab loaded, posted back with the form's values on save
let loaded = null;

function dlnaPanelHtml() {
    return '<div id="admin-dlna" class="hidden jf-settings-card"></div>';
}

// An id in one spelling, so a stored id matches a user's whatever its hyphens or case
function sameId(a, b) {
    const norm = id => String(id ?? '').replace(/-/g, '').toLowerCase();
    return norm(a) !== '' && norm(a) === norm(b);
}

// The tab's form: the switch, the user picker (no user first) and what the user decides
function dlnaSettingsHtml(config, users) {
    const chosen = users.find(u => sameId(u.Id, config.DlnaDefaultUserId));
    const option = u => {
        const name = u.Policy?.IsDisabled ? t('dlna_admin.user_disabled', { name: u.Name }) : u.Name;
        return `<option value="${escapeHtml(u.Id)}"${u === chosen ? ' selected' : ''}>${escapeHtml(name)}</option>`;
    };
    return `
        <h3 class="font-bold text-white text-base mb-4">${escapeHtml(t('dlna_admin.title'))}</h3>
        <form id="dlna-settings" class="space-y-4" onsubmit="return false">
            <label class="flex items-center gap-2 text-sm text-white/80">
                <input type="checkbox" id="dlna-enabled" class="${CHECKBOX}"${config.EnableDlnaServer ? ' checked' : ''}>
                ${escapeHtml(t('dlna_admin.enable'))}
            </label>
            <div>
                <label for="dlna-user" class="${LABEL}">${escapeHtml(t('dlna_admin.user'))}</label>
                <select id="dlna-user" class="jf-input w-full max-w-sm">
                    <option value=""${chosen ? '' : ' selected'}>${escapeHtml(t('dlna_admin.no_user'))}</option>
                    ${users.map(option).join('')}
                </select>
                <p class="text-xs text-white/40 mt-1">${escapeHtml(t('dlna_admin.user_hint'))}</p>
                ${chosen ? '' : `<p id="dlna-no-user" class="text-xs text-amber-400 mt-1">${escapeHtml(t('dlna_admin.no_user_warning'))}</p>`}
            </div>
            <p id="dlna-settings-error" class="hidden text-xs text-red-400"></p>
            <button type="button" class="jf-btn-primary text-xs" data-action="dlna-save">${escapeHtml(t('dlna_admin.save'))}</button>
        </form>`;
}

// The configuration with the form's values over the loaded one; no user is stored as null
function readDlnaSettings(form, config) {
    return {
        ...config,
        EnableDlnaServer: form.querySelector('#dlna-enabled').checked,
        DlnaDefaultUserId: form.querySelector('#dlna-user').value || null,
    };
}

async function loadDlnaAdmin() {
    const box = $('admin-dlna');
    if (!box) return;
    let users;
    try {
        [loaded, users] = await Promise.all([API.getConfig(), API.getUsers()]);
    } catch (err) {
        box.innerHTML = `<p class="text-xs text-red-400">${escapeHtml(t('dlna_admin.load_failed'))}: ${escapeHtml(err.message)}</p>`;
        return;
    }
    box.innerHTML = dlnaSettingsHtml(loaded, users);
}

// The form stays as chosen when a save fails: the error goes into the card
async function saveDlnaSettings() {
    const form = $('dlna-settings');
    if (!form || !loaded) return;
    const values = readDlnaSettings(form, loaded);
    hide('dlna-settings-error');
    try {
        await API.updateConfig(values);
    } catch (err) {
        $('dlna-settings-error').textContent = t('dlna_admin.save_failed', { error: err.message });
        show('dlna-settings-error');
        return;
    }
    loaded = values;
    const warning = $('dlna-no-user');
    if (warning) warning.classList.toggle('hidden', !!values.DlnaDefaultUserId);
    toast(t('dlna_admin.saved'));
}

export { dlnaPanelHtml, loadDlnaAdmin, saveDlnaSettings };
