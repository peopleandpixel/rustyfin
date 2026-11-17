// The admin dashboard's Networking, API keys, Devices, Branding and Backups tabs, as Jellyfin Web's
// pages of the same names. Each tab is filled when it is shown; its buttons carry data-sa="…" and
// are handled by the panel's own click handler, so no server value ends up in inline JavaScript.
import { $, toast, escapeHtml } from './util.js';
import { confirmDialog, serverPower } from './dashboard.js';
import { applyBrandingCss } from './branding.js';

const LABEL = 'block text-xs font-semibold text-white/70 mb-1.5';
const CHECKBOX = 'w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]';
const BTN = 'jf-btn-ghost !py-1.5 !px-3 text-xs';

const SERVER_ADMIN_TABS = ['admin-network', 'admin-apikeys', 'admin-devices', 'admin-branding', 'admin-backups'];

function serverAdminPanelsHtml() {
    return SERVER_ADMIN_TABS.map(id => `<div id="${id}" class="hidden jf-settings-card"></div>`).join('');
}

function formatDate(value) {
    if (!value) return '—';
    const date = new Date(value);
    return Number.isNaN(date.getTime()) ? String(value) : date.toLocaleString();
}

function heading(key, hintKey) {
    return `<h3 class="font-bold text-white text-base mb-1">${escapeHtml(t(key))}</h3>
        ${hintKey ? `<p class="text-xs text-white/40 mb-4">${escapeHtml(t(hintKey))}</p>` : ''}`;
}

// Fills a tab panel and routes its data-sa clicks to `actions`
function fill(id, markup, actions = {}) {
    const panel = $(id);
    if (!panel) return null;
    panel.innerHTML = markup;
    panel.onclick = e => {
        const el = e.target.closest?.('[data-sa]');
        if (el && actions[el.dataset.sa]) actions[el.dataset.sa](el);
    };
    return panel;
}

function failed(id, err) {
    fill(id, `<p class="text-red-400 text-xs">${escapeHtml(t('server_admin.load_failed', { message: err.message }))}</p>`);
}

// ── Networking ──

// The network fields: [name, kind, how Rustyfin applies it]. 'restart': read at the next start
// (unless its environment variable is set); 'stored': saved for Jellyfin clients, not applied.
const NETWORK_FIELDS = [
    ['LocalNetworkAddresses', 'list', 'restart'],
    ['InternalHttpPort', 'port', 'restart'],
    ['KnownProxies', 'list', 'restart'],
    ['PublishedServerUriBySubnet', 'list', 'restart'],
    ['BaseUrl', 'text', 'stored'],
    ['EnableRemoteAccess', 'bool', 'stored'],
    ['LocalNetworkSubnets', 'list', 'stored'],
    ['RemoteIPFilter', 'list', 'stored'],
    ['IsRemoteIPFilterBlacklist', 'bool', 'stored'],
    ['PublicHttpPort', 'port', 'stored'],
    ['EnableHttps', 'bool', 'stored'],
    ['RequireHttps', 'bool', 'stored'],
    ['InternalHttpsPort', 'port', 'stored'],
    ['PublicHttpsPort', 'port', 'stored'],
    ['CertificatePath', 'text', 'stored'],
    ['EnableUPnP', 'bool', 'stored'],
    ['AutoDiscovery', 'bool', 'stored'],
    ['EnableIPv4', 'bool', 'stored'],
    ['EnableIPv6', 'bool', 'stored'],
];

let networkLoaded = null;

function applyBadge(apply) {
    return apply === 'restart'
        ? `<span class="ml-2 px-1.5 py-0.5 rounded text-[10px] font-semibold bg-amber-500/20 text-amber-400">${escapeHtml(t('server_admin.needs_restart'))}</span>`
        : `<span class="ml-2 px-1.5 py-0.5 rounded text-[10px] font-semibold bg-white/10 text-white/50">${escapeHtml(t('server_admin.stored_only'))}</span>`;
}

