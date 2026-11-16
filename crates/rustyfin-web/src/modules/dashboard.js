// The admin dashboard's server pieces, as in Jellyfin Web's dashboard: restart and shut down with a
// confirmation, the Overview's status and paths, the General tab (server name, languages, Quick
// Connect), the library editor (folders and name) and the scheduled task triggers editor.
import { $, show, hide, icon, toast, escapeHtml } from './util.js';
import { renderAdmin, refreshTasks } from './admin.js';

const LABEL = 'block text-xs font-semibold text-white/70 mb-1.5';
// .NET ticks: 10,000,000 a second
const TICKS_PER_MINUTE = 600000000;
const TICKS_PER_HOUR = 60 * TICKS_PER_MINUTE;
const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
// Jellyfin Web's interval choices, in minutes
const INTERVALS = [15, 30, 45, 60, 120, 180, 240, 360, 480, 720, 1440];

// A <dialog> made once and reused, closed by a click on its backdrop
function dashboardDialog(id) {
    let dlg = $(id);
    if (!dlg) {
        dlg = document.createElement('dialog');
        dlg.id = id;
        dlg.className = 'modal';
        dlg.addEventListener('click', e => { if (e.target === dlg) dlg.close(); });
        document.body.appendChild(dlg);
    }
    return dlg;
}

// Resolves true when the admin confirms, false when the dialog closes any other way
function confirmDialog(title, text, confirmLabel) {
    const dlg = dashboardDialog('dashboard-confirm');
    dlg.innerHTML = `
        <form method="dialog" class="modal-box bg-[#101827] border border-white/10 max-w-sm">
            <h3 class="font-bold text-white text-base mb-2">${escapeHtml(title)}</h3>
            <p class="text-white/70 text-sm">${escapeHtml(text)}</p>
            <div class="modal-action">
                <button value="cancel" class="btn btn-sm">${escapeHtml(t('common.cancel'))}</button>
                <button value="ok" class="btn btn-sm btn-error" data-confirm>${escapeHtml(confirmLabel)}</button>
            </div>
        </form>`;
    dlg.returnValue = '';
    return new Promise(resolve => {
        dlg.addEventListener('close', () => resolve(dlg.returnValue === 'ok'), { once: true });
        dlg.showModal();
    });
}

// Restart or shut down the server ('restart' or 'shutdown'), once the admin confirms
async function serverPower(kind) {
    const restart = kind === 'restart';
    const key = restart ? 'restart' : 'shutdown';
    if (!await confirmDialog(t(`dashboard.${key}_title`), t(`dashboard.${key}_text`), t(`dashboard.${key}`))) return;
    try {
        await (restart ? API.restart() : API.shutdown());
        toast(t(restart ? 'admin.restarting' : 'dashboard.shutting_down'));
    } catch (err) {
        toast(t('dashboard.power_failed', { message: err.message }), 'error');
    }
}

function detailRow(label, value) {
    return `<div class="flex justify-between gap-4 py-2 border-b border-white/5"><span class="text-white/50 whitespace-nowrap">${escapeHtml(label)}</span> <span class="font-mono text-white truncate" title="${escapeHtml(value)}">${escapeHtml(value || '—')}</span></div>`;
}

// The Overview's state line: a restart or an update waiting, else running
function serverStatusHtml(info) {
    const pill = (cls, text) => `<span class="px-2 py-0.5 rounded text-[11px] font-bold ${cls}" data-server-status>${escapeHtml(text)}</span>`;
    const pills = [];
    if (info.IsShuttingDown) pills.push(pill('bg-red-500/20 text-red-300', t('dashboard.shutting_down')));
    if (info.HasPendingRestart) pills.push(pill('bg-amber-500/20 text-amber-300', t('dashboard.restart_pending')));
    if (info.HasUpdateAvailable) pills.push(pill('bg-sky-500/20 text-sky-300', t('dashboard.update_available')));
    if (!pills.length) pills.push(pill('bg-emerald-500/20 text-emerald-300', t('dashboard.status_running')));
    return pills.join(' ');
}