function networkFieldHtml([name, kind, apply], cfg) {
    const id = `net-${name}`;
    const label = `${escapeHtml(t(`server_admin.net.${name}`))}${applyBadge(apply)}`;
    const help = `<p class="text-xs text-white/40 mt-1">${escapeHtml(t(`server_admin.net.${name}_help`))}</p>`;
    const value = cfg[name];
    if (kind === 'bool') {
        return `<div><div class="flex items-center gap-3">
            <input type="checkbox" id="${id}" name="${name}" class="${CHECKBOX}" ${value ? 'checked' : ''}>
            <label for="${id}" class="text-sm text-white/70">${label}</label></div>${help}</div>`;
    }
    if (kind === 'list') {
        return `<div><label for="${id}" class="${LABEL}">${label}</label>
            <textarea id="${id}" name="${name}" rows="2" class="jf-input w-full font-mono text-xs">${escapeHtml((value || []).join('\n'))}</textarea>${help}</div>`;
    }
    const type = kind === 'port' ? 'number" min="1" max="65535' : 'text';
    return `<div><label for="${id}" class="${LABEL}">${label}</label>
        <input type="${type}" id="${id}" name="${name}" class="jf-input w-full" value="${escapeHtml(value ?? '')}">${help}</div>`;
}

function networkHtml(cfg) {
    return `${heading('server_admin.network', 'server_admin.network_hint')}
        <form id="network-form" class="space-y-4">
            ${NETWORK_FIELDS.map(f => networkFieldHtml(f, cfg)).join('')}
            <div class="flex justify-end pt-3 border-t border-white/10">
                <button type="submit" class="jf-btn-primary text-xs">${escapeHtml(t('server_admin.save'))}</button>
            </div>
        </form>`;
}

// The form's values over the configuration as loaded, so fields the page doesn't show are kept
function readNetwork(form, cfg) {
    const out = { ...cfg };
    for (const [name, kind] of NETWORK_FIELDS) {
        const el = form.elements[name];
        if (!el) continue;
        if (kind === 'bool') out[name] = !!el.checked;
        else if (kind === 'list') out[name] = el.value.split(/[\n,]/).map(s => s.trim()).filter(Boolean);
        else if (kind === 'port') out[name] = parseInt(el.value, 10);
        else out[name] = el.value.trim();
    }
    return out;
}

async function loadNetwork() {
    try {
        networkLoaded = await API.get('/System/Configuration/network', { cache: 'no-store' });
    } catch (err) {
        failed('admin-network', err);
        return;
    }
    const panel = fill('admin-network', networkHtml(networkLoaded));
    panel?.querySelector('#network-form')?.addEventListener('submit', saveNetwork);
}

async function saveNetwork(e) {
    e.preventDefault();
    const body = readNetwork(e.target, networkLoaded || {});
    try {
        await API.post('/System/Configuration/network', body);
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        return;
    }
    networkLoaded = body;
    toast(t('server_admin.network_saved'));
}

// ── API keys ──

function apiKeysHtml(keys) {
    const rows = keys.map(k => `
        <tr>
            <td class="py-2 text-white">${escapeHtml(k.AppName || '—')}</td>
            <td class="py-2 font-mono text-white/60">${escapeHtml(k.AccessToken || '')}</td>
            <td class="py-2 text-white/60">${escapeHtml(formatDate(k.DateCreated))}</td>
            <td class="py-2 text-white/60">${escapeHtml(formatDate(k.DateLastActivity))}</td>
            <td class="py-2 text-right"><button class="${BTN} text-red-400" data-sa="revoke-key" data-id="${escapeHtml(k.Id)}" data-name="${escapeHtml(k.AppName || '')}">${escapeHtml(t('server_admin.revoke'))}</button></td>
        </tr>`).join('');
    return `${heading('server_admin.api_keys', 'server_admin.api_keys_hint')}
        <form id="api-key-form" class="flex gap-2 mb-4">
            <input name="app" class="jf-input flex-1" maxlength="256" required placeholder="${escapeHtml(t('server_admin.app_name'))}" aria-label="${escapeHtml(t('server_admin.app_name'))}">
            <button type="submit" class="jf-btn-primary text-xs">${escapeHtml(t('server_admin.new_key'))}</button>
        </form>
        <div id="api-key-new"></div>
        ${keys.length ? `<div class="overflow-x-auto"><table class="w-full text-left text-xs">
            <thead><tr class="text-white/40 border-b border-white/10">
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.app_name'))}</th>
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.key'))}</th>
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.created'))}</th>
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.last_used'))}</th>
                <th></th></tr></thead>
            <tbody class="divide-y divide-white/5">${rows}</tbody></table></div>`
        : `<p class="text-xs text-white/40">${escapeHtml(t('server_admin.no_keys'))}</p>`}`;
}

// The new key, shown this once with a copy button; it is never stored or logged here
function showNewKey(key) {
    const box = $('api-key-new');
    if (!box) return;
    box.innerHTML = `<div class="mb-4 p-3 rounded-xl border border-emerald-500/30 bg-emerald-500/10">
        <p class="text-xs text-emerald-300 mb-2">${escapeHtml(t('server_admin.key_once'))}</p>
        <div class="flex gap-2 items-center">
            <code id="api-key-value" class="flex-1 font-mono text-sm text-white break-all select-all"></code>
            <button class="${BTN}" data-sa="copy-key">${escapeHtml(t('server_admin.copy'))}</button>
        </div></div>`;
    $('api-key-value').textContent = key;
}

async function loadApiKeys() {
    let keys;
    try {
        keys = (await API.get('/Auth/Keys', { cache: 'no-store' })).Items || [];
    } catch (err) {
        failed('admin-apikeys', err);
        return;
    }
    const panel = fill('admin-apikeys', apiKeysHtml(keys), {
        'revoke-key': revokeApiKey,
        'copy-key': () => navigator.clipboard?.writeText($('api-key-value')?.textContent || '')
            .then(() => toast(t('server_admin.copied')), () => {}),
    });
    panel?.querySelector('#api-key-form')?.addEventListener('submit', createApiKey);
}

async function createApiKey(e) {
    e.preventDefault();
    const app = e.target.elements.app.value.trim();
    if (!app) return;
    let created;
    try {
        created = await API.post(`/Auth/Keys?app=${encodeURIComponent(app)}`);
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        return;
    }
    await loadApiKeys();
    if (created?.AccessToken) showNewKey(created.AccessToken);
}

async function revokeApiKey(el) {
    if (!(await confirmDialog(t('server_admin.revoke_title'), t('server_admin.revoke_text', { name: el.dataset.name }), t('server_admin.revoke')))) return;
    try {
        await API.del(`/Auth/Keys/${encodeURIComponent(el.dataset.id)}`);
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        return;
    }
    loadApiKeys();
}

// ── Devices ──

// This browser's device id, as api.js sends it
function ownDeviceId() {
    return `web-${localStorage.getItem('rustyfin_device_id') || ''}`;
}

function devicesHtml(devices) {
    const own = ownDeviceId();
    const rows = devices.map(d => {
        const mine = d.Id === own;
        return `<tr>
            <td class="py-2 text-white">${escapeHtml(d.CustomName || d.Name || d.Id)}
                ${d.CustomName && d.CustomName !== d.Name ? `<span class="block text-[11px] text-white/40">${escapeHtml(d.Name || '')}</span>` : ''}
                ${mine ? `<span class="ml-1 px-1.5 py-0.5 rounded text-[10px] bg-[#00a4dc]/20 text-[#00a4dc]">${escapeHtml(t('server_admin.this_device'))}</span>` : ''}</td>
            <td class="py-2 text-white/60">${escapeHtml([d.AppName, d.AppVersion].filter(Boolean).join(' '))}</td>
            <td class="py-2 text-white/60">${escapeHtml(d.LastUserName || '—')}</td>
            <td class="py-2 text-white/60">${escapeHtml(formatDate(d.DateLastActivated))}</td>
            <td class="py-2 text-right whitespace-nowrap">
                <button class="${BTN}" data-sa="rename-device" data-id="${escapeHtml(d.Id)}" data-name="${escapeHtml(d.CustomName || '')}">${escapeHtml(t('server_admin.rename'))}</button>
                ${mine ? '' : `<button class="${BTN} text-red-400" data-sa="delete-device" data-id="${escapeHtml(d.Id)}" data-name="${escapeHtml(d.CustomName || d.Name || d.Id)}">${escapeHtml(t('common.delete'))}</button>`}
            </td></tr>`;
    }).join('');
    return `${heading('server_admin.devices', 'server_admin.devices_hint')}
        ${devices.length ? `<div class="overflow-x-auto"><table class="w-full text-left text-xs">
            <thead><tr class="text-white/40 border-b border-white/10">
                <th class="pb-2 font-semibold">${escapeHtml(t('common.name'))}</th>
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.app'))}</th>
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.last_user'))}</th>
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.last_active'))}</th>
                <th></th></tr></thead>
            <tbody class="divide-y divide-white/5">${rows}</tbody></table></div>`
        : `<p class="text-xs text-white/40">${escapeHtml(t('server_admin.no_devices'))}</p>`}`;
}