// The Overview's server details: what runs where, and the folders it uses, as Jellyfin's dashboard lists them
function serverDetailsHtml(info, version) {
    return `
        <div class="flex items-center gap-2 mb-4">
            <h3 class="font-bold text-white text-base">${escapeHtml(t('admin.system_details'))}</h3>
            ${serverStatusHtml(info)}
        </div>
        <div class="grid grid-cols-1 md:grid-cols-2 gap-x-6 text-xs">
            ${detailRow(t('settings.server_name'), info.ServerName || 'Rustyfin')}
            ${detailRow(t('dashboard.version'), version)}
            ${detailRow(t('dashboard.api_version'), info.Version)}
            ${detailRow(t('admin.operating_system'), [info.OperatingSystem, info.SystemArchitecture].filter(Boolean).join(' · '))}
        </div>
        <h4 class="font-semibold text-white/80 text-sm mt-5 mb-2">${escapeHtml(t('admin.paths'))}</h4>
        <div class="grid grid-cols-1 md:grid-cols-2 gap-x-6 text-xs" data-server-paths>
            ${detailRow(t('dashboard.path_program_data'), info.ProgramDataPath)}
            ${detailRow(t('dashboard.path_cache'), info.CachePath)}
            ${detailRow(t('dashboard.path_metadata'), info.InternalMetadataPath)}
            ${detailRow(t('dashboard.path_transcode'), info.TranscodingTempPath)}
            ${detailRow(t('dashboard.path_logs'), info.LogPath)}
            ${detailRow(t('dashboard.path_web'), info.WebPath)}
        </div>`;
}

// --- General tab ---

// What the General tab loaded: the configuration it posts back whole, so the fields it doesn't show are kept
let generalConfig = null;

function generalPanelHtml() {
    return '<div id="admin-general" class="hidden jf-settings-card"></div>';
}

function cultureOptions(list, selected, code) {
    const seen = list.some(c => code(c) === selected);
    const own = selected && !seen ? `<option value="${escapeHtml(selected)}" selected>${escapeHtml(selected)}</option>` : '';
    return own + list.map(c => `<option value="${escapeHtml(code(c))}"${code(c) === selected ? ' selected' : ''}>${escapeHtml(c.DisplayName || c.Name || code(c))}</option>`).join('');
}

function generalSettingsHtml(cfg, info, cultures, countries, uiCultures) {
    const field = (id, label, input, hint) => `
        <div>
            <label for="${id}" class="${LABEL}">${escapeHtml(label)}</label>
            ${input}
            ${hint ? `<p class="text-xs text-white/40 mt-1">${escapeHtml(hint)}</p>` : ''}
        </div>`;
    const readOnly = (id, label, value) => field(id, label, `<input id="${id}" class="jf-input w-full font-mono" value="${escapeHtml(value || '')}" readonly>`);
    return `
        <form id="general-settings" class="space-y-5" onsubmit="saveGeneralSettings(event)" autocomplete="off">
            <h3 class="font-bold text-white text-base">${escapeHtml(t('dashboard.general'))}</h3>
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                ${field('gen-server-name', t('settings.server_name'), `<input id="gen-server-name" name="ServerName" class="jf-input w-full" value="${escapeHtml(cfg.ServerName || '')}" required>`, t('dashboard.server_name_help'))}
                ${field('gen-ui-culture', t('dashboard.display_language'), `<select id="gen-ui-culture" name="UICulture" class="jf-input w-full">${cultureOptions(uiCultures, cfg.UICulture, c => c.Value)}</select>`)}
                ${field('gen-meta-lang', t('dashboard.metadata_language'), `<select id="gen-meta-lang" name="PreferredMetadataLanguage" class="jf-input w-full">${cultureOptions(cultures, cfg.PreferredMetadataLanguage, c => c.TwoLetterISOLanguageName)}</select>`)}
                ${field('gen-meta-country', t('dashboard.metadata_country'), `<select id="gen-meta-country" name="MetadataCountryCode" class="jf-input w-full">${cultureOptions(countries, cfg.MetadataCountryCode, c => c.TwoLetterISORegionName)}</select>`)}
            </div>
            <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                ${readOnly('gen-cache-path', t('dashboard.cache_path'), info.CachePath)}
                ${readOnly('gen-metadata-path', t('dashboard.metadata_path'), info.InternalMetadataPath)}
            </div>
            <p class="text-xs text-white/40 -mt-3">${escapeHtml(t('dashboard.paths_read_only'))}</p>
            <label class="flex items-center gap-2 text-sm text-white/80">
                <input id="gen-quick-connect" name="QuickConnectAvailable" type="checkbox" class="w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]"${cfg.QuickConnectAvailable ? ' checked' : ''}>
                ${escapeHtml(t('dashboard.quick_connect'))}
            </label>
            <div id="general-settings-error" class="hidden text-red-400 text-xs p-2 rounded bg-red-500/10 border border-red-500/20"></div>
            <button type="submit" class="jf-btn-primary text-xs">${escapeHtml(t('settings.save_config'))}</button>
        </form>`;
}

async function loadGeneralSettings() {
    const box = $('admin-general');
    if (!box) return;
    try {
        const [cfg, info, cultures, countries, uiCultures] = await Promise.all([
            API.getConfig(),
            API.getFullInfo(),
            API.get('/Localization/Cultures').catch(() => []),
            API.get('/Localization/Countries').catch(() => []),
            API.get('/Localization/Options').catch(() => []),
        ]);
        generalConfig = cfg;
        const list = value => (Array.isArray(value) ? value : []);
        // Jellyfin's /Localization/Options is a list of { Name, Value }; a server without it offers the cultures
        const display = Array.isArray(uiCultures) ? uiCultures
            : list(cultures).map(c => ({ Name: c.DisplayName, Value: c.TwoLetterISOLanguageName }));
        box.innerHTML = generalSettingsHtml(cfg, info, list(cultures), list(countries), display);
    } catch (err) {
        box.innerHTML = `<p class="text-xs text-red-400">${escapeHtml(t('dashboard.load_failed', { message: err.message }))}</p>`;
    }
}

// The loaded configuration with the form's values: every other field goes back as it was
function readGeneralSettings(form, cfg) {
    const el = name => form.elements[name];
    return {
        ...cfg,
        ServerName: el('ServerName').value.trim() || cfg.ServerName,
        UICulture: el('UICulture').value,
        PreferredMetadataLanguage: el('PreferredMetadataLanguage').value,
        MetadataCountryCode: el('MetadataCountryCode').value,
        QuickConnectAvailable: el('QuickConnectAvailable').checked,
    };
}

async function saveGeneralSettings(e) {
    e?.preventDefault();
    const form = $('general-settings');
    if (!form || !generalConfig) return;
    hide('general-settings-error');
    // Saved over the configuration as it is now, so a change made in another tab meanwhile is kept
    let current = generalConfig;
    try {
        current = await API.getConfig();
    } catch { /* the loaded one */ }
    const values = readGeneralSettings(form, current);
    try {
        await API.updateConfig(values);
    } catch (err) {
        $('general-settings-error').textContent = t('dashboard.save_failed', { message: err.message });
        show('general-settings-error');
        return;
    }
    generalConfig = values;
    toast(t('dashboard.saved'));
}

// --- Library editor: folders and name ---

let editedLibrary = null;