async function loadDevices() {
    let devices;
    try {
        devices = (await API.get('/Devices', { cache: 'no-store' })).Items || [];
    } catch (err) {
        failed('admin-devices', err);
        return;
    }
    fill('admin-devices', devicesHtml(devices), { 'rename-device': renameDevice, 'delete-device': deleteDevice });
}

async function renameDevice(el) {
    const name = prompt(t('server_admin.rename_prompt'), el.dataset.name || '');
    if (name === null) return;
    try {
        await API.post(`/Devices/Options?id=${encodeURIComponent(el.dataset.id)}`, { CustomName: name.trim() });
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        return;
    }
    loadDevices();
}

async function deleteDevice(el) {
    if (!(await confirmDialog(t('server_admin.delete_device_title'), t('server_admin.delete_device_text', { name: el.dataset.name }), t('common.delete')))) return;
    try {
        await API.del(`/Devices?id=${encodeURIComponent(el.dataset.id)}`);
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        return;
    }
    loadDevices();
}

// ── Branding ──

let brandingLoaded = null;

function brandingHtml(cfg) {
    return `${heading('server_admin.branding', 'server_admin.branding_hint')}
        <form id="branding-form" class="space-y-4">
            <div><label for="br-disclaimer" class="${LABEL}">${escapeHtml(t('server_admin.login_disclaimer'))}</label>
                <textarea id="br-disclaimer" name="LoginDisclaimer" rows="3" class="jf-input w-full">${escapeHtml(cfg.LoginDisclaimer || '')}</textarea>
                <p class="text-xs text-white/40 mt-1">${escapeHtml(t('server_admin.login_disclaimer_help'))}</p></div>
            <div><label for="br-css" class="${LABEL}">${escapeHtml(t('server_admin.custom_css'))}</label>
                <textarea id="br-css" name="CustomCss" rows="8" class="jf-input w-full font-mono text-xs" spellcheck="false">${escapeHtml(cfg.CustomCss || '')}</textarea>
                <p class="text-xs text-white/40 mt-1">${escapeHtml(t('server_admin.custom_css_help'))}</p></div>
            <div><div class="flex items-center gap-3">
                <input type="checkbox" id="br-splash" name="SplashscreenEnabled" class="${CHECKBOX}" ${cfg.SplashscreenEnabled ? 'checked' : ''}>
                <label for="br-splash" class="text-sm text-white/70">${escapeHtml(t('server_admin.splashscreen'))}${applyBadge('stored')}</label></div>
                <p class="text-xs text-white/40 mt-1">${escapeHtml(t('server_admin.splashscreen_help'))}</p></div>
            <div class="flex justify-end pt-3 border-t border-white/10">
                <button type="submit" class="jf-btn-primary text-xs">${escapeHtml(t('server_admin.save'))}</button>
            </div>
        </form>`;
}

function readBranding(form, cfg) {
    return {
        ...cfg,
        LoginDisclaimer: form.elements.LoginDisclaimer.value,
        CustomCss: form.elements.CustomCss.value,
        SplashscreenEnabled: !!form.elements.SplashscreenEnabled.checked,
    };
}

async function loadBranding() {
    try {
        brandingLoaded = await API.get('/System/Configuration/branding', { cache: 'no-store' });
    } catch (err) {
        failed('admin-branding', err);
        return;
    }
    const panel = fill('admin-branding', brandingHtml(brandingLoaded));
    panel?.querySelector('#branding-form')?.addEventListener('submit', saveBranding);
}

async function saveBranding(e) {
    e.preventDefault();
    const body = readBranding(e.target, brandingLoaded || {});
    try {
        await API.post('/System/Configuration/branding', body);
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        return;
    }
    brandingLoaded = body;
    applyBrandingCss();
    toast(t('server_admin.branding_saved'));
}

// ── Backups ──