function libraryEditorHtml(lib) {
    const paths = lib.Locations || lib.Paths || lib.paths || [];
    return `
        <div class="modal-box bg-[#101827] border border-white/10 max-w-lg">
            <h3 class="font-bold text-white text-base mb-4">${escapeHtml(t('dashboard.edit_library_title', { name: lib.Name || lib.name }))}</h3>
            <form class="flex gap-2 items-end mb-5" onsubmit="renameLibrary(event)">
                <div class="flex-1">
                    <label for="lib-edit-name" class="${LABEL}">${escapeHtml(t('common.name'))}</label>
                    <input id="lib-edit-name" class="jf-input w-full" value="${escapeHtml(lib.Name || lib.name)}" required>
                </div>
                <button type="submit" class="jf-btn-ghost text-xs !py-2">${escapeHtml(t('dashboard.rename'))}</button>
            </form>
            <p class="${LABEL}">${escapeHtml(t('dashboard.folders'))}</p>
            <ul class="space-y-1 mb-3" id="lib-edit-paths">
                ${paths.map(p => `
                <li class="flex items-center gap-2 rounded-lg bg-white/5 px-3 py-2">
                    <span class="font-mono text-xs text-white truncate flex-1" title="${escapeHtml(p)}">${escapeHtml(p)}</span>
                    <button type="button" class="text-red-400 hover:text-red-300 text-xs" data-action="library-remove-path" data-path="${escapeHtml(p)}"${paths.length < 2 ? ` disabled title="${escapeHtml(t('dashboard.last_folder'))}"` : ''}>${escapeHtml(t('dashboard.remove'))}</button>
                </li>`).join('')}
            </ul>
            <form class="path-field flex gap-2" onsubmit="addLibraryFolder(event)">
                <input id="lib-edit-new-path" data-path-field class="jf-input flex-1 font-mono text-xs" placeholder="/media/…" required>
                <button type="button" class="jf-btn-ghost !px-3 !py-2 text-xs" data-action="browse-path">${icon('FolderOpen', { size: 14 })}</button>
                <button type="submit" class="jf-btn-primary text-xs !py-2">${escapeHtml(t('dashboard.add_folder'))}</button>
            </form>
            <p class="text-xs text-white/40 mt-2">${escapeHtml(t('dashboard.folders_scan_note'))}</p>
            <div class="modal-action"><button type="button" class="btn btn-sm" onclick="this.closest('dialog').close()">${escapeHtml(t('common.close'))}</button></div>
        </div>`;
}

async function openLibraryEditor(id) {
    let libs;
    try {
        libs = await API.get('/Library/VirtualFolders');
    } catch (err) {
        toast(err.message, 'error');
        return;
    }
    const key = v => String(v || '').toLowerCase().replace(/-/g, '');
    const lib = libs.find(l => key(l.ItemId || l.Id || l.id) === key(id));
    if (!lib) return;
    editedLibrary = lib;
    const dlg = dashboardDialog('library-editor');
    dlg.innerHTML = libraryEditorHtml(lib);
    if (!dlg.open) {
        dlg.addEventListener('close', () => { editedLibrary = null; renderAdmin('libraries'); }, { once: true });
        dlg.showModal();
    }
}

// Runs one change of the edited library, then shows the library as the server has it now
async function changeLibrary(request, doneKey) {
    const lib = editedLibrary;
    if (!lib) return;
    try {
        await request(lib.Name || lib.name);
    } catch (err) {
        toast(t('dashboard.library_change_failed', { message: err.message }), 'error');
        return;
    }
    toast(t(doneKey));
    await openLibraryEditor(lib.ItemId || lib.Id || lib.id);
}

function addLibraryFolder(e) {
    e.preventDefault();
    const path = $('lib-edit-new-path')?.value.trim();
    if (!path) return;
    return changeLibrary(name => API.post('/Library/VirtualFolders/Paths?refreshLibrary=true', { Name: name, PathInfo: { Path: path } }), 'dashboard.folder_added');
}

// Asks first: the folder's items, with their watched state, leave the library; the files stay
async function removeLibraryFolder(path) {
    if (!await confirmDialog(t('dashboard.remove_folder_title'), t('dashboard.remove_folder_text', { path }), t('dashboard.remove'))) return;
    const q = name => new URLSearchParams({ name, path, refreshLibrary: 'true' });
    return changeLibrary(name => API.del(`/Library/VirtualFolders/Paths?${q(name)}`), 'dashboard.folder_removed');
}

function renameLibrary(e) {
    e.preventDefault();
    const newName = $('lib-edit-name')?.value.trim();
    if (!newName || newName === (editedLibrary?.Name || editedLibrary?.name)) return;
    return changeLibrary(name => API.post(`/Library/VirtualFolders/Name?${new URLSearchParams({ name, newName })}`), 'dashboard.renamed');
}

// --- Scheduled task triggers ---

function timeOfDay(ticks) {
    const hour = Math.floor(ticks / TICKS_PER_HOUR) % 24;
    const minute = Math.floor(ticks / TICKS_PER_MINUTE) % 60;
    return `${String(hour).padStart(2, '0')}:${String(minute).padStart(2, '0')}`;
}

function intervalLabel(ticks) {
    const minutes = Math.round(ticks / TICKS_PER_MINUTE);
    return minutes < 60 ? t('dashboard.every_minutes', { count: minutes }) : t('admin.every_hours', { count: Math.round(minutes / 60) });
}

function dayName(day) {
    const index = DAYS.indexOf(day);
    // 2023-01-01 was a Sunday
    return index < 0 ? String(day ?? '') : new Date(2023, 0, 1 + index).toLocaleDateString(globalThis.document?.documentElement.lang || undefined, { weekday: 'long' });
}

// One trigger in words, as Jellyfin Web's task page shows it, as HTML: the server's values in it are escaped
function triggerSummary(tr) {
    switch (tr.Type) {
        case 'IntervalTrigger': return tr.IntervalTicks ? intervalLabel(tr.IntervalTicks) : escapeHtml(tr.Type);
        case 'DailyTrigger': return t('dashboard.daily_at', { time: timeOfDay(tr.TimeOfDayTicks || 0) });
        case 'WeeklyTrigger': return t('dashboard.weekly_at', { day: escapeHtml(dayName(tr.DayOfWeek)), time: timeOfDay(tr.TimeOfDayTicks || 0) });
        case 'StartupTrigger': return t('dashboard.on_startup');
        default: return escapeHtml(tr.Type || '');
    }
}

let editedTask = null;

function triggerEditorHtml(task) {
    const triggers = task.Triggers || [];
    return `
        <div class="modal-box bg-[#101827] border border-white/10 max-w-md">
            <h3 class="font-bold text-white text-base mb-4">${escapeHtml(t('dashboard.triggers_title', { name: task.Name }))}</h3>
            <ul class="space-y-1 mb-4" id="task-triggers">
                ${triggers.length ? triggers.map((tr, i) => `
                <li class="flex items-center gap-2 rounded-lg bg-white/5 px-3 py-2 text-sm text-white">
                    ${icon('Clock', { size: 14 })}<span class="flex-1">${triggerSummary(tr)}</span>
                    <button type="button" class="text-red-400 hover:text-red-300 text-xs" data-action="task-remove-trigger" data-index="${i}">${escapeHtml(t('dashboard.remove'))}</button>
                </li>`).join('') : `<li class="text-white/40 text-xs">${escapeHtml(t('dashboard.no_triggers'))}</li>`}
            </ul>
            <form id="trigger-form" class="grid grid-cols-2 gap-3" onsubmit="addTaskTrigger(event)">
                <div class="col-span-2">
                    <label for="trigger-type" class="${LABEL}">${escapeHtml(t('dashboard.trigger_type'))}</label>
                    <select id="trigger-type" name="Type" class="jf-input w-full" onchange="showTriggerFields(this.value)">
                        <option value="DailyTrigger">${escapeHtml(t('dashboard.trigger_daily'))}</option>
                        <option value="WeeklyTrigger">${escapeHtml(t('dashboard.trigger_weekly'))}</option>
                        <option value="IntervalTrigger">${escapeHtml(t('dashboard.trigger_interval'))}</option>
                        <option value="StartupTrigger">${escapeHtml(t('dashboard.trigger_startup'))}</option>
                    </select>
                </div>
                <div data-trigger-field="WeeklyTrigger" class="hidden">
                    <label for="trigger-day" class="${LABEL}">${escapeHtml(t('dashboard.day'))}</label>
                    <select id="trigger-day" name="DayOfWeek" class="jf-input w-full">${DAYS.map(d => `<option value="${d}">${escapeHtml(dayName(d))}</option>`).join('')}</select>
                </div>
                <div data-trigger-field="DailyTrigger WeeklyTrigger">
                    <label for="trigger-time" class="${LABEL}">${escapeHtml(t('dashboard.time'))}</label>
                    <input id="trigger-time" name="TimeOfDay" type="time" class="jf-input w-full" value="03:00">
                </div>
                <div data-trigger-field="IntervalTrigger" class="hidden">
                    <label for="trigger-interval" class="${LABEL}">${escapeHtml(t('dashboard.every'))}</label>
                    <select id="trigger-interval" name="Interval" class="jf-input w-full">${INTERVALS.map(m => `<option value="${m}"${m === 1440 ? ' selected' : ''}>${escapeHtml(intervalLabel(m * TICKS_PER_MINUTE))}</option>`).join('')}</select>
                </div>
                <div class="col-span-2"><button type="submit" class="jf-btn-primary text-xs">${escapeHtml(t('dashboard.add_trigger'))}</button></div>
            </form>
            <div class="modal-action"><button type="button" class="btn btn-sm" onclick="this.closest('dialog').close()">${escapeHtml(t('common.close'))}</button></div>
        </div>`;
}