function backupsHtml(backups) {
    const rows = backups.map(b => `<tr>
        <td class="py-2 text-white">${escapeHtml(formatDate(b.DateCreated))}</td>
        <td class="py-2 font-mono text-white/60">${escapeHtml(b.Path)}</td>
        <td class="py-2 text-white/60">${escapeHtml(b.ServerVersion || '')}</td>
        <td class="py-2 text-right whitespace-nowrap">
            <button class="${BTN}" data-sa="download-backup" data-path="${escapeHtml(b.Path)}">${escapeHtml(t('server_admin.download'))}</button>
            <button class="${BTN} text-amber-400" data-sa="restore-backup" data-path="${escapeHtml(b.Path)}">${escapeHtml(t('server_admin.restore'))}</button>
            <button class="${BTN} text-red-400" data-sa="delete-backup" data-path="${escapeHtml(b.Path)}">${escapeHtml(t('common.delete'))}</button>
        </td></tr>`).join('');
    return `${heading('server_admin.backups', 'server_admin.backups_hint')}
        <div class="mb-4"><button class="jf-btn-primary text-xs" data-sa="create-backup">${escapeHtml(t('server_admin.create_backup'))}</button></div>
        ${backups.length ? `<div class="overflow-x-auto"><table class="w-full text-left text-xs">
            <thead><tr class="text-white/40 border-b border-white/10">
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.created'))}</th>
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.file'))}</th>
                <th class="pb-2 font-semibold">${escapeHtml(t('server_admin.version'))}</th>
                <th></th></tr></thead>
            <tbody class="divide-y divide-white/5">${rows}</tbody></table></div>`
        : `<p class="text-xs text-white/40">${escapeHtml(t('server_admin.no_backups'))}</p>`}`;
}

async function loadBackups() {
    let backups;
    try {
        backups = await API.get('/Backup', { cache: 'no-store' });
    } catch (err) {
        failed('admin-backups', err);
        return;
    }
    fill('admin-backups', backupsHtml(backups || []), {
        'create-backup': createBackup,
        'download-backup': downloadBackup,
        'restore-backup': restoreBackup,
        'delete-backup': deleteBackup,
    });
}

async function createBackup(el) {
    el.disabled = true;
    try {
        await API.post('/Backup/Create', { Metadata: false, Trickplay: false, Subtitles: false, Database: true });
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        el.disabled = false;
        return;
    }
    toast(t('server_admin.backup_created'));
    loadBackups();
}

// Fetches the file with the session's credentials, then saves it under its own name
async function downloadBackup(el) {
    const path = el.dataset.path;
    try {
        const res = await fetch(`${API.base}/Backup/Download?path=${encodeURIComponent(path)}`, { headers: API.headers() });
        if (!res.ok) throw new Error(`${res.status}`);
        const url = URL.createObjectURL(await res.blob());
        const a = document.createElement('a');
        a.href = url;
        a.download = path;
        document.body.appendChild(a);
        a.click();
        a.remove();
        setTimeout(() => URL.revokeObjectURL(url), 1000);
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
    }
}

async function restoreBackup(el) {
    const path = el.dataset.path;
    if (!(await confirmDialog(t('server_admin.restore_title'), t('server_admin.restore_text', { name: path }), t('server_admin.restore')))) return;
    try {
        await API.post('/Backup/Restore', { ArchiveFileName: path });
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        return;
    }
    toast(t('server_admin.restore_staged'));
    serverPower('restart');
}

async function deleteBackup(el) {
    const path = el.dataset.path;
    if (!(await confirmDialog(t('server_admin.delete_backup_title'), t('server_admin.delete_backup_text', { name: path }), t('common.delete')))) return;
    try {
        await API.del(`/Backup/${encodeURIComponent(path)}`);
    } catch (err) {
        toast(t('server_admin.save_failed', { message: err.message }), 'error');
        return;
    }
    loadBackups();
}

// The loader of each tab, by panel id
const SERVER_ADMIN_LOADERS = {
    'admin-network': loadNetwork,
    'admin-apikeys': loadApiKeys,
    'admin-devices': loadDevices,
    'admin-branding': loadBranding,
    'admin-backups': loadBackups,
};

export {
    SERVER_ADMIN_TABS, SERVER_ADMIN_LOADERS, serverAdminPanelsHtml, readNetwork, readBranding, networkHtml,
    apiKeysHtml, devicesHtml, backupsHtml, loadNetwork, saveNetwork, loadApiKeys, createApiKey, revokeApiKey,
    loadDevices, renameDevice, deleteDevice, loadBranding, saveBranding, loadBackups, createBackup, restoreBackup,
    deleteBackup, NETWORK_FIELDS,
};