function showTriggerFields(type) {
    document.querySelectorAll('#trigger-form [data-trigger-field]').forEach(el => {
        el.classList.toggle('hidden', !el.dataset.triggerField.split(' ').includes(type));
    });
}

// The trigger the form describes, in Jellyfin's TaskTriggerInfo shape
function readTrigger(form) {
    const el = name => form.elements[name];
    const type = el('Type').value;
    const [h, m] = (el('TimeOfDay').value || '00:00').split(':').map(Number);
    const trigger = { Type: type };
    if (type === 'DailyTrigger' || type === 'WeeklyTrigger') trigger.TimeOfDayTicks = (h * 60 + m) * TICKS_PER_MINUTE;
    if (type === 'WeeklyTrigger') trigger.DayOfWeek = el('DayOfWeek').value;
    if (type === 'IntervalTrigger') trigger.IntervalTicks = Number(el('Interval').value) * TICKS_PER_MINUTE;
    return trigger;
}

async function openTriggerEditor(id) {
    let task;
    try {
        task = await API.get(`/ScheduledTasks/${encodeURIComponent(id)}`);
    } catch (err) {
        toast(err.message, 'error');
        return;
    }
    editedTask = task;
    const dlg = dashboardDialog('trigger-editor');
    dlg.innerHTML = triggerEditorHtml(task);
    if (!dlg.open) {
        dlg.addEventListener('close', () => { editedTask = null; refreshTasks(true); }, { once: true });
        dlg.showModal();
    }
}

// Posts the task's whole trigger list, as Jellyfin Web does, and shows what the server kept
async function saveTriggers(triggers) {
    const task = editedTask;
    if (!task) return;
    try {
        await API.post(`/ScheduledTasks/${encodeURIComponent(task.Id)}/Triggers`, triggers);
    } catch (err) {
        toast(t('dashboard.triggers_failed', { message: err.message }), 'error');
        return;
    }
    await openTriggerEditor(task.Id);
}

function addTaskTrigger(e) {
    e.preventDefault();
    if (!editedTask) return;
    return saveTriggers([...(editedTask.Triggers || []), readTrigger(e.target)]);
}

function removeTaskTrigger(index) {
    if (!editedTask) return;
    return saveTriggers((editedTask.Triggers || []).filter((_, i) => i !== index));
}

export {
    serverPower, serverDetailsHtml, serverStatusHtml, generalPanelHtml, loadGeneralSettings, saveGeneralSettings,
    readGeneralSettings, openLibraryEditor, addLibraryFolder, removeLibraryFolder, renameLibrary, triggerSummary,
    openTriggerEditor, addTaskTrigger, removeTaskTrigger, showTriggerFields, readTrigger, confirmDialog,
};
