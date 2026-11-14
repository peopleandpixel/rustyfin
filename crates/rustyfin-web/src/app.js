/* Rustyfin Web Client - Jellyfin 12.x Architecture */
let currentPage = 'home';
let currentLibraryId = null;
let currentDetailId = null;
let libraries = [];
let wizStep = 0;
let wizProgress = { configSaved: false, userCreated: false, loggedIn: false, queued: false };
let wizCredentials = null;
let wizFinishing = false;
let wizSkipImport = false; // set by "Finish without importing" for one run of wizComplete
let wizLibraries = []; // { id, name, type, paths } of the libraries this page created
let wizListening = false;
let wizPreview = null; // { path, data } of the last Jellyfin preview that succeeded
let wizNameDefault = 'admin'; // the account name the wizard last filled in (or the markup's default)
let wizOthersFor = null; // the Jellyfin user the other-users list was built for
let wizBusy = false; // true while wizNext runs, so a second Next or Enter is ignored
// The wizard's step numbers (DOM order, ids wiz-step-N)
const WIZ = { choice: 1, folder: 2, user: 3, libs: 4, others: 5, account: 6, library: 7, finish: 8 };
let serverVersion = '0.3.0';
let playerReportInterval = null;
let playerHideTimeout = null;
let activeItem = null;
// Library id -> { percent, queued } while it scans (percent 0 = not known yet, so its ring spins; queued = waiting its turn). Absent when not scanning.
let scanStatuses = {};

// --- DOM & Helper Utilities ---
function $(id) { return document.getElementById(id); }
function html(id, content) { const el = $(id); if (el) el.innerHTML = content; }
function show(id) {
    const el = $(id);
    if (!el) return;
    el.classList.remove('hidden');
    el.style.removeProperty('display');
}
function hide(id) {
    const el = $(id);
    if (!el) return;
    el.classList.add('hidden');
    el.style.setProperty('display', 'none', 'important');
}

function initLazyImages(root = document) {
    // Hover-based full-res image loading: swap thumbnail for full-res on card hover
    root.querySelectorAll('img.jf-thumb-img[data-full-src]').forEach(img => {
        if (img.dataset.lazyBound) return;
        const card = img.closest('.jf-card');
        if (!card) return;
        img.dataset.lazyBound = '1';

        card.addEventListener('mouseenter', () => {
            if (img.dataset.fullLoaded) return;
            img.dataset.fullLoaded = '1';
            const fullSrc = img.dataset.fullSrc;
            const fullImg = new Image();
            fullImg.decode().then(() => {
                img.src = fullSrc;
                img.style.opacity = '1';
                img.classList.remove('blur-sm', 'scale-105');
            }).catch(() => {
                // Full-res failed; keep thumbnail
                delete img.dataset.fullLoaded;
            });
            fullImg.src = fullSrc;
        }, { once: true });
    });
}

function toast(msg, type = 'success') {
    const c = $('toast-container');
    if (!c) return;
    const div = document.createElement('div');
    const isError = type === 'error';
    div.className = 'jf-toast pointer-events-auto flex items-center gap-3 px-4 py-3 rounded-xl shadow-2xl text-xs font-semibold backdrop-blur-xl border ' +
        (isError ? 'bg-red-500/90 text-white border-red-400/40' : 'bg-[#0f172a]/95 text-white border-[#00a4dc]/40');
    div.innerHTML = `
        <span class="w-2 h-2 rounded-full ${isError ? 'bg-red-300' : 'bg-[#00a4dc]'} shadow-sm"></span>
        <span class="leading-snug">${escapeHtml(msg)}</span>
    `;
    c.appendChild(div);
    setTimeout(() => {
        div.classList.add('removing');
        setTimeout(() => div.remove(), 250);
    }, 3200);
}

function escapeHtml(str) {
    if (!str) return '';
    return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// Inner markup of a scan ring: nothing when the library is not scanning, else a progressbar with a track and an arc
function scanRingInner(libId, name) {
    const s = scanStatuses[libId];
    if (s === undefined) return '';
    const p = s.percent;
    const known = p > 0;
    const label = escapeHtml(s.queued ? t('library.scan_queued', { name }) : known ? t('library.scanning_percent', { name, percent: p }) : t('library.scanning', { name }));
    const valueNow = known && !s.queued ? ` aria-valuenow="${p}"` : '';
    const spin = known || s.queued ? '' : ' motion-safe:animate-spin';
    const arc = known ? p : 25;
    // A queued scan has no arc: only the track, drawn dashed
    const track = s.queued
        ? '<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2.5" opacity="0.25" pathLength="100" stroke-dasharray="6.25 6.25"/>'
        : '<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2.5" opacity="0.25"/>';
    const arcCircle = s.queued ? '' : `<circle cx="8" cy="8" r="6" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" pathLength="100" stroke-dasharray="${arc} 100" transform="rotate(-90 8 8)"/>`;
    return `<span role="progressbar" class="block w-full h-full text-[#00a4dc]" title="${label}" aria-label="${label}" aria-valuemin="0" aria-valuemax="100"${valueNow}>`
        + `<svg aria-hidden="true" viewBox="0 0 16 16" class="w-full h-full${spin}">`
        + track + arcCircle
        + '</svg></span>';
}

// A ring next to a library name, sized by `size`. The wrapper is hidden by empty:hidden while the library is not scanning.
function scanRing(libId, name, size = 'w-4 h-4') {
    return `<span data-scan-ring="${escapeHtml(libId)}" data-name="${escapeHtml(name)}" class="inline-flex flex-shrink-0 ${size} empty:hidden">${scanRingInner(libId, name)}</span>`;
}

// Redraws the rings of one library, or of every library when called without an id
function updateScanRings(libId) {
    const selector = libId === undefined ? '[data-scan-ring]' : `[data-scan-ring="${CSS.escape(libId)}"]`;
    document.querySelectorAll(selector).forEach(el => {
        el.innerHTML = scanRingInner(el.dataset.scanRing, el.dataset.name);
    });
}

function formatRuntime(ticks) {
    if (!ticks) return '';
    const totalMinutes = Math.round(ticks / 600000000);
    if (totalMinutes < 60) return t('common.minutes_short', { count: totalMinutes });
    const hours = Math.floor(totalMinutes / 60);
    const mins = totalMinutes % 60;
    const hoursText = t('common.hours_short', { count: hours });
    if (mins === 0) return hoursText;
    return t('common.hours_minutes_short', { hours: hoursText, minutes: t('common.minutes_short', { count: mins }) });
}

function formatDurationSeconds(secs) {
    if (isNaN(secs) || secs === null) return '0:00';
    const m = Math.floor(secs / 60);
    const s = Math.floor(secs % 60);
    return `${m}:${s < 10 ? '0' : ''}${s}`;
}

// --- Setup Wizard ---
async function checkSetupWizard() {
    try {
        const info = await API.getPublicInfo();
        const done = info.StartupWizardCompleted ?? info.startup_wizard_completed ?? true;
        // Update version badge with server version
        const reportedVersion = info.RustyfinVersion || info.Version || info.version;
        if (reportedVersion) {
            serverVersion = reportedVersion;
            const badge = $('version-badge');
            if (badge) badge.textContent = serverVersion;
        }
        if (!done) {
            initWizardLanguage();
            if (!document.querySelector('#wiz-man-libs .wiz-man-lib')) wizAddManualLibrary(false);
            wizSetStep(0);
            show('setup-wizard');
            wizFocusStep(wizStep);
            hide('main-navbar');
            if (!wizListening) {
                // Another tab or device may finish the setup while this page waits
                wizListening = true;
                window.addEventListener('focus', () => wizLeaveIfDone());
                document.addEventListener('visibilitychange', () => { if (!document.hidden) wizLeaveIfDone(); });
            }
            return true;
        }
    } catch (e) { console.warn('Could not read the server info for the setup wizard:', e); }
    return false;
}

// The display language picker of the first step; the metadata language follows it
function initWizardLanguage() {
    const select = $('wiz-ui-lang');
    select.innerHTML = uiLanguageOptions().map(o => `<option value="${o.value}">${escapeHtml(o.label)}</option>`).join('');
    select.value = getUiLanguage();
    matchMetadataLanguage();
}

async function wizSetUiLanguage(lng) {
    await setUiLanguage(lng);
    matchMetadataLanguage();
}

function matchMetadataLanguage() {
    const metadataSelect = $('wiz-lang');
    const lng = getUiLanguage().split('-')[0];
    if ([...metadataSelect.options].some(o => o.value === lng)) metadataSelect.value = lng;
}

// The last step index, counted from the DOM so a step added later needs no change here
function wizLastStep() { return document.querySelectorAll('#setup-wizard .jf-wizard-step').length - 1; }

function wizStepIndexOf(id) {
    const step = $(id).closest('.jf-wizard-step');
    return [...document.querySelectorAll('#setup-wizard .jf-wizard-step')].indexOf(step);
}

// The choice on step 1: 'yes' imports from Jellyfin; no choice yet counts as the manual path
function wizChoice() { return document.querySelector('input[name="wiz-import"]:checked')?.value ?? ''; }
function wizPath() { return wizChoice() === 'yes' ? 'import' : 'manual'; }

// Hides the wizard's steps, rows and dots that belong to the other path
function wizApplyPath() {
    // Picking either option clears the "choose one" error from the choice step
    wizSetError($('wiz-choice-error'), '');
    const path = wizPath();
    document.querySelectorAll('#setup-wizard [data-wiz-path]').forEach(el => el.classList.toggle('hidden', el.dataset.wizPath !== path));
}

// DOM indexes of the steps the chosen path shows
function wizActiveIndexes() {
    return [...document.querySelectorAll('#setup-wizard .jf-wizard-step')].flatMap((el, i) => el.classList.contains('hidden') ? [] : [i]);
}

// The step Next (delta 1) or Back (delta -1) goes to, counted within the chosen path
function wizMove(delta) {
    wizApplyPath();
    const steps = wizActiveIndexes();
    const at = steps.indexOf(wizStep);
    return steps[Math.min(steps.length - 1, Math.max(0, at + delta))];
}

function wizSetStep(s) {
    wizStep = s;
    wizApplyPath();
    const steps = wizActiveIndexes(), pos = steps.indexOf(s);
    document.querySelectorAll('#setup-wizard .jf-wizard-step').forEach((el, i) => el.classList.toggle('active', i === s));
    for (let i = 0; i <= wizLastStep(); i++) {
        const dot = $('wiz-dot-' + i);
        if (!dot) continue;
        const j = steps.indexOf(i);
        dot.className = j === pos
            ? 'h-2 rounded-full transition-all duration-300 w-8 bg-[#00a4dc]'
            : (j >= 0 && j < pos ? 'h-2 rounded-full transition-all duration-300 w-4 bg-[#00a4dc]/50' : 'h-2 rounded-full transition-all duration-300 w-2 bg-white/20');
        dot.classList.toggle('hidden', j < 0);
    }
    if (s === WIZ.account && wizPath() === 'import') prefillAccountName();
    if (s === WIZ.others) fillOtherUsers();
    if (s === wizLastStep()) {
        $('wiz-summary-user').textContent = $('wiz-username').value.trim();
        const langEl = $('wiz-lang');
        $('wiz-summary-lang').textContent = langEl.options[langEl.selectedIndex].text;
        const libs = wizManualRowsCheck(wizManualRows()).map(r => r.library).filter(Boolean);
        const summary = $('wiz-summary-library');
        summary.classList.add('whitespace-pre-line');
        summary.textContent = libs.length
            ? libs.map(l => `${l.name} (${t(WIZ_LIB_TYPES[l.type])}): ${l.paths.join(', ')}`).join('\n')
            : t('wizard.summary_no_library');
        if (wizPath() === 'import') {
            const names = (list) => list.length ? list.join(', ') : t('wizard.summary_none');
            $('wiz-summary-jf-folder').textContent = wizPreview.path;
            $('wiz-summary-jf-user').textContent = wizPickedUser().Name;
            $('wiz-summary-jf-libraries').textContent = names(wizCheckedLibRows().map(r => r.querySelector('.wiz-lib-name').value.trim()));
            $('wiz-summary-jf-users').textContent = names([...document.querySelectorAll('.wiz-other-user:checked')].map(c => c.parentElement.textContent.trim()));
        }
    }
    wizFocusStep(s);
}

// Focuses the step's first visible, enabled field, else its primary button (Enter then clicks it).
function wizFocusStep(s) {
    const target = [...document.querySelectorAll('#wiz-step-' + s + ' input, #wiz-step-' + s + ' select')]
        .find(el => el.type !== 'hidden' && !el.disabled && el.offsetParent !== null)
        || document.querySelector('#wiz-step-' + s + ' .jf-btn-primary');
    if (target) target.focus();
}

// Enter in a wizard field clicks the active step's primary button (the wizard has no <form>).
function wizEnterKey(e) {
    if (e.key !== 'Enter') return;
    // A held-down Enter repeats: swallow it in the wizard, so a step's newly focused button isn't pressed
    if (e.repeat) { if (e.target?.closest?.('#setup-wizard')) e.preventDefault(); return; }
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229) return;
    if (e.shiftKey || e.ctrlKey || e.altKey || e.metaKey) return;
    const el = e.target;
    if (!el || (el.tagName !== 'INPUT' && el.tagName !== 'SELECT')) return;
    if (el.tagName === 'INPUT' && ['button', 'submit', 'reset', 'image', 'file'].includes(el.type)) return;
    const step = el.closest('#setup-wizard .jf-wizard-step.active');
    if (!step) return;
    const btn = step.querySelector('.jf-btn-primary');
    if (!btn) return;
    e.preventDefault();
    btn.click();
}
document.addEventListener('keydown', wizEnterKey);

// --- Jellyfin import in the wizard (helpers for the steps) ---
function wizClone(id) { return $(id).content.cloneNode(true); }

// Shows or hides an error line under a field; an empty message hides it
function wizSetError(el, message) {
    el.textContent = message;
    el.classList.toggle('hidden', !message);
    return !message;
}

function wizPickedUser() {
    const id = document.querySelector('input[name="wiz-jf-user"]:checked')?.value;
    return wizPreview?.data.Users.find(u => u.JellyfinId === id) ?? null;
}

// The account name follows the picked Jellyfin user until the admin types another name
function prefillAccountName() {
    const picked = wizPickedUser();
    if (!picked) return;
    const name = $('wiz-username');
    if (!name.value.trim() || name.value === wizNameDefault) name.value = picked.Name;
    wizNameDefault = picked.Name;
}

// One checkbox per Jellyfin user except the picked one, all checked; rebuilt only when the pick changes
function fillOtherUsers() {
    const picked = wizPickedUser();
    if (!picked || wizOthersFor === picked.JellyfinId) return;
    wizOthersFor = picked.JellyfinId;
    $('wiz-jf-others').replaceChildren(...wizPreview.data.Users.filter(u => u !== picked).map(u => {
        const frag = wizClone('wiz-tpl-other');
        frag.querySelector('.wiz-name').textContent = u.Name;
        frag.querySelector('.wiz-other-user').value = u.JellyfinId;
        return frag;
    }));
}

// The user and library lists of steps 3 and 4 for a new preview
function wizFillLists(data) {
    $('wiz-jf-users').replaceChildren(...data.Users.map(u => {
        const frag = wizClone('wiz-tpl-user');
        frag.querySelector('input').value = u.JellyfinId;
        frag.querySelector('.wiz-name').textContent = u.Name;
        const tag = frag.querySelector('.wiz-admin-tag');
        tag.hidden = !u.WasAdministrator;
        tag.textContent = t('wizard.user_admin_tag');
        frag.querySelector('.wiz-played').textContent = t('wizard.user_played', { count: u.PlayedItems });
        return frag;
    }));
    $('wiz-jf-libs').replaceChildren(...data.Libraries.map(wizLibraryRow));
    wizOthersFor = null;
}

const WIZ_LIB_TYPES = { Movies: 'library.type_movies', TvShows: 'library.type_shows', Music: 'library.type_music', Photos: 'library.type_photos', Mixed: 'library.type_mixed' };

function wizLibraryRow(lib) {
    const frag = wizClone('wiz-tpl-lib');
    const row = frag.querySelector('.wiz-lib-row');
    const supported = lib.ContentType !== null;
    const check = row.querySelector('.wiz-lib-check');
    check.checked = supported;
    check.disabled = !supported;
    row.querySelector('.wiz-lib-jf-name').textContent = lib.Name;
    const note = row.querySelector('.wiz-lib-unsupported');
    note.textContent = t('wizard.lib_unsupported');
    note.classList.toggle('hidden', supported);
    row.querySelector('.wiz-lib-name').value = lib.Name;
    row.querySelector('.wiz-lib-name-label').textContent = t('wizard.library_name');
    row.querySelector('.wiz-lib-type-label').textContent = t('wizard.content_type');
    const type = row.querySelector('.wiz-lib-type');
    type.replaceChildren(...Object.entries(WIZ_LIB_TYPES).map(([value, key]) => new Option(t(key), value)));
    type.value = lib.ContentType ?? 'Mixed';
    // A library without Jellyfin paths gets one empty field: the admin must enter a path for it
    row.querySelector('.wiz-lib-paths').replaceChildren(...(lib.Paths.length ? lib.Paths : ['']).map(wizPathRow));
    // A library whose type can't be created stays read-only: its name, type and paths are disabled too
    row.querySelectorAll('.wiz-lib-name, .wiz-lib-type, .wiz-path-input').forEach(i => { i.disabled = !supported; });
    return frag;
}

// One path of a library: the field holds the Jellyfin path until the admin changes it
function wizPathRow(from) {
    const frag = wizClone('wiz-tpl-path');
    const el = frag.querySelector('.wiz-path');
    el.dataset.from = from;
    el.querySelector('.wiz-path-label').textContent = t('wizard.path_on_server');
    el.querySelector('.wiz-path-input').value = from;
    el.querySelector('.path-field button').textContent = t('library.browse');
    el.querySelector('.wiz-jf-path-label').textContent = t('wizard.jellyfin_path');
    el.querySelector('.wiz-jf-path').textContent = from;
    return frag;
}

// Appends one empty library row to the manual step, focusing its name input when asked
function wizAddManualLibrary(focus = true) {
    const row = wizClone('wiz-tpl-man-lib').querySelector('.wiz-man-lib');
    applyTranslations(row);
    $('wiz-man-libs').appendChild(row);
    if (focus) row.querySelector('.wiz-man-name').focus();
}

function wizCheckedLibRows() {
    return [...document.querySelectorAll('.wiz-lib-row')].filter(r => r.querySelector('.wiz-lib-check').checked);
}

// Answers one { library, error } per manual row: trimmed name, paths trimmed and deduplicated, empty rows skipped
function wizManualRowsCheck(rows) {
    const names = new Set();
    return rows.map(({ name, type, paths }) => {
        const trimmed = name.trim();
        const key = trimmed.toLowerCase();
        const duplicate = trimmed !== '' && names.has(key);
        if (trimmed) names.add(key);
        const list = [...new Set(paths.map(p => p.trim()).filter(Boolean))];
        if (!trimmed && !list.length) return { library: null, error: '' };
        if (!trimmed || !list.length) return { library: null, error: 'wizard.library_incomplete' };
        if (duplicate) return { library: null, error: 'wizard.library_name_duplicate' };
        return { library: { name: trimmed, type, paths: list }, error: '' };
    });
}

// Matches created libraries to wanted ones (name, type, ordered paths) and answers what to keep, remove and create
function wizLibraryPlan(created, wanted) {
    // Two libraries are the same when name, type and the ordered paths are equal
    const same = (a, b) => a.name === b.name && a.type === b.type
        && a.paths.length === b.paths.length && a.paths.every((p, i) => p === b.paths[i]);
    const unmatched = [...wanted];
    const keep = [];
    const remove = [];
    for (const lib of created) {
        // Each wanted library matches at most one created library
        const i = unmatched.findIndex(w => same(lib, w));
        if (i < 0) remove.push(lib);
        else { keep.push(lib); unmatched.splice(i, 1); }
    }
    return { keep, remove, create: unmatched };
}

// Checked libraries need unique names, and every path of them must exist on this server
async function wizCheckLibraries() {
    const rows = wizCheckedLibRows();
    const seen = new Set();
    let ok = true;
    for (const row of rows) {
        const key = row.querySelector('.wiz-lib-name').value.trim().toLowerCase();
        const msg = !key ? t('wizard.lib_name_required') : seen.has(key) ? t('wizard.lib_name_duplicate') : '';
        seen.add(key);
        if (!wizSetError(row.querySelector('.wiz-lib-error'), msg)) ok = false;
    }
    // Without a token (owner created, not signed in) ValidatePath would answer 401; the queue checks the paths then
    const canCheck = !wizProgress.userCreated || wizProgress.loggedIn;
    for (const input of rows.flatMap(r => [...r.querySelectorAll('.wiz-path-input')])) {
        const value = input.value.trim();
        let msg = value ? '' : t('wizard.lib_path_required');
        if (!msg && canCheck) {
            try { await API.validatePath(value); } catch (e) { msg = e.message; }
        }
        if (!wizSetError(input.closest('.wiz-path').querySelector('.wiz-path-error'), msg)) ok = false;
    }
    return ok;
}

// The manual step's rows as typed: { row, name, type, paths, inputs } (inputs are the .wiz-man-path fields)
function wizManualRows() {
    return [...document.querySelectorAll('#wiz-man-libs .wiz-man-lib')].map(row => {
        const inputs = [...row.querySelectorAll('.wiz-man-path')];
        return {
            row,
            name: row.querySelector('.wiz-man-name').value,
            type: row.querySelector('.wiz-man-type').value,
            paths: inputs.map(input => input.value),
            inputs,
        };
    });
}

// Checks every manual row, then each path of the rows without an error; answers true only with no error anywhere
async function wizCheckManualLibraries() {
    const rows = wizManualRows();
    const checked = wizManualRowsCheck(rows);
    for (const input of rows.flatMap(r => r.inputs)) {
        input.removeAttribute('aria-invalid');
        input.classList.remove('ring-1', 'ring-red-500');
    }
    const messages = checked.map(c => c.error ? [t(c.error)] : []);
    // Without a token (owner created, not signed in) ValidatePath would answer 401; the create at Finish checks the paths then
    const canCheck = !(wizProgress.userCreated && !wizProgress.loggedIn);
    for (const [i, row] of rows.entries()) {
        if (checked[i].error || !canCheck) continue;
        for (const input of row.inputs) {
            const value = input.value.trim();
            if (!value) continue;
            try {
                await API.validatePath(value);
            } catch (e) {
                input.setAttribute('aria-invalid', 'true');
                input.classList.add('ring-1', 'ring-red-500');
                messages[i].push(`${value}: ${e.message}`);
            }
        }
    }
    rows.forEach((row, i) => {
        const el = row.row.querySelector('.wiz-man-error');
        el.classList.add('whitespace-pre-line');
        wizSetError(el, messages[i].join('\n'));
    });
    hide('wiz-lib-error');
    return messages.every(m => m.length === 0);
}

// Reads the Jellyfin folder when its path is new; answers whether the step may close
async function wizReadJellyfin() {
    const path = $('wiz-jf-path').value.trim();
    const err = $('wiz-jf-error');
    if (!path) return wizSetError(err, t('wizard.jf_folder_required'));
    if (wizPreview?.path === path) return wizSetError(err, '');
    const btn = document.querySelector('#wiz-step-2 .jf-btn-primary');
    btn.disabled = true;
    try {
        const data = await API.setupJellyfinPreview(path);
        wizPreview = { path, data };
        wizFillLists(data);
        return wizSetError(err, '');
    } catch (e) {
        wizPreview = null;
        return wizSetError(err, e.message);
    } finally {
        btn.disabled = false;
    }
}

// The body of POST /JellyfinImport/Queue: the picked user, the checked users and libraries
function wizQueueBody() {
    return {
        SourcePath: wizPreview.path,
        Layout: wizPreview.data.Layout,
        IntoMyAccount: wizPickedUser().JellyfinId,
        UserIds: [...document.querySelectorAll('.wiz-other-user:checked')].map(c => c.value),
        Libraries: wizCheckedLibRows().map(row => ({
            Name: row.querySelector('.wiz-lib-name').value.trim(),
            ContentType: row.querySelector('.wiz-lib-type').value,
            Paths: [...row.querySelectorAll('.wiz-path')].map(p => ({ From: p.dataset.from, To: p.querySelector('.wiz-path-input').value.trim() }))
        }))
    };
}

// Once the import is queued the Jellyfin choices are fixed: the queue is built from them
function wizLockImport() {
    document.querySelectorAll('#setup-wizard input[name="wiz-import"], #setup-wizard [data-wiz-path="import"] input').forEach(i => { i.disabled = true; });
}

// Closes the wizard when the server reports the setup complete and this page did not create the owner
// (another tab or device finished it, or this page was loaded before). Answers whether it closed.
async function wizLeaveIfDone(force = false) {
    if (wizProgress.userCreated || (wizFinishing && !force) || $('setup-wizard').classList.contains('hidden')) return false;
    let info;
    try { info = await API.getPublicInfo(); } catch (e) { return false; }
    const done = info.StartupWizardCompleted ?? info.startup_wizard_completed ?? false;
    if (!done) return false;
    hide('setup-wizard');
    show('main-navbar');
    updateAuthUI();
    toast(t('wizard.already_complete'));
    if (API.isLoggedIn()) await loadSidebar();
    navigate(API.isLoggedIn() ? 'home' : 'login');
    return true;
}

async function wizNext() {
    if (wizBusy) return;
    wizBusy = true;
    const primary = document.querySelector('#wiz-step-' + wizStep + ' .jf-btn-primary');
    if (primary) primary.disabled = true;
    try {
        if (await wizLeaveIfDone()) return;
        if (await wizCheckStep()) wizSetStep(wizMove(1));
    } finally {
        wizBusy = false;
        if (primary) primary.disabled = false;
    }
}

// Checks the current step before Next leaves it. Answers true to move on; a false or an early return stays.
async function wizCheckStep() {
    if (wizStep === WIZ.choice) {
        const ok = !!wizChoice();
        return wizSetError($('wiz-choice-error'), ok ? '' : t('wizard.import_choice_required'));
    }
    if (wizStep === WIZ.folder) return wizReadJellyfin();
    if (wizStep === WIZ.user) return wizSetError($('wiz-user-pick-error'), wizPickedUser() ? '' : t('wizard.user_required'));
    if (wizStep === WIZ.libs) return wizCheckLibraries();
    if (wizStep === WIZ.library) return wizCheckManualLibraries();
    if (wizStep === WIZ.account) {
        const pw = $('wiz-password').value, pw2 = $('wiz-password2').value;
        const user = $('wiz-username').value.trim();
        if (!user) {
            $('wiz-user-error').textContent = t('wizard.username_required');
            show('wiz-user-error');
            return;
        }
        if (pw && pw !== pw2) {
            $('wiz-user-error').textContent = t('wizard.passwords_mismatch');
            show('wiz-user-error');
            return;
        }
        hide('wiz-user-error');
    }
    return true;
}

async function wizPrev() {
    if (wizFinishing) return;
    if (await wizLeaveIfDone()) return;
    wizSetStep(wizMove(-1));
}

function wizSkipLibrary() {
    document.querySelectorAll('#wiz-man-libs .wiz-man-lib').forEach(row => row.remove());
    wizAddManualLibrary(false);
    hide('wiz-lib-error');
    wizNext();
}

// After a Queue error the finish button retries the queue, and "Finish without importing" appears
function wizShowQueueError(failed) {
    $('wiz-skip-import-btn').classList.toggle('hidden', !failed);
    $('wiz-finish-btn').textContent = t(failed ? 'wizard.retry' : 'wizard.finish');
}

function wizFinishWithoutImport() {
    if (wizFinishing) return;
    wizSkipImport = true;
    return wizComplete();
}

async function wizComplete() {
    if (wizFinishing) return;
    wizFinishing = true;
    const skipImport = wizSkipImport;
    wizSkipImport = false;
    const btn = $('wiz-finish-btn');
    const back = document.querySelector('#wiz-step-' + wizStep + ' [onclick="wizPrev()"]');
    const showFinishError = (message, hint = t('wizard.retry_hint')) => {
        const el = $('wiz-finish-error');
        el.textContent = message;
        el.append(document.createElement('br'), hint);
        show('wiz-finish-error');
    };
    try {
        btn.disabled = true;
        if (back) back.disabled = true;
        hide('wiz-finish-error');
        wizShowQueueError(false);
        if (await wizLeaveIfDone(true)) return;
        if (!wizProgress.configSaved) {
            try {
                await API.updateStartupConfig({
                    PreferredMetadataLanguage: $('wiz-lang').value,
                    MetadataCountryCode: $('wiz-country').value
                });
                wizProgress.configSaved = true;
            } catch (e) {
                showFinishError(t('wizard.config_save_failed', { message: e.message }));
                return;
            }
        }
        if (!wizProgress.userCreated) {
            const name = $('wiz-username').value.trim();
            const password = $('wiz-password').value;
            try {
                await API.createStartupUser(name, password);
                wizCredentials = { name, password };
                wizProgress.userCreated = true;
                $('wiz-username').disabled = true;
                $('wiz-password').disabled = true;
                $('wiz-password2').disabled = true;
            } catch (e) {
                if (e.status === 403 && await wizLeaveIfDone(true)) return;
                showFinishError(t('wizard.user_create_failed', { message: e.message }));
                return;
            }
        }
        if (!wizProgress.loggedIn) {
            try {
                await API.login(wizCredentials.name, wizCredentials.password);
                wizProgress.loggedIn = true;
                wizCredentials = null; // the password isn't needed after the sign-in
                // Replace a socket or retry left from an earlier session, so the new token is used now
                stopScanProgressWebSocket();
                initScanProgressWebSocket();
            } catch (e) {
                showFinishError(t('wizard.login_failed', { name: wizCredentials.name, message: e.message }));
                return;
            }
        }
        // The import runs in the background: Queue creates its libraries and the other users' accounts
        // Queue answers 409 when a queue from an earlier try is already there: that counts as queued
        if (wizPath() === 'import' && !wizProgress.queued && !skipImport) {
            try {
                await API.queueJellyfinImport(wizQueueBody());
            } catch (e) {
                if (e.status !== 409) {
                    showFinishError(e.message, t('wizard.queue_failed_hint'));
                    wizShowQueueError(true);
                    return;
                }
            }
            wizProgress.queued = true;
            wizLockImport();
        }
        // The library step may have changed since this page created its library; the page's last choice wins
        const rows = wizPath() !== 'import' ? wizManualRows() : []; // the import path creates its libraries itself
        const checked = wizManualRowsCheck(rows);
        const wanted = [];
        const rowOf = new Map(); // the wanted library object -> its row element
        checked.forEach((c, i) => { if (c.library) { wanted.push(c.library); rowOf.set(c.library, rows[i].row); } });
        const plan = wizLibraryPlan(wizLibraries, wanted);
        for (const lib of plan.remove) {
            try {
                await API.deleteLibrary(lib.id);
                wizLibraries = wizLibraries.filter(l => l !== lib);
            } catch (e) {
                showFinishError(t('wizard.library_replace_failed', { name: lib.name, message: e.message }));
                return;
            }
        }
        let createFailed = false;
        for (const want of plan.create) {
            const { name, type, paths } = want;
            try {
                const lib = await API.createLibrary(name, type, paths);
                wizLibraries.push({ id: lib.Id || lib.id, name, type, paths });
                // Scanned once setup completes (below), so a Back-edit Finish can't replace it mid-scan
                toast(t('library.created', { name }));
            } catch (e) {
                createFailed = true;
                wizSetError(rowOf.get(want).querySelector('.wiz-man-error'), e.message);
            }
        }
        if (createFailed) {
            wizSetStep(WIZ.library);
            hide('wiz-finish-error');
            wizSetError($('wiz-lib-error'), t('wizard.libraries_create_failed'));
            return;
        }
        try {
            await API.completeStartup();
        } catch (e) {
            showFinishError(t('wizard.complete_failed', { message: e.message }));
            return;
        }
        // Scan the libraries now that setup is done: before, a retry could replace one mid-scan
        for (const lib of wizLibraries) API.scanLibrary(lib.id).catch(err => toast(t('library.scan_start_failed', { message: err.message }), 'error'));
        hide('setup-wizard');
        show('main-navbar');
        updateAuthUI();
        await loadSidebar();
        if (wizPath() === 'import' && !skipImport) navigate('admin', 'import');
        else navigate(API.isLoggedIn() ? 'home' : 'login');
    } finally {
        wizFinishing = false;
        btn.disabled = false;
        if (back) back.disabled = false;
    }
}

// --- Navigation & Routing ---
function navigate(page, data) {
    document.querySelectorAll('[id^="page-"]').forEach(el => el.classList.add('hidden'));
    const el = $('page-' + page);
    if (el) {
        el.classList.remove('hidden');
        el.classList.add('jf-page');
    }
    currentPage = page;
    window.location.hash = data ? `${page}/${data}` : page;

    // Update sidebar active states
    document.querySelectorAll('.jf-drawer-item').forEach(item => item.classList.remove('active'));
    if (page === 'home') $('side-nav-home')?.classList.add('active');
    if (page === 'favorites') $('side-nav-fav')?.classList.add('active');
    if (page === 'admin') $('side-nav-admin')?.classList.add('active');
    if (page === 'settings') $('side-nav-settings')?.classList.add('active');

    const pages = {
        home: renderHome,
        favorites: renderFavorites,
        login: () => renderLogin(data),
        library: () => renderLibrary(data),
        detail: () => renderDetail(data),
        search: () => renderSearch(data),
        person: () => renderPerson(data),
        settings: () => renderSettings(data),
        admin: () => renderAdmin(data)
    };

    if (pages[page]) pages[page]();
    closeUserMenu();
    window.scrollTo({ top: 0, behavior: 'smooth' });
}

// --- UI Navigation Helpers ---
function toggleUserMenu() { $('user-dropdown')?.classList.toggle('hidden'); }
function closeUserMenu() { $('user-dropdown')?.classList.add('hidden'); }
document.addEventListener('click', e => {
    if (!e.target.closest('#nav-user-menu')) closeUserMenu();
});

// Toasts that a scan started, once. Refresh answers only when the scan ends, so the toast shows 300 ms after
// sending, or when the request ends if sooner; a request that fails before then, such as a 404 for a deleted
// library, shows only the error.
function startScanWithToast(request, startedMessage) {
    let started = false;
    const showStarted = () => {
        if (started) return;
        started = true;
        toast(startedMessage);
    };
    const timer = setTimeout(showStarted, 300);
    return request.then(showStarted, err => {
        clearTimeout(timer);
        toast(t(started ? 'library.scan_failed' : 'library.scan_start_failed', { message: err.message }), 'error');
    }).finally(() => clearTimeout(timer));
}

// Clicks on rendered markup are dispatched here from data-action and data-* attributes, so server values never end up inside inline JavaScript.
const clickActions = {
    navigate: el => navigate(el.dataset.page, el.dataset.id),
    // A video with a resume point asks first; the detail page's Resume and Play from beginning buttons are that choice
    play: el => {
        const { id, streamType, title } = el.dataset;
        if (el.dataset.resume === 'true') playItem(id, streamType, title);
        else if (el.dataset.fromStart === 'true') playItem(id, streamType, title, { fromStart: true });
        else playWithResumeChoice(id, streamType, title);
    },
    'scan-library': el => startScanWithToast(API.scanLibrary(el.dataset.id), t('library.scan_started_for', { name: el.dataset.name })),
    'scan-library-page': el => startScanWithToast(API.scanLibrary(el.dataset.id), t('library.media_scan_started_for', { name: el.dataset.name })),
    'scan-library-admin': el => startScanWithToast(API.scanLibrary(el.dataset.id), t('admin.scan_started')),
    'scan-all': () => startScanWithToast(API.scanAllLibraries(), t('admin.scanning_all')),
    'jump-letter': el => jumpToLetter(el.dataset.id, el.dataset.letter),
    'toggle-favorite': el => toggleFavorite(el.dataset.id, el.dataset.favorite === 'true'),
    'toggle-played': el => togglePlayed(el.dataset.id, el.dataset.played === 'true'),
    'refresh-item': el => API.refreshItem(el.dataset.id).then(() => toast(t('detail.metadata_refreshing'))),
    'admin-tab': el => showAdminTab(el, el.dataset.tab),
    'delete-user': el => { if (confirm(t('admin.delete_user_confirm'))) API.deleteUser(el.dataset.id).then(() => renderAdmin()); },
    'set-user-admin': el => setUserPolicy(el, { IsAdministrator: el.checked }),
    'set-user-disabled': el => setUserPolicy(el, { IsDisabled: el.checked }),
    'delete-library': el => deleteLibraryFromAdmin(el.dataset.id),
    'toggle-realtime-monitor': el => toggleRealtimeMonitor(el),
    'start-task': el => startTaskAction(el.dataset.id, el.dataset.name),
    'stop-task': el => stopTaskAction(el.dataset.id, el.dataset.name),
    'browse-path': el => openFolderBrowser(el.closest('.path-field').querySelector('[data-path-field]')),
    'add-lib-path': el => addLibraryPathRow(el.closest('.wiz-man-lib')?.querySelector('.wiz-man-paths') || $('add-lib-paths')),
    'remove-lib-path': el => removeLibraryPathRow(el),
    'wiz-add-lib': () => wizAddManualLibrary(),
    'wiz-remove-lib': el => { el.closest('.wiz-man-lib').remove(); document.querySelector('[data-action="wiz-add-lib"]').focus(); },
    'folder-open': el => folderBrowserLoad(el.dataset.path),
    'folder-up': () => folderBrowserUp(),
    'folder-cancel': () => closeFolderBrowser(),
    'folder-select': () => folderBrowserSelect(),
    'oidc-sign-in': () => startOidcSignIn(),
    'oidc-link': () => startOidcLink(),
    'oidc-unlink': el => unlinkOidc(el.dataset.id),
    'oidc-remove-link': el => removeOidcLinkAsAdmin(el),
    'oidc-copy-callback': () => copyOidcCallbackUrl(),
    'oidc-save-settings': () => saveOidcAdminSettings(),
    'plugin-edit-config': el => openPluginConfig(el.dataset.pluginId),
    'plugin-edit-priority': el => openPluginPriority(el.dataset.pluginId),
};
document.addEventListener('click', e => {
    const el = e.target.closest('[data-action]');
    if (el) clickActions[el.dataset.action]?.(el, e);
});

function toggleSidebar() {
    const m = $('sidebar-mobile'), o = $('sidebar-overlay');
    if (!m || !o) return;
    if (m.classList.contains('hidden')) {
        m.classList.remove('hidden');
        o.classList.remove('hidden');
        setTimeout(() => m.style.transform = 'translateX(0)', 10);
        const content = $('sidebar')?.innerHTML || '';
        $('sidebar-mobile-content').innerHTML = content;
    } else {
        m.style.transform = 'translateX(-100%)';
        o.classList.add('hidden');
        setTimeout(() => m.classList.add('hidden'), 200);
    }
}

// --- Media Card Renderer (12.x Style) ---
// Picks card artwork from tags the item carries, so no image is requested that doesn't exist.
// Returns { id, type, tag, contain } or null (null means the type icon is shown).
function cardImage(item, landscape) {
    const id = item.Id || item.id;
    const pick = (imgId, type, tag, contain = false) => (imgId && tag ? { id: imgId, type, tag, contain } : null);
    if (!landscape) return pick(id, 'Primary', item.ImageTags?.Primary);
    const isEpisode = (item.Type || item.type) === 'Episode';
    return pick(id, 'Thumb', item.ImageTags?.Thumb)
        || pick(id, 'Backdrop', item.BackdropImageTags?.[0])
        || (isEpisode ? pick(id, 'Primary', item.ImageTags?.Primary) : null)
        || pick(item.ParentThumbItemId, 'Thumb', item.ParentThumbImageTag)
        || pick(item.SeriesId, 'Thumb', item.SeriesThumbImageTag)
        || pick(item.ParentBackdropItemId, 'Backdrop', item.ParentBackdropImageTags?.[0])
        || pick(id, 'Primary', item.ImageTags?.Primary, true)
        || pick(item.SeriesId, 'Primary', item.SeriesPrimaryImageTag, true)
        || null;
}

// Own backdrop URL, else the parent's (series or season); '' when neither is tagged.
function backdropUrl(item, maxWidth) {
    if (item.BackdropImageTags?.[0]) {
        return API.imageUrl(item.Id || item.id, 'Backdrop', { maxWidth, tag: item.BackdropImageTags[0] });
    }
    if (item.ParentBackdropItemId && item.ParentBackdropImageTags?.[0]) {
        return API.imageUrl(item.ParentBackdropItemId, 'Backdrop', { maxWidth, tag: item.ParentBackdropImageTags[0] });
    }
    return '';
}

function itemCard(item, options = {}) {
    // rawName goes into the onclick code, which translates the fallback itself
    const rawName = item.Name || item.name || '';
    const name = rawName || t('common.unknown');
    const year = item.ProductionYear || item.production_year || '';
    const type = item.Type || item.type || '';
    const id = item.Id || item.id;
    const played = item.UserData?.Played || item.user_data?.played || false;
    const fav = item.UserData?.IsFavorite || item.user_data?.is_favorite || false;
    const progress = item.UserData?.PlayedPercentage || item.user_data?.played_percentage || item.CompletionPercentage || 0;
    const rating = item.CommunityRating || item.community_rating;
    // Use explicit backdrop option if provided; otherwise derive from type
    const isBackdrop = options.backdrop !== undefined ? options.backdrop : type === 'Episode';

    // Progressive image loading: thumbnail for fast first paint,
    // full-res loaded on hover only to save bandwidth
    const picture = cardImage(item, isBackdrop);
    const thumbSrc = picture ? API.imageUrl(picture.id, picture.type, { maxWidth: 150, format: 'webp', tag: picture.tag }) : '';
    const fullSrc = picture ? API.imageUrl(picture.id, picture.type, { maxWidth: 250, tag: picture.tag }) : '';
    // Episode landscape cards: series name on the title line, "S1:E2 · name" on the second line
    const hasNumbers = item.ParentIndexNumber != null && item.IndexNumber != null;
    const isEpisodeCard = isBackdrop && type === 'Episode' && !!item.SeriesName && hasNumbers;
    const cardTitle = isEpisodeCard ? item.SeriesName : name;
    const episodeLine = isEpisodeCard
        ? t('home.episode_line', {
            season: item.ParentIndexNumber,
            episode: item.IndexNumberEnd != null && item.IndexNumberEnd !== item.IndexNumber
                ? `${item.IndexNumber}-${item.IndexNumberEnd}`
                : item.IndexNumber,
            name,
        })
        : '';

    const canPlay = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video'].includes(type);
    const streamType = type === 'Audio' ? 'Audio' : 'Videos';

    // Type icons
    const typeIcons = {
        Movie: '🎬', Series: '📺', Episode: '📺', Audio: '🎵', MusicAlbum: '💿',
        MusicArtist: '🎤', Folder: '📁', CollectionFolder: '🗂️', Photo: '🖼️'
    };

    return `
    <div class="jf-card group" data-action="navigate" data-page="detail" data-id="${escapeHtml(id)}">
        <div class="${isBackdrop ? 'jf-poster-backdrop' : 'jf-poster'}">
            ${picture ? `
            <div class="jf-poster-skeleton"></div>
            <!-- Thumbnail loads first for instant visual feedback; full-res swaps on hover -->
            <img src="${thumbSrc}" alt="${escapeHtml(cardTitle)}" decoding="async" loading="lazy" class="jf-thumb-img absolute inset-0 w-full h-full object-cover z-[1] opacity-0 blur-sm scale-105 transition-opacity duration-200${picture.contain ? ' jf-img-contain' : ''}"
                 data-full-src="${fullSrc}"
                 onload="this.style.opacity='0.8';this.classList.remove('blur-sm','scale-105');const sk=this.previousElementSibling;if(sk&&sk.classList.contains('jf-poster-skeleton'))sk.style.display='none';"
                 onerror="this.style.display='none';const sk=this.previousElementSibling;if(sk&&sk.classList.contains('jf-poster-skeleton'))sk.style.display='none';const ic=this.nextElementSibling;if(ic)ic.style.display='flex';">` : ''}
            <div class="${picture ? 'hidden' : 'flex'} items-center justify-center h-full text-4xl text-white/20 absolute inset-0 bg-white/5"${picture ? ' style="display:none"' : ''}>
                ${typeIcons[type] || '📁'}
            </div>

            <!-- Top Badges -->
            <div class="absolute top-2 left-2 right-2 flex items-center justify-between pointer-events-none z-10">
                ${fav ? '<span class="text-rose-500 bg-black/60 backdrop-blur-md p-1 rounded-full text-xs leading-none">♥</span>' : '<span></span>'}
                ${rating ? `<span class="jf-badge jf-badge-rating text-[10px] !py-0.5 !px-1.5 shadow-md">★ ${rating.toFixed(1)}</span>` : ''}
            </div>

            <!-- Watched checkmark badge -->
            ${played ? `
            <div class="absolute top-2 right-2 w-6 h-6 rounded-full bg-[#00a4dc] text-white flex items-center justify-center shadow-lg border border-white/20 z-10">
                <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="3" d="M5 13l4 4L19 7"/></svg>
            </div>` : ''}

            <!-- Play Button Hover Overlay -->
            ${canPlay ? `
            <div class="jf-play-overlay" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(rawName || t('common.unknown'))}">
                <div class="jf-play-btn-circle">
                    <svg class="w-7 h-7 text-white ml-1" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
                </div>
            </div>` : ''}

            <!-- Progress Bar -->
            ${progress > 0 && progress < 100 ? `
            <div class="jf-progress">
                <div class="jf-progress-bar" style="width:${progress}%"></div>
            </div>` : ''}
        </div>

        <div class="p-3">
            <p class="text-xs sm:text-sm font-semibold truncate text-white group-hover:text-[#38bdf8] transition-colors" title="${escapeHtml(cardTitle)}">${escapeHtml(cardTitle)}</p>
            <div class="flex items-center gap-1.5 text-[11px] text-white/40 mt-0.5">
                ${isEpisodeCard ? `<span class="truncate">${escapeHtml(episodeLine)}</span>` : `
                ${year ? `<span>${year}</span>` : ''}
                ${year && type ? `<span>·</span>` : ''}
                <span class="truncate">${escapeHtml(type)}</span>`}
            </div>
        </div>
    </div>`;
}

// --- Create a single card DOM element (avoids innerHTML parsing overhead) ---
function createCardElement(item, wrapperClass = '', cardOptions = {}) {
    const wrapper = document.createElement('div');
    wrapper.innerHTML = itemCard(item, cardOptions);
    const card = wrapper.firstElementChild;
    if (card && wrapperClass) card.classList.add(...wrapperClass.split(/\s+/).filter(Boolean));
    return card;
}

function sectionRow(title, items, moreAction = null, options = {}) {
    if (!items || items.length === 0) return '';
    const row = document.createElement('div');
    row.className = 'mb-10';
    const header = document.createElement('div');
    header.className = 'flex items-center justify-between mb-4';
    header.innerHTML = `
        <div class="flex items-center gap-2">
            <span class="w-2.5 h-2.5 rounded-full bg-[#00a4dc]"></span>
            <h2 class="text-lg lg:text-xl font-bold tracking-tight text-white">${escapeHtml(title)}</h2>
            <span class="text-xs text-white/30 font-medium ml-1">(${items.length})</span>
        </div>
        ${moreAction ? `<button class="text-xs font-semibold text-[#00a4dc] hover:text-[#38bdf8] flex items-center gap-1 transition" data-action="navigate" data-page="${escapeHtml(moreAction.page)}" data-id="${escapeHtml(moreAction.data)}">${t('common.view_all')}</button>` : ''}
    `;
    row.appendChild(header.firstElementChild);
    if (moreAction) row.appendChild(header.lastElementChild);
    const scrollRow = document.createElement('div');
    scrollRow.className = 'jf-scroll-row pb-2';
    const fragment = document.createDocumentFragment();
    // Force consistent poster aspect-ratio (2/3) for all cards in a row,
    // regardless of item type — prevents mixed Movie/Episode cards from
    // having wildly different heights (2/3 vs 16/9)
    const useBackdrop = options.backdrop === true;
    const wrapperCls = useBackdrop ? 'w-64 sm:w-72' : 'w-36 sm:w-44 md:w-48';
    items.forEach(it => {
        const cardOpts = { ...options, backdrop: useBackdrop };
        const card = createCardElement(it, wrapperCls, cardOpts);
        if (card) fragment.appendChild(card);
    });
    scrollRow.appendChild(fragment);
    row.appendChild(scrollRow);
    return row.outerHTML;
}

// --- Home Page Renderer ---
async function renderHome() {
    const session = API.token; // stop drawing if this session ends meanwhile
    const page = $('page-home');
    if (!API.isLoggedIn()) {
        try {
            const info = await API.getPublicInfo();
            if (API.token !== session) return;
            const sn = info.ServerName || info.server_name || 'Rustyfin';
            const ver = info.RustyfinVersion || info.Version || info.version || serverVersion;
            page.innerHTML = `
            <div class="flex flex-col items-center justify-center min-h-[75vh] text-center jf-page px-4">
                <div class="relative mb-6">
                    <img src="/web/assets/logo.png" alt="Rustyfin" class="w-32 h-32 rounded-3xl shadow-2xl p-2 bg-gradient-to-tr from-white/10 to-transparent border border-white/10">
                    <span class="absolute -bottom-2 right-2 px-2.5 py-0.5 rounded-full text-[10px] font-bold bg-[#00a4dc] text-white shadow-lg">v${escapeHtml(ver)}</span>
                </div>
                <h1 class="text-4xl sm:text-5xl font-black mb-3 tracking-tight text-white">${escapeHtml(sn)}</h1>
                <p class="text-white/60 text-base max-w-lg mb-8 leading-relaxed">
                    ${t('home.tagline')}
                </p>
                <div class="grid grid-cols-3 gap-6 mb-10 w-full max-w-md">
                    <div class="p-4 rounded-xl bg-white/5 border border-white/10 text-center">
                        <p class="text-xl font-bold text-emerald-400">${t('home.online')}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">${t('common.status')}</p>
                    </div>
                    <div class="p-4 rounded-xl bg-white/5 border border-white/10 text-center">
                        <p class="text-xl font-bold text-[#00a4dc]">&lt; 20 MB</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">${t('home.ram_idle')}</p>
                    </div>
                    <div class="p-4 rounded-xl bg-white/5 border border-white/10 text-center">
                        <p class="text-xl font-bold text-amber-400">v${escapeHtml(ver)}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">${t('home.release')}</p>
                    </div>
                </div>
                <button class="jf-btn-primary !px-8 !py-3.5 text-base shadow-xl" onclick="navigate('login')">
                    ${t('home.sign_in_now')}
                </button>
            </div>`;
        } catch {
            if (API.token !== session) return;
            page.innerHTML = `<div class="p-10 text-center text-red-400 jf-page">${t('home.server_unreachable')}</div>`;
        }
        return;
    }

    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        let libsError = null;
        const [resumeData, nextUpData, latestData, libsData] = await Promise.all([
            (API.getResume ? API.getResume(12) : Promise.resolve({ Items: [] })).catch(() => ({ Items: [] })),
            (API.getNextUp ? API.getNextUp(12) : Promise.resolve({ Items: [] })).catch(() => ({ Items: [] })),
            (API.getLatest ? API.getLatest(20) : Promise.resolve([])).catch(() => []),
            API.getLibraries().catch(err => { libsError = err; return null; })
        ]);
        if (API.token !== session) return;

        const resume = resumeData.Items || resumeData.items || [];
        const nextUp = nextUpData.Items || nextUpData.items || [];
        const latest = Array.isArray(latestData) ? latestData : (latestData.Items || latestData.items || []);
        if (!libsError) libraries = libsData || [];

        let c = '<div class="jf-page space-y-8">';

        // 1. Featured Spotlight Hero (Pick top item from latest or resume)
        const spotlightItem = resume[0] || latest[0];
        if (spotlightItem) {
            const spId = spotlightItem.Id || spotlightItem.id;
            const spName = spotlightItem.Name || spotlightItem.name || '';
            const spOverview = spotlightItem.Overview || spotlightItem.overview || '';
            const spYear = spotlightItem.ProductionYear || spotlightItem.production_year || '';
            const spRating = spotlightItem.CommunityRating || spotlightItem.community_rating;
            const spType = spotlightItem.Type || spotlightItem.type || 'Movie';
            const spBackdrop = backdropUrl(spotlightItem, 1920);
            const canPlay = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video'].includes(spType);
            const streamType = spType === 'Audio' ? 'Audio' : 'Videos';

            c += `
            <div class="jf-spotlight mb-10">
                ${spBackdrop ? `<img src="${spBackdrop}" class="jf-spotlight-backdrop" onerror="this.style.display='none'">` : ''}
                <div class="jf-spotlight-gradient"></div>
                <div class="relative z-10 p-6 sm:p-10 lg:p-14 flex flex-col justify-end min-h-[420px] max-w-3xl">
                    <div class="flex items-center gap-2 mb-3">
                        <span class="jf-badge jf-badge-accent uppercase tracking-wider text-[11px]">${t('home.highlights')}</span>
                        <span class="jf-badge jf-badge-primary uppercase tracking-wider text-[11px]">${escapeHtml(spType)}</span>
                        ${spYear ? `<span class="jf-badge text-[11px]">${escapeHtml(spYear)}</span>` : ''}
                        ${spRating ? `<span class="jf-badge jf-badge-rating text-[11px]">★ ${spRating.toFixed(1)}</span>` : ''}
                    </div>
                    <h1 class="text-3xl sm:text-4xl lg:text-5xl font-black text-white tracking-tight mb-3 drop-shadow-md">${escapeHtml(spName)}</h1>
                    ${spOverview ? `<p class="text-white/80 text-xs sm:text-sm line-clamp-3 mb-6 max-w-2xl leading-relaxed">${escapeHtml(spOverview)}</p>` : ''}
                    <div class="flex flex-wrap items-center gap-3">
                        ${canPlay ? `
                        <button class="jf-btn-primary !py-2.5 !px-6 text-sm" data-action="play" data-id="${escapeHtml(spId)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(spName)}">
                            <svg class="w-4 h-4" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
                            ${t('common.play')}
                        </button>` : ''}
                        <button class="jf-btn-ghost !py-2.5 !px-5 text-sm" data-action="navigate" data-page="detail" data-id="${escapeHtml(spId)}">
                            ${t('home.more_info')}
                        </button>
                    </div>
                </div>
            </div>`;
        }

        // 2. "Meine Medien" Library Tiles
        if (libsError) {
            c += `
            <div class="mb-10">
                <div class="flex items-center gap-2 mb-4">
                    <span class="w-2.5 h-2.5 rounded-full bg-[#00a4dc]"></span>
                    <h2 class="text-lg lg:text-xl font-bold tracking-tight text-white">${t('home.my_media')}</h2>
                </div>
                <p class="text-sm text-red-400">${t('library.list_failed', { message: escapeHtml(libsError.message) })}</p>
            </div>`;
        } else if (libraries.length > 0) {
            c += `
            <div class="mb-10">
                <div class="flex items-center gap-2 mb-4">
                    <span class="w-2.5 h-2.5 rounded-full bg-[#00a4dc]"></span>
                    <h2 class="text-lg lg:text-xl font-bold tracking-tight text-white">${t('home.my_media')}</h2>
                </div>
                <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 gap-4">
                    ${libraries.map(lib => {
                        const libId = lib.id || lib.Id;
                        const libName = lib.name || lib.Name;
                        const libType = lib.content_type || lib.ContentType || 'Mixed';
                        const gradients = {
                            Movies: 'from-blue-600/40 to-cyan-500/10 border-blue-500/30',
                            TvShows: 'from-purple-600/40 to-indigo-500/10 border-purple-500/30',
                            Music: 'from-amber-600/40 to-orange-500/10 border-amber-500/30',
                            Photos: 'from-emerald-600/40 to-teal-500/10 border-emerald-500/30',
                            Mixed: 'from-slate-600/40 to-zinc-500/10 border-slate-500/30'
                        };
                        const icons = { Movies: '🎬', TvShows: '📺', Music: '🎵', Photos: '🖼️', Mixed: '📁' };
                        return `
                        <div class="jf-card p-5 bg-gradient-to-br ${gradients[libType] || gradients.Mixed} flex flex-col justify-between min-h-[120px] relative" data-action="navigate" data-page="library" data-id="${escapeHtml(libId)}">
                            <div class="flex items-center justify-between">
                                <span class="text-3xl">${icons[libType] || '📁'}</span>
                                <div class="flex items-center gap-1">
                                    <button type="button" class="w-6 h-6 rounded flex items-center justify-center text-white/40 hover:text-white hover:bg-white/10 transition" data-action="scan-library" data-id="${escapeHtml(libId)}" data-name="${escapeHtml(libName)}" title="${escapeHtml(t('library.scan_library'))}">
                                        <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/></svg>
                                    </button>
                                </div>
                            </div>
                            <div>
                                <div class="flex items-center gap-1.5 min-w-0">
                                    <p class="font-bold text-white text-base truncate">${escapeHtml(libName)}</p>
                                    ${scanRing(libId, libName)}
                                </div>
                            </div>
                        </div>`;
                    }).join('')}
                </div>
            </div>`;
        }

        // 3. Sections: Continue Watching & Next Up & Latest
        c += sectionRow(t('home.continue_watching'), resume, null, { backdrop: true });
        c += sectionRow(t('home.next_up'), nextUp, null, { backdrop: true });
        c += sectionRow(t('home.recently_added'), latest);

        // Empty Library Welcome Banner
        if (!resume.length && !nextUp.length && !latest.length) {
            c += `
            <div class="text-center py-20 px-4 rounded-2xl bg-white/5 border border-white/10 my-8">
                <img src="/web/assets/logo.png" class="w-20 h-20 mx-auto rounded-2xl opacity-40 mb-4 p-1 bg-white/5">
                <h3 class="text-xl font-bold text-white mb-2">${t('home.empty_title')}</h3>
                <p class="text-white/50 text-sm max-w-md mx-auto mb-6">${t('home.empty_hint')}</p>
                ${API.isAdmin ? `
                <button class="jf-btn-primary" onclick="openAddLibrary()">
                    ${t('home.create_first_library')}
                </button>` : ''}
            </div>`;
        }

        c += '</div>';
        page.innerHTML = c;
    } catch (e) {
        if (API.token !== session) return;
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 border border-red-500/20 text-red-400 jf-page">${t('home.load_failed', { message: escapeHtml(e.message) })}</div>`;
    }
}

// --- Favorites Page Renderer (with pagination) ---
async function renderFavorites() {
    const session = API.token;
    favoritesPage = 0;
    destroyInfiniteScroll();
    const page = $('page-favorites');
    // Show skeleton grid immediately
    page.innerHTML = `
    <div class="jf-page">
        <div class="flex items-center gap-3 mb-6">
            <span class="w-3 h-3 rounded-full bg-rose-500 shadow-md"></span>
            <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${t('favorites.title')}</h1>
        </div>
        <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4">
            ${Array.from({length: 12}, () => `
            <div class="jf-card">
                <div class="jf-poster"><div class="jf-poster-skeleton"></div></div>
                <div class="p-3">
                    <div class="h-3 w-3/4 bg-white/5 rounded animate-pulse mb-1"></div>
                    <div class="h-2.5 w-1/2 bg-white/5 rounded animate-pulse"></div>
                </div>
            </div>`).join('')}
        </div>
    </div>`;
    try {
        // Use pagination to avoid loading all favorites at once
        const data = await API.getUserItems({ IsFavorite: true, StartIndex: 0, Limit: LIBRARY_PAGE_SIZE });
        if (API.token !== session) return;
        const items = data.Items || data.items || [];
        const total = data.TotalRecordCount || data.total_record_count || items.length;
        const hasMore = items.length >= LIBRARY_PAGE_SIZE;

        if (items.length > 0) {
            page.innerHTML = `
            <div class="jf-page">
                <div class="flex items-center gap-3 mb-6">
                    <span class="w-3 h-3 rounded-full bg-rose-500 shadow-md"></span>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${t('favorites.title')}</h1>
                    <span class="text-xs text-white/40">(${total})</span>
                </div>
                <div id="fav-grid" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4"></div>
                ${hasMore ? `
                <div id="fav-infinite-container" class="text-center mt-8">
                    <div id="infinite-scroll-spinner" class="jf-infinite-spinner" style="display:none"><div class="jf-spinner"></div></div>
                    <div id="infinite-scroll-sentinel" class="jf-infinite-sentinel"></div>
                </div>` : ''}
            </div>`;
            appendFavoriteCards(items);
        } else {
            page.innerHTML = `
            <div class="jf-page">
                <div class="flex items-center gap-3 mb-6">
                    <span class="w-3 h-3 rounded-full bg-rose-500 shadow-md"></span>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${t('favorites.title')}</h1>
                    <span class="text-xs text-white/40">(${total})</span>
                </div>
                <div class="text-center py-24 text-white/40">
                    <p class="text-4xl mb-3">♡</p>
                    <p class="text-base font-semibold">${t('favorites.empty')}</p>
                    <p class="text-xs text-white/30 mt-1">${t('favorites.empty_hint')}</p>
                </div>
            </div>`;
        }
        // Setup infinite scroll for favorites
        if (hasMore) {
            setupInfiniteScroll(() => loadMoreFavoritesInfinite(total));
        }
    } catch (e) {
        if (API.token !== session) return;
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

let favoritesPage = 0;

// Appends one card per item to the favorites grid
function appendFavoriteCards(items) {
    const grid = $('fav-grid');
    if (!grid) return;
    const frag = document.createDocumentFragment();
    for (const item of items) {
        const card = createCardElement(item);
        if (card) frag.appendChild(card);
    }
    grid.appendChild(frag);
    initLazyImages(grid);
}

async function loadMoreFavoritesInfinite(initialTotal) {
    try {
        favoritesPage++;
        const startIndex = favoritesPage * LIBRARY_PAGE_SIZE;
        const data = await API.getUserItems({ IsFavorite: true, StartIndex: startIndex, Limit: LIBRARY_PAGE_SIZE });
        const items = data.Items || data.items || [];
        const total = data.TotalRecordCount || data.total_record_count || initialTotal;

        appendFavoriteCards(items);

        // If no more items, destroy infinite scroll and remove sentinel
        const remaining = total - (favoritesPage + 1) * LIBRARY_PAGE_SIZE;
        if (remaining <= 0) {
            destroyInfiniteScroll();
            const sentinelContainer = $('fav-infinite-container');
            if (sentinelContainer) sentinelContainer.remove();
        }
    } catch (e) {
        toast(t('common.error_message', { message: e.message }), 'error');
    }
}

// Library list state: each load gets a new generation, and responses from older generations are dropped
let libraryGen = 0;
let libraryQuery = null;
let libraryTotal = null;
let libraryObserver = null;
let libraryRequested = new Set();
let letterClicks = 0;
// The letter rail: the anchor of the last jump ({ k, first }) and the pending frame of an update
let railAnchor = null;
let railFrame = 0;
const LIBRARY_PAGE_SIZE = 50;

// --- Infinite Scroll via IntersectionObserver ---
let infiniteScrollObserver = null;
let infiniteScrollLoading = false;
let infiniteScrollCallback = null;

function setupInfiniteScroll(callback) {
    infiniteScrollCallback = callback;
    infiniteScrollLoading = false;
    
    // Destroy existing observer
    if (infiniteScrollObserver) {
        infiniteScrollObserver.disconnect();
    }
    
    // Find or create sentinel element
    let sentinel = document.getElementById('infinite-scroll-sentinel');
    if (!sentinel) {
        sentinel = document.createElement('div');
        sentinel.id = 'infinite-scroll-sentinel';
        sentinel.className = 'jf-infinite-sentinel';
    }
    
    // Observe sentinel
    infiniteScrollObserver = new IntersectionObserver((entries) => {
        if (entries[0].isIntersecting && !infiniteScrollLoading && infiniteScrollCallback) {
            infiniteScrollLoading = true;
            // Show spinner
            const spinner = document.getElementById('infinite-scroll-spinner');
            if (spinner) spinner.style.display = 'flex';
            
            infiniteScrollCallback().finally(() => {
                infiniteScrollLoading = false;
                // Hide spinner after a short delay
                if (spinner) setTimeout(() => { spinner.style.display = 'none'; }, 300);
            });
        }
    }, { rootMargin: '400px' });
    
    infiniteScrollObserver.observe(sentinel);
}

function destroyInfiniteScroll() {
    if (infiniteScrollObserver) {
        infiniteScrollObserver.disconnect();
        infiniteScrollObserver = null;
    }
    infiniteScrollCallback = null;
    infiniteScrollLoading = false;
    const sentinel = document.getElementById('infinite-scroll-sentinel');
    if (sentinel) sentinel.remove();
    const spinner = document.getElementById('infinite-scroll-spinner');
    if (spinner) spinner.remove();
}

function addInfiniteScrollSentinel(container) {
    // Remove existing sentinel and spinner
    const existingSentinel = container.querySelector('#infinite-scroll-sentinel');
    const existingSpinner = container.querySelector('#infinite-scroll-spinner');
    if (existingSentinel) existingSentinel.remove();
    if (existingSpinner) existingSpinner.remove();
    
    // Add loading spinner
    const spinner = document.createElement('div');
    spinner.id = 'infinite-scroll-spinner';
    spinner.className = 'jf-infinite-spinner hidden';
    spinner.style.display = 'none';
    spinner.innerHTML = '<div class="jf-spinner"></div>';
    container.appendChild(spinner);
    
    // Add sentinel
    const sentinel = document.createElement('div');
    sentinel.id = 'infinite-scroll-sentinel';
    sentinel.className = 'jf-infinite-sentinel';
    container.appendChild(sentinel);
}

// --- Library Explorer Page Renderer (with pagination) ---
async function renderLibrary(libId) {
    const session = API.token;
    currentLibraryId = libId;
    const page = $('page-library');
    
    // Show skeleton grid immediately for instant visual feedback
    const lib = libraries.find(l => (l.id || l.Id) === libId);
    const libName = lib ? (lib.name || lib.Name) : t('common.library');
    const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';
    const libIcon = contentType === 'Movies' ? '🎬' : contentType === 'TvShows' ? '📺' : contentType === 'Music' ? '🎵' : '📁';
    
    page.innerHTML = `
    <div class="jf-page">
        <div class="flex flex-col md:flex-row md:items-center justify-between gap-4 mb-6 pb-4 border-b border-white/10">
            <div>
                <div class="flex items-center gap-2">
                    <span class="text-2xl">${libIcon}</span>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
                </div>
                <p class="text-xs text-white/40 mt-1">${t('common.loading')}</p>
            </div>
        </div>
        <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4">
            ${Array.from({length: 21}, () => `
            <div class="jf-card">
                <div class="jf-poster">
                    <div class="jf-poster-skeleton"></div>
                </div>
                <div class="p-3">
                    <div class="h-3 w-3/4 bg-white/5 rounded animate-pulse mb-1"></div>
                    <div class="h-2.5 w-1/2 bg-white/5 rounded animate-pulse"></div>
                </div>
            </div>`).join('')}
        </div>
    </div>`;
    renderLetterRail(libId);

    const gen = startLibraryLoad(libId);
    try {
        // Fetch first page with pagination + total count, and the genres for the filter
        const [{ items, total }, filterData] = await Promise.all([
            fetchLibraryPage(0),
            API.getFilters().catch(() => ({ Genres: [] }))
        ]);
        if (gen !== libraryGen || API.token !== session) return;
        const genres = filterData.Genres || filterData.genres || [];

        page.innerHTML = `
        <div class="jf-page">
            <!-- Header bar with title, count, and actions -->
            <div class="flex flex-col md:flex-row md:items-center justify-between gap-4 mb-6 pb-4 border-b border-white/10">
                <div>
                    <div class="flex items-center gap-2">
                        <span class="text-2xl">${lib?.content_type === 'Movies' ? '🎬' : lib?.content_type === 'TvShows' ? '📺' : lib?.content_type === 'Music' ? '🎵' : '📁'}</span>
                        <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
                        ${scanRing(libId, libName, 'w-5 h-5')}
                    </div>
                    <p id="lib-count-label" class="text-xs text-white/40 mt-1">${t('library.item_count', { count: total })}</p>
                </div>

                <div class="flex flex-wrap items-center gap-2">
                    <!-- Sort selector, with a toggle that reverses its order -->
                    <div class="flex items-center gap-1">
                        <select id="lib-sort" class="jf-input !py-1.5 text-xs">
                            <option value="SortName,ProductionYear:Ascending">${t('library.sort_name')}</option>
                            <option value="PremiereDate,SortName,ProductionYear:Descending">${t('library.sort_release_date')}</option>
                            <option value="DateCreated,SortName,ProductionYear:Descending">${t('library.sort_date_added')}</option>
                            <option value="CommunityRating,SortName,ProductionYear:Descending">${t('library.sort_rating')}</option>
                        </select>
                        <button id="lib-sort-reverse" type="button" class="jf-btn-ghost self-stretch !py-1.5 !px-2 text-xs" aria-pressed="false" title="${escapeHtml(t('library.sort_reverse'))}" aria-label="${escapeHtml(t('library.sort_reverse'))}">
                            <svg class="w-4 h-4" fill="none" stroke="currentColor" stroke-width="2" viewBox="0 0 24 24" aria-hidden="true"><path stroke-linecap="round" stroke-linejoin="round" d="M3 7.5 7.5 3m0 0L12 7.5M7.5 3v13.5m13.5 0L16.5 21m0 0L12 16.5m4.5 4.5V7.5"/></svg>
                        </button>
                    </div>

                    <!-- Genre filter -->
                    ${genres.length > 0 ? `
                    <select id="lib-genre" class="jf-input !py-1.5 text-xs max-w-[140px]">
                        <option value="">${t('library.all_genres')}</option>
                        ${genres.map(g => `<option value="${escapeHtml(g)}">${escapeHtml(g)}</option>`).join('')}
                    </select>` : ''}

                    <!-- Scan trigger -->
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs" data-action="scan-library-page" data-id="${escapeHtml(libId)}" data-name="${escapeHtml(libName)}">
                        ${t('library.scan_button')}
                    </button>
                </div>
            </div>

            <!-- Media Grid Container -->
            <div id="lib-grid-container"></div>
        </div>`;
        $('lib-sort')?.addEventListener('change', () => reloadLibrary(libId));
        $('lib-sort-reverse')?.addEventListener('click', e => {
            const btn = e.currentTarget;
            btn.setAttribute('aria-pressed', btn.getAttribute('aria-pressed') === 'true' ? 'false' : 'true');
            reloadLibrary(libId);
        });
        $('lib-genre')?.addEventListener('change', () => reloadLibrary(libId));
        fillLibraryGrid(items, total);
    } catch (e) {
        if (gen !== libraryGen || API.token !== session) return;
        page.classList.remove('jf-with-rail');
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

// The library list's sort: the select's value, with the order flipped while the reverse toggle is pressed
function librarySort() {
    const [sortBy, sortOrder] = ($('lib-sort')?.value || 'SortName,ProductionYear:Ascending').split(':');
    const reversed = $('lib-sort-reverse')?.getAttribute('aria-pressed') === 'true';
    if (!reversed) return { sortBy, sortOrder };
    return { sortBy, sortOrder: sortOrder === 'Ascending' ? 'Descending' : 'Ascending' };
}

// The /Items query for the library list; paging is added per request
function libraryListQuery(libId) {
    const lib = libraries.find(l => (l.id || l.Id) === libId);
    const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';

    let includeItemTypes = undefined;
    if (contentType === 'TvShows') includeItemTypes = 'Series';
    else if (contentType === 'Music') includeItemTypes = 'MusicAlbum';
    else if (contentType === 'Movies') includeItemTypes = 'Movie';

    const { sortBy, sortOrder } = librarySort();
    return {
        LibraryId: libId,
        IncludeItemTypes: includeItemTypes,
        SortBy: sortBy,
        SortOrder: sortOrder,
        Fields: 'PrimaryImageAspectRatio',
        Genres: $('lib-genre')?.value || undefined
    };
}

// Starts a load: a new generation, and the observer of the previous load is dropped
function startLibraryLoad(libId) {
    libraryObserver?.disconnect();
    libraryObserver = null;
    libraryQuery = libraryListQuery(libId);
    libraryTotal = null;
    libraryRequested = new Set([0]); // the first page is requested by the load itself
    railAnchor = null;
    return ++libraryGen;
}

// One page of the current load, with the total record count
async function fetchLibraryPage(startIndex) {
    const data = await API.getItems({ ...libraryQuery, StartIndex: startIndex, Limit: LIBRARY_PAGE_SIZE });
    const items = data.Items || data.items || [];
    return { items, total: data.TotalRecordCount || data.total_record_count || items.length };
}

// Fills #lib-grid-container with one child per item: cards for the loaded items, placeholders for the rest
function fillLibraryGrid(items, total) {
    const container = $('lib-grid-container');
    if (!container) return;
    libraryTotal = total;
    if (total === 0) {
        container.innerHTML = `<div class="text-center py-20 text-white/40">${t('library.no_filter_results')}</div>`;
        return;
    }
    container.innerHTML = '<div id="lib-grid" class="jf-lib-grid"></div>';
    const grid = container.firstElementChild;

    const template = document.createElement('div');
    template.innerHTML = '<div class="jf-card"><div class="jf-poster"></div><div class="p-3"><p class="text-xs sm:text-sm">&nbsp;</p><div class="text-[11px] mt-0.5">&nbsp;</div></div></div>';
    const placeholder = template.firstElementChild;
    const frag = document.createDocumentFragment();
    for (let i = 0; i < total; i++) {
        if (i < items.length) {
            const card = createCardElement(items[i]);
            card.dataset.letter = railLetter(items[i]);
            frag.appendChild(card);
        } else {
            const ph = placeholder.cloneNode(true);
            ph.dataset.i = i;
            frag.appendChild(ph);
        }
    }
    grid.appendChild(frag);
    initLazyImages(grid);

    // A placeholder that comes near the viewport requests its page, once per load
    const gen = libraryGen;
    libraryObserver = new IntersectionObserver(entries => {
        if (gen !== libraryGen) return; // a newer load has replaced this observer
        for (const entry of entries) {
            if (!entry.isIntersecting) continue;
            const page = Math.floor(Number(entry.target.dataset.i) / LIBRARY_PAGE_SIZE);
            if (libraryRequested.has(page)) continue;
            libraryRequested.add(page);
            loadLibraryPage(gen, page);
        }
    }, { rootMargin: '200% 0px' });
    grid.querySelectorAll('[data-i]').forEach(ph => libraryObserver.observe(ph));
    scheduleRailUpdate();
}

// Puts the cards of one page in place of its placeholders
async function loadLibraryPage(gen, page) {
    try {
        const { items } = await fetchLibraryPage(page * LIBRARY_PAGE_SIZE);
        if (gen !== libraryGen) return;
        const grid = $('lib-grid');
        items.forEach((item, j) => {
            const placeholder = grid?.children[page * LIBRARY_PAGE_SIZE + j];
            if (!placeholder) return;
            const card = createCardElement(item);
            card.dataset.letter = railLetter(item);
            placeholder.replaceWith(card);
            libraryObserver.unobserve(placeholder);
            initLazyImages(card);
        });
        scheduleRailUpdate();
    } catch (e) {
        if (gen !== libraryGen) return;
        libraryRequested.delete(page);
        toast(t('library.load_failed', { message: e.message }), 'error');
    }
}

// A sort or genre change: a new load for the new query, shown from the top of the list
async function reloadLibrary(libId) {
    renderLetterRail(libId);

    window.scrollTo({ top: 0, behavior: 'instant' });
    const gen = startLibraryLoad(libId);
    const container = $('lib-grid-container');
    if (container) {
        container.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    }

    try {
        const { items, total } = await fetchLibraryPage(0);
        if (gen !== libraryGen) return;
        fillLibraryGrid(items, total);
        const countLabel = $('lib-count-label');
        if (countLabel) countLabel.textContent = t('library.item_count', { count: total });
    } catch (e) {
        if (gen !== libraryGen) return;
        toast(t('library.filter_failed', { message: e.message }), 'error');
    }
}

// Scrolls the list to the first item of a letter. Nothing is filtered.
async function jumpToLetter(libId, letter) {
    const click = ++letterClicks;
    const gen = libraryGen;
    if (libraryTotal === null || !$('lib-grid')) return;
    const query = libraryQuery;
    const n = libraryTotal;
    const desc = query.SortOrder === 'Descending';
    // The rail's first entry goes back to the top of the page, header and controls included; no request
    if (letter === (desc ? 'Z' : '#')) {
        railAnchor = null;
        window.scrollTo({ top: 0, behavior: 'instant' });
        scheduleRailUpdate();
        return;
    }
    const stale = () => click !== letterClicks || gen !== libraryGen;
    // Count of items whose name sorts at or after the bound; Limit 0 fetches no items
    const countFrom = async (bound) => {
        const data = await API.getItems({ ...query, NameStartsWithOrGreater: bound, Limit: 0 });
        return data.TotalRecordCount || data.total_record_count || 0;
    };
    try {
        let k = 0;
        if (letter !== '#') {
            const b = letter.toLowerCase();
            k = desc
                ? await countFrom(String.fromCharCode(b.charCodeAt(0) + 1))
                : n - await countFrom(b);
        } else if (desc) {
            k = await countFrom('a');
        }
        if (stale()) return;
        const grid = $('lib-grid');
        if (!grid) return;
        if (k < n) {
            grid.children[k].scrollIntoView({ block: 'start', behavior: 'instant' });
            // The clicked section stays lit while its row is the top row (that row often starts with the previous letter)
            railAnchor = { k, first: firstVisibleIndex(grid) };
            setRailLetter(letter);
            updateLetterRail();
        } else {
            railAnchor = null;
            window.scrollTo({ top: document.scrollingElement.scrollHeight, behavior: 'instant' });
            scheduleRailUpdate();
        }
    } catch (e) {
        if (stale()) return;
        toast(t('library.load_failed', { message: e.message }), 'error');
    }
}

// The letter rail (#lib-letter-rail in index.html) shows for the name sorts, its letters reversed for a descending order
function renderLetterRail(libId) {
    const { sortBy, sortOrder } = librarySort();
    $('page-library').classList.toggle('jf-with-rail', sortBy.startsWith('SortName'));
    const letters = ['#', ...'ABCDEFGHIJKLMNOPQRSTUVWXYZ'];
    if (sortOrder === 'Descending') letters.reverse();
    $('lib-letter-rail').innerHTML = letters.map(l =>
        `<button type="button" data-action="jump-letter" data-letter="${l}" data-id="${escapeHtml(libId)}">${l}</button>`).join('');
    railAnchor = null;
}

// An item's rail section: the server sorts by the lowercased name, so its first character decides
function railLetter(item) {
    const c = (item.SortName || item.Name || '').toLowerCase().charAt(0);
    if (c < 'a') return '#';
    if (c > 'z') return 'Z';
    return c.toUpperCase();
}

// Index of the first card whose bottom edge is below the header; cards are in row order, so a binary search works
function firstVisibleIndex(grid) {
    const top = $('main-navbar').getBoundingClientRect().bottom;
    const kids = grid.children;
    let lo = 0, hi = kids.length;
    while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (kids[mid].getBoundingClientRect().bottom > top) hi = mid;
        else lo = mid + 1;
    }
    return lo;
}

function setRailLetter(letter) {
    for (const b of $('lib-letter-rail').children) {
        if (b.dataset.letter === letter) b.setAttribute('aria-current', 'true');
        else b.removeAttribute('aria-current');
    }
}

// Lights the section of the first item at the top of the view, or after a jump the clicked one (see jumpToLetter)
function updateLetterRail() {
    const page = $('page-library');
    const grid = $('lib-grid');
    if (!grid || page.classList.contains('hidden') || !page.classList.contains('jf-with-rail')) return;
    let i = firstVisibleIndex(grid);
    if (railAnchor && i === railAnchor.first) i = railAnchor.k;
    else railAnchor = null;
    const letter = grid.children[i]?.dataset.letter;
    if (letter) setRailLetter(letter); // a placeholder keeps the highlight until its page is in
}

// At most one update per frame
function scheduleRailUpdate() {
    if (railFrame) return;
    railFrame = requestAnimationFrame(() => {
        railFrame = 0;
        updateLetterRail();
    });
}
window.addEventListener('scroll', scheduleRailUpdate, { passive: true });
window.addEventListener('resize', scheduleRailUpdate);

// --- Detail View Renderer (Movies, TV Series, Music) ---
async function renderDetail(itemId) {
    const session = API.token;
    currentDetailId = itemId;
    const page = $('page-detail');
    // Show skeleton for detail view
    page.innerHTML = `
    <div class="jf-page">
        <div class="flex flex-col lg:flex-row gap-8 mb-10">
            <div class="w-48 lg:w-56 flex-shrink-0 mx-auto lg:mx-0">
                <div class="jf-poster rounded-xl overflow-hidden"><div class="jf-poster-skeleton"></div></div>
            </div>
            <div class="flex-1 space-y-4">
                <div class="h-8 w-3/4 bg-white/5 rounded animate-pulse"></div>
                <div class="h-4 w-1/2 bg-white/5 rounded animate-pulse"></div>
                <div class="h-4 w-1/3 bg-white/5 rounded animate-pulse"></div>
                <div class="flex gap-2 mt-4">
                    <div class="h-10 w-28 bg-white/5 rounded animate-pulse"></div>
                    <div class="h-10 w-28 bg-white/5 rounded animate-pulse"></div>
                </div>
                <div class="space-y-2 mt-6">
                    <div class="h-3 w-full bg-white/5 rounded animate-pulse"></div>
                    <div class="h-3 w-full bg-white/5 rounded animate-pulse"></div>
                    <div class="h-3 w-2/3 bg-white/5 rounded animate-pulse"></div>
                </div>
            </div>
        </div>
    </div>`;

    try {
        const item = await API.getItem(itemId);
        if (API.token !== session) return;
        activeItem = item;
        const name = item.Name || item.name || '';
        const overview = item.Overview || item.overview || '';
        const year = item.ProductionYear || item.production_year || '';
        const type = item.Type || item.type || '';
        const runtime = item.RunTimeTicks || item.run_time_ticks;
        const genres = item.Genres || item.genres || [];
        const rating = item.CommunityRating || item.community_rating;
        const officialRating = item.OfficialRating || item.official_rating;
        const played = item.UserData?.Played || item.user_data?.played || false;
        const fav = item.UserData?.IsFavorite || item.user_data?.is_favorite || false;
        const people = item.People || item.people || [];
        const id = item.Id || item.id;

        const imgSrc = item.ImageTags?.Primary ? API.imageUrl(id, 'Primary', { maxWidth: 500, tag: item.ImageTags.Primary }) : '';
        const backdropSrc = backdropUrl(item, 1920);
        const canPlay = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video'].includes(type);
        const streamType = type === 'Audio' ? 'Audio' : 'Videos';
        const resumeTicks = streamType === 'Videos' ? resumePositionTicks(item) : 0;

        // Parallel fetch: Children/Seasons/Episodes AND Similar media
        // Previously these were sequential, causing a waterfall effect
        const isContainer = ['Series', 'Season', 'MusicAlbum', 'Folder'].includes(type);
        const [childrenData, similarData] = await Promise.all([
            isContainer ? API.getChildren(id).catch(() => ({ Items: [] })) : Promise.resolve({ Items: [] }),
            API.getSimilar(id).catch(() => ({ Items: [] }))
        ]);
        if (API.token !== session) return;

        // Children / Seasons / Episodes
        let seasonsOrEpisodesHtml = '';
        const children = childrenData.Items || childrenData.items || [];
        if (children.length > 0) {
            if (type === 'Series') {
                // Build seasons grid using DocumentFragment
                const seasonsDiv = document.createElement('div');
                seasonsDiv.className = 'mb-10';
                seasonsDiv.innerHTML = `<h2 class="text-xl font-bold mb-4 text-white">${t('detail.seasons_count', { count: children.length })}</h2>`;
                const grid = document.createElement('div');
                grid.className = 'grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-6 gap-4';
                const fragment = document.createDocumentFragment();
                children.forEach(s => {
                    const card = createCardElement(s);
                    if (card) fragment.appendChild(card);
                });
                grid.appendChild(fragment);
                seasonsDiv.appendChild(grid);
                seasonsOrEpisodesHtml = seasonsDiv.outerHTML;
            } else if (type === 'Season') {
                seasonsOrEpisodesHtml = `
                <div class="mb-10">
                    <h2 class="text-xl font-bold mb-4 text-white">${t('detail.episodes_count', { count: children.length })}</h2>
                    <div class="space-y-3">
                        ${children.map(ep => {
                            const epId = ep.Id || ep.id;
                            // epRawName goes into the onclick code, which translates the fallback itself
                            const epRawName = ep.Name || ep.name || '';
                            const epName = epRawName || t('detail.episode');
                            const epIndex = ep.IndexNumber || ep.index_number || '';
                            const epDesc = ep.Overview || ep.overview || '';
                            const epDur = formatRuntime(ep.RunTimeTicks || ep.run_time_ticks);
                            const epPlayed = ep.UserData?.Played || ep.user_data?.played || false;
                            const epThumb = ep.ImageTags?.Primary ? API.imageUrl(epId, 'Primary', { maxWidth: 400, tag: ep.ImageTags.Primary }) : '';

                            return `
                            <div class="jf-episode-card group" data-action="play" data-id="${escapeHtml(epId)}" data-stream-type="Videos" data-title="${escapeHtml(epRawName || t('detail.episode'))}">
                                <div class="relative w-full sm:w-48 h-28 rounded-lg overflow-hidden bg-white/5 flex-shrink-0">
                                    ${epThumb ? `<img src="${epThumb}" alt="${escapeHtml(epName)}" class="w-full h-full object-cover">` : ''}
                                    <div class="absolute inset-0 bg-black/40 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity">
                                        <div class="w-10 h-10 rounded-full bg-[#00a4dc] text-white flex items-center justify-center shadow-lg">
                                            <svg class="w-5 h-5 ml-0.5" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
                                        </div>
                                    </div>
                                    ${epPlayed ? `<span class="absolute top-2 right-2 w-5 h-5 rounded-full bg-[#00a4dc] text-white flex items-center justify-center text-[10px]">✓</span>` : ''}
                                </div>
                                <div class="flex-1 min-w-0">
                                    <div class="flex items-center gap-2 mb-1">
                                        ${epIndex !== '' ? `<span class="text-xs font-bold text-[#00a4dc]">${t('detail.episode_number', { number: epIndex })}</span>` : ''}
                                        <h3 class="text-sm font-bold text-white truncate group-hover:text-[#38bdf8] transition-colors">${escapeHtml(epName)}</h3>
                                        ${epDur ? `<span class="text-[11px] text-white/40 ml-auto flex-shrink-0">${epDur}</span>` : ''}
                                    </div>
                                    <p class="text-xs text-white/60 line-clamp-2 leading-relaxed">${escapeHtml(epDesc) || t('detail.no_overview')}</p>
                                </div>
                            </div>`;
                        }).join('')}
                    </div>
                </div>`;
            } else {
                seasonsOrEpisodesHtml = sectionRow(t('detail.titles'), children);
            }
        }

        // Similar media
        let similarHtml = '';
        const similar = similarData.Items || similarData.items || [];
        if (similar.length > 0) {
            similarHtml = sectionRow(t('detail.similar'), similar.slice(0, 7));
        }

        // Cast & Crew rail
        let castHtml = '';
        if (people.length > 0) {
            castHtml = `
            <div class="mb-10">
                <h2 class="text-xl font-bold mb-4 text-white">${t('detail.cast_crew')}</h2>
                <div class="jf-scroll-row pb-2">
                    ${people.slice(0, 15).map(p => {
                        const pId = p.Id || p.id;
                        const pName = p.Name || p.name || '';
                        const pRole = p.Role || p.role || p.Type || p.type || t('detail.actor');
                        const pImg = pId ? API.imageUrl(pId, 'Primary', { maxWidth: 200, tag: p.PrimaryImageTag }) : '';
                        return `
                        <div class="w-28 text-center flex-shrink-0 cursor-pointer group" ${pId ? `data-action="navigate" data-page="person" data-id="${escapeHtml(pId)}"` : ''}>
                            <div class="w-20 h-20 mx-auto rounded-full overflow-hidden bg-white/10 mb-2 border-2 border-transparent group-hover:border-[#00a4dc] transition-all">
                                <img src="${pImg}" alt="${escapeHtml(pName)}" class="w-full h-full object-cover" onerror="this.style.display='none';this.nextElementSibling.style.display='flex'">
                                <div class="hidden w-full h-full items-center justify-center text-xs font-bold text-white/40 bg-white/5" style="display:none">
                                    ${escapeHtml(pName[0] || 'P')}
                                </div>
                            </div>
                            <p class="text-xs font-semibold text-white truncate group-hover:text-[#38bdf8] transition-colors">${escapeHtml(pName)}</p>
                            <p class="text-[10px] text-white/40 truncate">${escapeHtml(pRole)}</p>
                        </div>`;
                    }).join('')}
                </div>
            </div>`;
        }

        // Breadcrumb above the title: series, then season, then episode number (episodes); series only for seasons.
        let breadcrumbHtml = '';
        if (type === 'Episode' || type === 'Season') {
            const crumbs = [];
            const crumbLink = (id, text) => `<a href="#detail/${escapeHtml(id)}" class="text-[#00a4dc] hover:text-[#38bdf8] hover:underline transition">${escapeHtml(text)}</a>`;
            if (item.SeriesId && item.SeriesName) crumbs.push(crumbLink(item.SeriesId, item.SeriesName));
            if (type === 'Episode') {
                if (item.SeasonId && (item.SeasonName || item.ParentIndexNumber != null)) {
                    crumbs.push(crumbLink(item.SeasonId, item.SeasonName || t('detail.season_number', { number: item.ParentIndexNumber })));
                }
                if (item.IndexNumber != null) {
                    crumbs.push(`<span class="text-white/70">${t('detail.episode_number', { number: item.IndexNumber })}</span>`);
                }
            }
            if (crumbs.length > 0) {
                breadcrumbHtml = `<nav aria-label="${escapeHtml(t('detail.breadcrumb'))}" class="flex flex-wrap items-center gap-x-2 gap-y-1 mb-2 text-sm sm:text-base font-semibold min-w-0 break-words">${crumbs.join('<span aria-hidden="true" class="text-white/40">›</span>')}</nav>`;
            }
        }

        page.innerHTML = `
        <div class="jf-page">
            <!-- Full Hero Backdrop Banner -->
            <div class="jf-detail-hero -mx-4 lg:-mx-8 -mt-6">
                ${backdropSrc ? `<img src="${backdropSrc}" class="w-full h-[460px] object-cover opacity-35" onerror="this.style.display='none'">` : ''}
                <div class="absolute bottom-0 left-0 right-0 p-6 sm:p-10 lg:p-12 z-10 flex flex-col md:flex-row gap-8 items-end">
                    <!-- Poster Box -->
                    <div class="w-44 sm:w-56 rounded-2xl overflow-hidden shadow-2xl border border-white/10 flex-shrink-0 hidden md:block bg-black/60">
                        ${imgSrc ? `<img src="${imgSrc}" alt="${escapeHtml(name)}" class="w-full h-auto object-cover" onerror="this.style.display='none'">` : ''}
                    </div>

                    <!-- Meta & Actions -->
                    <div class="flex-1 min-w-0">
                        <div class="flex flex-wrap items-center gap-2 mb-3">
                            <span class="jf-badge jf-badge-primary uppercase tracking-wider text-[11px]">${escapeHtml(type)}</span>
                            ${year ? `<span class="jf-badge text-[11px] font-bold">${year}</span>` : ''}
                            ${officialRating ? `<span class="jf-badge text-[11px] font-bold bg-white/15">${escapeHtml(officialRating)}</span>` : ''}
                            ${runtime ? `<span class="jf-badge text-[11px]">${formatRuntime(runtime)}</span>` : ''}
                            ${rating ? `<span class="jf-badge jf-badge-rating text-[11px]">★ ${rating.toFixed(1)}</span>` : ''}
                        </div>

                        ${breadcrumbHtml}
                        <h1 class="text-3xl sm:text-4xl lg:text-5xl font-black text-white tracking-tight mb-4 drop-shadow-lg">${escapeHtml(name)}</h1>

                        ${genres.length > 0 ? `
                        <div class="flex flex-wrap gap-1.5 mb-6">
                            ${genres.map(g => `<span class="text-[11px] font-medium px-2.5 py-0.5 rounded-full bg-white/10 text-white/80 border border-white/10">${escapeHtml(g)}</span>`).join('')}
                        </div>` : ''}

                        <!-- Action Bar -->
                        <div class="flex flex-wrap items-center gap-3">
                            ${canPlay ? `
                            <button class="jf-btn-primary !py-3 !px-7 text-sm font-bold shadow-xl" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(name)}"${resumeTicks > 0 ? ' data-resume="true"' : ''}>
                                <svg class="w-5 h-5" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
                                ${resumeTicks > 0 ? t('detail.resume') : t('common.play')}
                            </button>
                            ${resumeTicks > 0 ? `
                            <button class="jf-btn-ghost !py-3 !px-4 text-sm" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(name)}" data-from-start="true">
                                ${t('detail.play_from_beginning')}
                            </button>` : ''}` : ''}

                            <button class="jf-btn-ghost !py-3 !px-4 text-sm" title="${escapeHtml(t('detail.toggle_favorite'))}" data-action="toggle-favorite" data-id="${escapeHtml(id)}" data-favorite="${fav}">
                                <span class="${fav ? 'text-rose-500 font-bold' : 'text-white/60'} text-base">♥</span>
                                <span class="hidden sm:inline text-xs ml-1">${fav ? t('detail.favorite') : t('detail.add_favorite')}</span>
                            </button>

                            <button class="jf-btn-ghost !py-3 !px-4 text-xs font-semibold" data-action="toggle-played" data-id="${escapeHtml(id)}" data-played="${played}">
                                ${played ? `<span class="text-emerald-400">${t('detail.played')}</span>` : `<span class="text-white/60">${t('detail.unplayed')}</span>`}
                            </button>

                            ${API.isAdmin ? `
                            <button class="jf-btn-ghost !py-3 !px-3 text-xs" title="${escapeHtml(t('detail.refresh_metadata'))}" data-action="refresh-item" data-id="${escapeHtml(id)}">
                                🔄
                            </button>` : ''}
                        </div>
                    </div>
                </div>
            </div>

            <!-- Overview section -->
            ${overview ? `
            <div class="mb-10 max-w-4xl">
                <h2 class="text-lg font-bold mb-2 text-white">${t('detail.overview')}</h2>
                <p class="text-white/75 text-sm sm:text-base leading-relaxed">${escapeHtml(overview)}</p>
            </div>` : ''}

            <!-- Cast & Crew -->
            ${castHtml}

            <!-- Seasons / Episodes -->
            ${seasonsOrEpisodesHtml}

            <!-- Similar Titles -->
            ${similarHtml}
        </div>`;
    } catch (e) {
        if (API.token !== session) return;
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

// --- Person Detail View ---
async function renderPerson(personId) {
    const page = $('page-person');
    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    try {
        const [person, itemsData] = await Promise.all([
            API.getPerson(personId),
            API.getPersonItems(personId).catch(() => ({ Items: [] }))
        ]);
        const pName = person.Name || person.name || t('person.fallback_name');
        const pOverview = person.Overview || person.overview || '';
        const items = itemsData.Items || itemsData.items || [];
        const pImg = API.imageUrl(personId, 'Primary', { maxWidth: 400 });

        page.innerHTML = `
        <div class="jf-page">
            <div class="flex flex-col sm:flex-row gap-6 items-center sm:items-start mb-10 pb-6 border-b border-white/10">
                <div class="w-36 h-36 rounded-full overflow-hidden bg-white/10 flex-shrink-0 border-2 border-white/20 shadow-xl">
                    <img src="${pImg}" alt="${escapeHtml(pName)}" class="w-full h-full object-cover" onerror="this.style.display='none'">
                </div>
                <div>
                    <h1 class="text-3xl font-extrabold text-white mb-2">${escapeHtml(pName)}</h1>
                    ${pOverview ? `<p class="text-white/70 text-xs sm:text-sm max-w-2xl leading-relaxed">${escapeHtml(pOverview)}</p>` : ''}
                </div>
            </div>
            ${items.length > 0 ? `
            <div>
                <h2 class="text-xl font-bold text-white mb-4">${t('person.known_for', { count: items.length })}</h2>
                <div id="person-known-grid" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4"></div>
            </div>` : `<p class="text-white/40">${t('person.no_items')}</p>`}
        </div>`;
        // Populate KnownFor grid using DocumentFragment
        if (items.length > 0) {
            const gridEl = $('person-known-grid');
            if (gridEl) {
                const fragment = document.createDocumentFragment();
                items.forEach(it => {
                    const card = createCardElement(it);
                    if (card) fragment.appendChild(card);
                });
                gridEl.appendChild(fragment);
            }
            initLazyImages();
        }
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

// --- Search Page Renderer ---
async function renderSearch(term) {
    term = decodeURIComponent(term || '').trim();
    if ($('search-input')) $('search-input').value = term;
    const page = $('page-search');
    const showNoResults = () => {
        page.innerHTML = `<div class="text-center py-24 text-white/40 jf-page"><p class="text-lg">${t('search.no_results', { term: escapeHtml(term) })}</p></div>`;
    };
    // A blank term never reaches the server: /Search/Hints answers 400 to it.
    if (!term) {
        showNoResults();
        return;
    }
    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        const data = await API.search(term, 50);
        const hints = data.SearchHints || data.search_hints || [];
        if (!hints.length) {
            showNoResults();
            return;
        }
        const items = hints.map(h => ({
            Id: h.Id || h.id || h.ItemId,
            Name: h.Name || h.name,
            Type: h.Type || h.type,
            ProductionYear: h.ProductionYear || h.production_year,
            ...(h.PrimaryImageTag ? { ImageTags: { Primary: h.PrimaryImageTag } } : {})
        }));

        page.innerHTML = `
        <div class="jf-page">
            <h1 class="text-2xl font-bold mb-6 text-white">${t('search.results_for', { term: escapeHtml(term), count: items.length })}</h1>
            <div id="search-grid" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4"></div>
        </div>`;
        // Populate search grid using DocumentFragment
        const gridEl = $('search-grid');
        if (gridEl) {
            const fragment = document.createDocumentFragment();
            items.forEach(it => {
                const card = createCardElement(it);
                if (card) fragment.appendChild(card);
            });
            gridEl.appendChild(fragment);
        }
        initLazyImages();
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

let searchTimeout = null;

function doSearch(term) {
    if (term && term.trim()) {
        // Debounce: prevent rapid successive searches
        if (searchTimeout) clearTimeout(searchTimeout);
        searchTimeout = setTimeout(() => {
            navigate('search', encodeURIComponent(term.trim()));
            searchTimeout = null;
        }, 300);
    }
}

// HLS.js instance for adaptive streaming
let hlsInstance = null;
// The playback session in progress, or null when nothing is playing
let activePlayback = null;
// Promise of the last Stopped report; playItem waits for it before reading the resume point
let pendingStop = Promise.resolve();
// Incremented by each playItem call, so a call that finishes loading after a newer one gives up
let playRequest = 0;
// True while the seek bar is being dragged, so the timeline does not move the thumb
let seekDragging = false;

// Ticks to resume from. Items at or past 90 % of their runtime count as watched and start over.
function resumePositionTicks(item) {
    const pos = item?.UserData?.PlaybackPositionTicks || 0;
    if (!(pos > 0)) return 0;
    const runtime = item.RunTimeTicks || 0;
    if (runtime > 0 && pos >= runtime * 0.9) return 0;
    return pos;
}

// The resume point as m:ss under an hour and h:mm:ss from an hour on
function formatResumeTime(ticks) {
    const total = Math.floor(ticks / 1e7);
    const h = Math.floor(total / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = String(total % 60).padStart(2, '0');
    return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

// Asks whether to resume at ticks or start over. Resolves 'resume' or 'start', or '' when the menu is cancelled
// (Escape, a click outside) or already open. Escape keeps the old returnValue, so it is cleared before opening.
function askResumeOrStart(title, ticks) {
    const menu = $('resume-menu');
    if (!menu || menu.open) return Promise.resolve('');
    $('resume-menu-title').textContent = title;
    $('resume-menu-resume').textContent = t('player.resume_from', { time: formatResumeTime(ticks) });
    menu.returnValue = '';
    return new Promise(resolve => {
        menu.addEventListener('close', () => resolve(menu.returnValue), { once: true });
        menu.showModal();
        // The buttons fade in with the dialog (input.css transitions all their properties, visibility too), so
        // autofocus finds Resume still hidden and focuses the backdrop; focus Resume once the fade is done
        setTimeout(() => { if (menu.open) $('resume-menu-resume').focus(); }, 250);
    });
}

// Starts an item from a card or a Play button. A video with a resume point asks first whether to resume.
async function playWithResumeChoice(itemId, streamType = 'Videos', title = '') {
    if (streamType !== 'Videos') return playItem(itemId, streamType, title);
    // Read the point after the last Stopped report and uncached, so it includes the position just saved
    await pendingStop;
    let item = null;
    try {
        item = await API.get('/Items/' + itemId);
    } catch (e) {
        console.warn('Could not load item for resume point:', e);
    }
    const ticks = resumePositionTicks(item);
    if (ticks <= 0) return playItem(itemId, streamType, title);
    const choice = await askResumeOrStart(title || item?.Name || '', ticks);
    if (choice === 'resume') playItem(itemId, streamType, title);
    else if (choice === 'start') playItem(itemId, streamType, title, { fromStart: true });
}

// Stops the session that is still playing, if any. Returns the promise of its Stopped report.
function stopPlayback() {
    if (activePlayback) activePlayback.stop();
    return pendingStop;
}

// Loads the stream of a session from startTicks (item ticks) and attaches it to the video element
async function openStream(session, startTicks) {
    const video = $('video-player');
    const { itemId, streamType } = session;
    const load = ++session.loads;
    // This load stays current until the session stops or a newer load starts
    const current = () => activePlayback === session && session.loads === load;
    const startSeconds = startTicks / 1e7;
    // Capture the subtitle before the request so a change made during it sees this load's burn-in
    const requestedSubtitle = session.subtitleIndex;
    session.burnedIn = streamType === 'Videos' && isImageSubtitle(findStream(session, 'subtitle', requestedSubtitle)?.Codec);

    let url;
    let isHls = false;
    let playSessionId = null;
    let mediaSourceId = null;
    if (streamType === 'Videos') {
        // Use Universal endpoint to get the optimal playback URL
        try {
            const data = await API.getUniversalVideoUrl(itemId, {
                startTimeTicks: startTicks,
                mediaSourceId: session.mediaSourceId,
                audioStreamIndex: session.audioIndex,
                subtitleStreamIndex: session.subtitleIndex,
            });
            if (!current()) return;
            if (data && data.url) {
                // Add auth token to the stream URL so the video element can authenticate
                const sep = data.url.includes('?') ? '&' : '?';
                url = API.base + data.url + sep + 'api_key=' + encodeURIComponent(API.token);
                isHls = data.url.includes('.m3u8');
                // Determine playback method from the URL, not from SupportsDirectPlay flag
                // which may be stale from the database
                let method;
                if (isHls) {
                    method = t('player.hls_transcode');
                } else if (data.url.includes('/stream')) {
                    // Check if it's a static direct play or direct stream
                    method = data.url.includes('Static=true') ? t('player.direct_play') : t('player.direct_stream');
                } else {
                    method = t('player.stream');
                }
                $('player-subtitle').textContent = method;
                playSessionId = isHls ? data.PlaySessionId : null;
                mediaSourceId = data.MediaSources?.[0]?.Id || null;
            } else {
                // Fallback: direct stream
                url = API.streamUrl(itemId, streamType);
                $('player-subtitle').textContent = t('player.video_stream_fallback');
            }
        } catch (e) {
            if (!current()) return;
            console.warn('Universal endpoint failed, using direct stream:', e);
            url = API.streamUrl(itemId, streamType);
            $('player-subtitle').textContent = t('player.video_stream_fallback');
        }
    } else if (streamType === 'Audio') {
        url = API.streamUrl(itemId, streamType);
        $('player-subtitle').textContent = t('player.audio_stream');
    } else {
        url = API.streamUrl(itemId, streamType);
        $('player-subtitle').textContent = t('player.stream');
    }

    session.offset = isHls ? startSeconds : 0;
    session.startAt = startSeconds;
    session.isHls = isHls;
    session.playSessionId = playSessionId;
    session.mediaSourceId = mediaSourceId || session.mediaSourceId;
    session.burnedIn = isHls && isImageSubtitle(findStream(session, 'subtitle', requestedSubtitle)?.Codec);

    video.onerror = () => {
        if (!current()) return;
        console.error('Video playback error:', video.error);
        session.failed = true;
        const errMsg = video.error ? video.error.message : t('common.unknown_error');
        $('player-subtitle').textContent = t('common.error_message', { message: errMsg });
    };

    // Handle HLS streams with hls.js
    if (isHls && typeof Hls !== 'undefined' && Hls.isSupported()) {
        // The transcode already starts at startTicks, so its playlist starts at 0
        hlsInstance = new Hls({
            enableWorker: true,
            lowLatencyMode: false,
            startPosition: 0,
        });
        hlsInstance.loadSource(url);
        hlsInstance.attachMedia(video);
        hlsInstance.on(Hls.Events.MANIFEST_PARSED, () => {
            if (!current()) return;
            video.play().catch(e => console.warn('Autoplay block:', e));
        });
        // A transcode that starts after the last keyframe of copied video lists no time: nothing is left
        // to play, so end the playback as the stream's end does
        hlsInstance.on(Hls.Events.LEVEL_LOADED, (event, data) => { if (current() && session.offset > 0 && data.details.totalduration === 0) session.stop(); });
        hlsInstance.on(Hls.Events.ERROR, (event, data) => {
            if (!current()) return;
            console.error('HLS error:', data);
            if (data.fatal) {
                $('player-subtitle').textContent = t('player.hls_error');
                if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
                    hlsInstance.startLoad();
                } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
                    hlsInstance.recoverMediaError();
                } else {
                    // Playback that can't recover is reported as failed, which leaves the watched state alone
                    session.failed = true;
                    hlsInstance.destroy();
                }
            }
        });
    } else if (isHls && video.canPlayType('application/vnd.apple.mpegurl')) {
        // Safari native HLS support; its playlist also starts at startTicks, so it needs no seek
        video.src = url;
        video.addEventListener('loadedmetadata', () => {
            if (!current()) return;
            video.play().catch(e => console.warn('Autoplay block:', e));
        }, { once: true });
    } else {
        // Direct play/stream
        video.src = url;
        if (startSeconds > 0) {
            video.addEventListener('loadedmetadata', () => {
                if (current()) video.currentTime = startSeconds;
            }, { once: true });
        }
        video.play().catch(e => console.warn('Autoplay block:', e));
    }
    if (streamType === 'Videos') showTextSubtitle(session);
    updatePlayerControls(session);
}

// Shows the item time and the bar; the bar stays still while the thumb is being dragged
function updatePlayerControls(session) {
    const pos = session.position();
    const total = session.total();
    $('player-duration').textContent = formatDurationSeconds(total);
    // previewSeek owns the elapsed time and the bar during a drag
    if (seekDragging) return;
    $('player-cur-time').textContent = formatDurationSeconds(pos);
    $('player-seek').value = total > 0 ? Math.min(100, pos / total * 100) : 0;
}

// Moves within the transcoded range in place; any other HLS point restarts the transcode after a pause
function seekTo(session, seconds) {
    const video = $('video-player');
    const total = session.total();
    if (total > 0) seconds = Math.min(seconds, total);
    seconds = Math.max(0, seconds);
    // A waiting track change restarts at the new point instead
    if (session.tracksChanged) return scheduleRestart(session, seconds);
    if (!session.isHls) {
        video.currentTime = seconds;
        return;
    }
    const rel = seconds - session.offset;
    const seekable = video.seekable;
    const end = seekable.length ? seekable.end(seekable.length - 1) : 0;
    if (rel >= 0 && rel <= end) {
        clearTimeout(session.seekTimer);
        session.seekTimer = null;
        session.pendingSeek = null;
        video.currentTime = rel;
        return;
    }
    scheduleRestart(session, seconds);
}

// Waits 400 ms after the last seek, then restarts the transcode at that point
function scheduleRestart(session, seconds) {
    session.pendingSeek = seconds;
    updatePlayerControls(session);
    clearTimeout(session.seekTimer);
    session.seekTimer = setTimeout(() => { session.seekTimer = null; restartStream(session, seconds); }, 400);
}

// Starts the transcode again at seconds of item time, stopping the old transcode first
async function restartStream(session, seconds) {
    const total = session.total();
    // The last second holds nothing to transcode, so end the playback as the stream's end does
    // (pendingSeek makes the Stopped report say the end)
    if (total > 0 && seconds > total - 1) {
        session.pendingSeek = total;
        session.stop();
        return;
    }
    session.tracksChanged = false;
    session.pendingSeek = seconds;
    if (hlsInstance) {
        hlsInstance.destroy();
        hlsInstance = null;
    }
    if (session.isHls && session.playSessionId) API.stopActiveEncodings(session.playSessionId).catch(() => {});
    await openStream(session, Math.round(seconds * 1e7));
    // A newer seek keeps its own target
    if (activePlayback === session && session.pendingSeek === seconds && !session.seekTimer) session.pendingSeek = null;
}

// Seeks to a point on the bar once the drag ends
function seekVideo(percent) {
    seekDragging = false;
    if (!activePlayback) return;
    seekTo(activePlayback, percent / 100 * activePlayback.total());
}

// Skips relative to the current item time
function skipTime(deltaSeconds) {
    if (!activePlayback) return;
    seekTo(activePlayback, activePlayback.position() + deltaSeconds);
}

// Shows the elapsed time under the thumb while the bar is being dragged
function previewSeek(percent) {
    if (!activePlayback) return;
    seekDragging = true;
    $('player-cur-time').textContent = formatDurationSeconds(percent / 100 * activePlayback.total());
}

// Picks another audio track and restarts the transcode at the current point
function changeAudioTrack(value) {
    const session = activePlayback;
    if (!session) return;
    session.audioIndex = Number(value);
    session.tracksChanged = true;
    scheduleRestart(session, session.position());
}

// A text subtitle is added to the player; a picture one is burned in, so it restarts the transcode
function changeSubtitleTrack(value) {
    const session = activePlayback;
    if (!session) return;
    session.subtitleIndex = Number(value);
    if (session.burnedIn || isImageSubtitle(findStream(session, 'subtitle', session.subtitleIndex)?.Codec)) {
        clearTextSubtitle();
        session.tracksChanged = true;
        scheduleRestart(session, session.position());
    } else {
        showTextSubtitle(session);
    }
}

// CodecType comes lower case from the server: 'audio', 'subtitle' or 'video'
function isStreamType(s, type) {
    return (s.CodecType || '').toLowerCase() === type;
}

// The stream of this type with this index, if the session has one
function findStream(session, type, index) {
    return session.streams.find(s => s.Index === index && isStreamType(s, type));
}

// Picture subtitles are burned in by the transcode (the server's codec names)
function isImageSubtitle(codec) {
    const c = (codec || '').toLowerCase();
    return c.includes('pgs') || ['dvd_subtitle', 'dvdsub', 'vobsub', 'dvb_subtitle', 'dvbsub', 'xsub', 'sup'].includes(c);
}

// Removes the text subtitle track that showTextSubtitle added
function clearTextSubtitle() {
    $('video-player').querySelectorAll('track[data-player-subtitle]').forEach(track => track.remove());
}

// Adds the chosen text subtitle as WebVTT; without a label, hls.js leaves the track alone
function showTextSubtitle(session) {
    clearTextSubtitle();
    const stream = findStream(session, 'subtitle', session.subtitleIndex);
    if (!session.mediaSourceId || !stream || isImageSubtitle(stream.Codec)) return;
    const track = document.createElement('track');
    track.kind = 'subtitles';
    track.dataset.playerSubtitle = '1';
    // The route takes the start in ticks: the transcode's offset, or 0 for direct play
    track.src = API.base + '/Videos/' + session.itemId + '/' + encodeURIComponent(session.mediaSourceId) +
        '/Subtitles/' + stream.Index + '/' + Math.round(session.offset * 1e7) + '/Stream.vtt?api_key=' +
        encodeURIComponent(API.token);
    track.addEventListener('error', () => {
        if (activePlayback === session) $('player-subtitle').textContent = t('player.subtitles_error');
    });
    $('video-player').appendChild(track);
    track.track.mode = 'showing';
}

// Language name in the page's language; und, mis, mul and zxx name no language
function trackLanguageName(code) {
    if (!code || ['und', 'mis', 'mul', 'zxx'].includes(code.toLowerCase())) return '';
    let name = code;
    try {
        name = new Intl.DisplayNames([document.documentElement.lang || 'en'], { type: 'language' }).of(code) || code;
    } catch (e) {
        // Not a code Intl knows: the code stays as it is
    }
    return name.charAt(0).toUpperCase() + name.slice(1);
}

// The title, then each tag the title does not already contain (Jellyfin's DisplayTitle)
function trackTitle(title, tags) {
    const parts = tags.filter(Boolean);
    if (!title) return parts.join(' - ');
    const extra = parts.filter(tag => !title.toLowerCase().includes(tag.toLowerCase()));
    return [title, ...extra].join(' - ');
}

// Audio menu label: language, profile or codec, channel layout or count, then the flags
function audioTrackLabel(s) {
    const codec = (s.Codec || '').toLowerCase();
    const codecNames = { ac3: 'Dolby Digital', eac3: 'Dolby Digital+', dca: 'DTS' };
    const profile = s.Profile || '';
    const layout = s.ChannelLayout ? s.ChannelLayout.charAt(0).toUpperCase() + s.ChannelLayout.slice(1) : '';
    const tags = [
        trackLanguageName(s.Language),
        (profile && profile.toLowerCase() !== 'lc') ? profile : (codecNames[codec] || codec.toUpperCase()),
        layout || (s.Channels ? t('player.track_channels', { count: s.Channels }) : ''),
        s.IsDefault ? t('player.track_default') : '',
        s.IsExternal ? t('player.track_external') : '',
    ];
    return trackTitle(s.Title, tags) || t('player.track_undefined');
}

// Subtitle menu label: language, default and forced flags, codec name, then external
function subtitleTrackLabel(s) {
    const codec = (s.Codec || '').toLowerCase();
    const codecNames = { dvb_subtitle: 'DVBSUB', dvb_teletext: 'DVBTXT', dvd_subtitle: 'DVDSUB', hdmv_pgs_subtitle: 'PGSSUB' };
    const tags = [
        trackLanguageName(s.Language) || t('player.track_undefined'),
        s.IsDefault ? t('player.track_default') : '',
        s.IsForced ? t('player.track_forced') : '',
        codecNames[codec] || codec.toUpperCase(),
        s.IsExternal ? t('player.track_external') : '',
    ];
    return trackTitle(s.Title, tags) || t('player.track_undefined');
}

// Jellyfin's Default mode: external first, then forced in the audio's language, forced, default
function defaultSubtitleIndex(subs, chosenAudio) {
    const audioLang = (chosenAudio?.Language || '').toLowerCase();
    let best = null;
    let bestKey = 0;
    for (const s of subs) {
        const sameLanguage = (s.Language || '').toLowerCase() === audioLang;
        const key = (s.IsExternal ? 8 : 0) + (s.IsForced && sameLanguage ? 4 : 0) + (s.IsForced ? 2 : 0) + (s.IsDefault ? 1 : 0);
        if (key > bestKey) {
            best = s;
            bestKey = key;
        }
    }
    return best ? best.Index : -1;
}

// Fills the audio and subtitle menus from the session; null hides both
function renderTrackMenus(session) {
    const audio = session ? session.streams.filter(s => isStreamType(s, 'audio')) : [];
    const subs = session ? session.streams.filter(s => isStreamType(s, 'subtitle')) : [];
    $('player-audio').replaceChildren(...audio.map(s => new Option(audioTrackLabel(s), s.Index)));
    $('player-subs').replaceChildren(new Option(t('player.subtitles_off'), -1), ...subs.map(s => new Option(subtitleTrackLabel(s), s.Index)));
    $('player-audio').value = String(session?.audioIndex ?? '');
    $('player-subs').value = String(session?.subtitleIndex ?? '');
    $('player-audio-wrap').hidden = audio.length < 2;
    $('player-subs-wrap').hidden = subs.length < 1;
}

// --- Video Player (Cinema Mode) ---
async function playItem(itemId, streamType = 'Videos', title = '', { fromStart = false } = {}) {
    const video = $('video-player');
    const modal = $('player-modal');
    if (!video || !modal) return;
    const request = ++playRequest;

    // Report a session that is still playing as stopped before starting another
    stopPlayback();

    // Destroy any existing HLS instance
    if (hlsInstance) {
        hlsInstance.destroy();
        hlsInstance = null;
    }

    $('player-title').textContent = title || t('player.playback');
    $('player-subtitle').textContent = t('player.loading');
    // Clear the times and the bar so the previous item's values never show
    $('player-cur-time').textContent = '0:00';
    $('player-duration').textContent = '0:00';
    $('player-seek').value = 0;
    seekDragging = false;
    clearTextSubtitle();
    renderTrackMenus(null);

    // Show modal immediately while loading
    if (!modal.open) {
        if (typeof modal.showModal === 'function') modal.showModal();
        else modal.setAttribute('open', '');
    }

    // Read the resume point only after the Stopped report, so it includes the position just saved
    await pendingStop;
    // The item gives the resume point and PlaybackInfo the tracks; a failed PlaybackInfo leaves no menus
    const [item, info] = await Promise.all([
        API.get('/Items/' + itemId).catch(e => {
            console.warn('Could not load item for resume point:', e);
            return null;
        }),
        streamType === 'Videos'
            ? API.getPlaybackInfo(itemId).catch(e => {
                console.warn('Could not load playback info:', e);
                return null;
            })
            : null,
    ]);
    if (!modal.open || request !== playRequest) return; // closed or replaced while loading
    if (!title && item?.Name) $('player-title').textContent = item.Name;

    const startTicks = (streamType !== 'Videos' || fromStart) ? 0 : resumePositionTicks(item);
    const runtimeTicks = item?.RunTimeTicks || 0;

    // Track choices as Jellyfin's defaults pick them; with no PlaybackInfo there are none
    const source = info?.MediaSources?.[0];
    const streams = source?.MediaStreams || [];
    const audioStreams = streams.filter(s => isStreamType(s, 'audio'));
    const subStreams = streams.filter(s => isStreamType(s, 'subtitle'));
    const defaultAudio = source?.DefaultAudioStreamIndex;
    const chosenAudio = audioStreams.find(s => s.Index === defaultAudio) || audioStreams.find(s => s.IsDefault) || audioStreams[0] || null;
    let audioIndex = null;
    let subtitleIndex = null;
    if (source) {
        audioIndex = chosenAudio ? chosenAudio.Index : null;
        const defaultSub = source.DefaultSubtitleStreamIndex;
        subtitleIndex = (defaultSub === -1 || subStreams.some(s => s.Index === defaultSub))
            ? defaultSub
            : defaultSubtitleIndex(subStreams, chosenAudio);
    }

    // The session owns this playback: stop() reports the position once and tears the player down
    let stopped = false;
    const session = {
        itemId,
        streamType,
        runtime: runtimeTicks > 0 ? runtimeTicks / 1e7 : null,
        offset: 0,
        startAt: startTicks / 1e7,
        isHls: false,
        playSessionId: null,
        mediaSourceId: source?.Id || null,
        streams,
        audioIndex,
        subtitleIndex,
        burnedIn: false,
        tracksChanged: false,
        loads: 0,
        failed: false,
        pendingSeek: null,
        seekTimer: null,
        // Item time in seconds: the pending seek target, else where the stream is
        position() {
            if (this.pendingSeek !== null) return this.pendingSeek;
            const video = $('video-player');
            // Before metadata has loaded currentTime reads 0 even when resuming, so use the start position
            if (video.readyState < HTMLMediaElement.HAVE_METADATA) return this.startAt;
            return this.offset + video.currentTime;
        },
        // The item's runtime when known, else the stream's length
        total() {
            if (this.runtime) return this.runtime;
            const duration = $('video-player').duration;
            return Number.isFinite(duration) && duration > 0 ? this.offset + duration : 0;
        },
        // The track ids the playback reports carry; a null one is left out
        reportFields() {
            const fields = {};
            if (this.mediaSourceId !== null) fields.MediaSourceId = this.mediaSourceId;
            if (this.audioIndex !== null) fields.AudioStreamIndex = this.audioIndex;
            if (this.subtitleIndex !== null) fields.SubtitleStreamIndex = this.subtitleIndex;
            return fields;
        },
        stop() {
            if (stopped) return pendingStop;
            stopped = true;
            clearTimeout(session.seekTimer);
            const ticks = Math.round(session.position() * 1e7);
            if (playerReportInterval) {
                clearInterval(playerReportInterval);
                playerReportInterval = null;
            }
            if (activePlayback === session) activePlayback = null;
            // A failed playback leaves the watched state alone, as in Jellyfin
            const reported = API.reportStopped(itemId, ticks, { ...session.reportFields(), ...(session.failed ? { Failed: true } : {}) }).catch(e => console.warn('Reporting playback stop failed:', e));
            pendingStop = reported;
            // Once the Stopped report is in, refresh the open view so it shows the new resume point
            reported.then(() => {
                API.invalidateCache('/Items');
                API.invalidateCache('/Shows');
                if (currentPage === 'home') return renderHome();
                if (currentPage === 'detail') return renderDetail(currentDetailId);
            }).catch(e => console.warn('Refreshing views after playback failed:', e));
            video.pause();
            if (hlsInstance) {
                hlsInstance.destroy();
                hlsInstance = null;
            }
            if (session.isHls && session.playSessionId) API.stopActiveEncodings(session.playSessionId).catch(e => console.warn('Stopping the transcode failed:', e));
            video.removeAttribute('src');
            video.load();
            clearTextSubtitle();
            return reported;
        },
    };
    activePlayback = session;
    renderTrackMenus(session);

    // Periodic progress reporting
    if (playerReportInterval) clearInterval(playerReportInterval);
    playerReportInterval = setInterval(() => {
        if (!video.paused && !video.ended) {
            const ticks = Math.round(session.position() * 1e7);
            API.reportProgress(itemId, ticks, false, session.reportFields()).catch(() => {});
        }
    }, 8000);

    // Video events for custom UI
    video.ontimeupdate = () => {
        if (activePlayback === session) updatePlayerControls(session);
    };

    video.onplay = () => updatePlayIcon(true);
    video.onplaying = () => { session.failed = false; };
    video.onpause = () => updatePlayIcon(false);
    video.onvolumechange = () => {
        updateVolumeIcon();
        const slider = $('player-vol');
        if (slider) slider.value = video.volume;
    };

    video.onended = () => {
        if (activePlayback === session) session.stop();
    };

    await openStream(session, startTicks);
    if (activePlayback === session) API.reportPlaying(itemId, startTicks, session.reportFields()).catch(() => {});
}

function closePlayer() {
    const m = $('player-modal');
    if (m) {
        if (typeof m.close === 'function') m.close();
        else m.removeAttribute('open');
    }
}

function togglePlayPause() {
    const v = $('video-player');
    if (!v) return;
    if (v.paused) v.play();
    else v.pause();
}

function updatePlayIcon(isPlaying) {
    const icon = $('player-play-icon');
    if (icon) {
        icon.innerHTML = isPlaying
            ? '<path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/>'
            : '<path d="M8 5v14l11-7z"/>';
    }
}

function setVolume(val) {
    const v = $('video-player');
    if (v) {
        v.volume = parseFloat(val);
        updateVolumeIcon();
        // Sync slider with video volume
        const slider = $('player-vol');
        if (slider && parseFloat(slider.value) !== parseFloat(val)) {
            slider.value = val;
        }
    }
}

function toggleMute() {
    const v = $('video-player');
    if (v) {
        v.muted = !v.muted;
        updateVolumeIcon();
    }
}

function updateVolumeIcon() {
    const v = $('video-player');
    const icon = $('player-vol-icon');
    if (!v || !icon) return;
    if (v.muted || v.volume === 0) {
        // Muted: speaker with X
        icon.innerHTML = '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5L6 9H2v6h4l5 4V5z"/><line stroke-linecap="round" stroke-width="2" x1="23" y1="9" x2="17" y2="15"/><line stroke-linecap="round" stroke-width="2" x1="17" y1="9" x2="23" y2="15"/>';
    } else if (v.volume < 0.5) {
        // Low volume: speaker only
        icon.innerHTML = '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M11 5L6 9H2v6h4l5 4V5z"/>';
    } else {
        // High volume: speaker with waves
        icon.innerHTML = '<path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15.536 8.464a5 5 0 010 7.072M18.364 5.636a9 9 0 010 12.728M11 5L6 9H2v6h4l5 4V5z"/>';
    }
}

function setPlaybackRate(rate) {
    const v = $('video-player');
    if (v) v.playbackRate = parseFloat(rate);
}

function toggleFullscreen() {
    const video = $('video-player');
    if (!video) return;
    if (!document.fullscreenElement) {
        // Try video element first, then fallback to modal
        if (video.requestFullscreen) {
            video.requestFullscreen().catch(() => {
                // Fallback: try the modal dialog
                const m = $('player-modal');
                if (m && m.requestFullscreen) m.requestFullscreen().catch(() => {});
            });
        } else {
            // Older browsers
            const m = $('player-modal');
            if (m && m.requestFullscreen) m.requestFullscreen().catch(() => {});
        }
    } else {
        document.exitFullscreen().catch(() => {});
    }
}

// Global Keyboard Shortcuts
document.addEventListener('keydown', e => {
    if ($('player-modal')?.open) {
        if (e.code === 'Space') { e.preventDefault(); togglePlayPause(); }
        if (e.code === 'ArrowLeft') { e.preventDefault(); skipTime(-10); }
        if (e.code === 'ArrowRight') { e.preventDefault(); skipTime(30); }
        if (e.key === 'f' || e.key === 'F') { e.preventDefault(); toggleFullscreen(); }
        if (e.key === 'm' || e.key === 'M') { e.preventDefault(); toggleMute(); }
        if (e.key === 'Escape') { closePlayer(); }
    }
});

// --- Action Helpers (Favorites / Played) ---
async function toggleFavorite(id, current) {
    try {
        if (current) await API.unsetFavorite(id);
        else await API.setFavorite(id);
        toast(current ? t('detail.favorite_removed') : t('detail.favorite_added'));
        if (currentDetailId === id) renderDetail(id);
    } catch (e) {
        toast(e.message, 'error');
    }
}

async function togglePlayed(id, current) {
    try {
        if (current) await API.markUnplayed(id);
        else await API.markPlayed(id);
        // Marking changes the item's (and a show's episodes') watch data, so drop the cached lists before re-rendering
        API.invalidateCache('/Items');
        API.invalidateCache('/Shows');
        toast(current ? t('detail.marked_unplayed') : t('detail.marked_played'));
        if (currentDetailId === id) renderDetail(id);
    } catch (e) {
        toast(e.message, 'error');
    }
}

// --- OpenID Connect: helpers for the sign-in page and the profile ---
// The redirect codes of the OIDC flows, each mapped to its translation key. A code not listed here gets the
// generic message; the code itself never reaches t().
const OIDC_ERROR_KEYS = new Map([
    ['cancelled', 'oidc.error.cancelled'],
    ['state_mismatch', 'oidc.error.state_mismatch'],
    ['expired', 'oidc.error.expired'],
    ['disabled', 'oidc.error.disabled'],
    ['provider_error', 'oidc.error.provider_error'],
    ['invalid_token', 'oidc.error.invalid_token'],
    ['no_username', 'oidc.error.no_username'],
    ['not_allowed', 'oidc.error.not_allowed'],
    ['name_taken', 'oidc.error.name_taken'],
    ['name_taken_ask_admin', 'oidc.error.name_taken_ask_admin'],
    ['already_linked', 'oidc.error.already_linked'],
    ['account_disabled', 'oidc.error.account_disabled'],
    ['server_error', 'oidc.error.server_error'],
]);

// The code of an 'oidc-error/<code>' route, or null for any other route.
function oidcErrorCode(data) {
    const match = /^oidc-error\/(.*)$/.exec(data || '');
    return match ? match[1] : null;
}

function oidcErrorText(code) {
    return t(OIDC_ERROR_KEYS.get(code) || 'oidc.error.unknown');
}

// Sends the browser to the provider. Only http(s) links are followed; anything else is an error.
function goToOidcProvider(url) {
    if (!/^https?:\/\//i.test(url || '')) throw new Error(t('oidc.error.provider_error'));
    window.location.href = url;
}

function showLoginError(message) {
    $('login-error').textContent = message;
    show('login-error');
}

// Adds the provider button below the sign-in form when OIDC is on. A failed Info call counts as off.
async function loadOidcSignIn() {
    const info = await API.oidcInfo().catch(() => null);
    const box = $('oidc-sign-in');
    if (!info?.Enabled || !box) return;
    const label = (info.ButtonLabel || '').trim() || t('oidc.sign_in_default');
    box.innerHTML = `<button type="button" class="jf-btn-ghost w-full !py-3 !font-semibold text-sm mt-4" data-action="oidc-sign-in">${escapeHtml(label)}</button>`;
}

async function startOidcSignIn() {
    try {
        const { AuthorizationUrl } = await API.oidcStart();
        goToOidcProvider(AuthorizationUrl);
    } catch (err) {
        showLoginError(err.message);
    }
}

// The flow is single-use, so the browser's return to #login/oidc is completed once per page load.
let oidcCompleteStarted = false;

async function completeOidcSignIn() {
    if (oidcCompleteStarted) return;
    oidcCompleteStarted = true;
    const completed = await API.oidcComplete().then(() => true, () => false);
    if (!completed) {
        showLoginError(t('oidc.complete_failed'));
        history.replaceState(null, '', '#login');
        return;
    }
    await finishSignIn();
}

// Starts linking the signed-in account. The provider's callback returns to #settings.
async function startOidcLink() {
    try {
        const { AuthorizationUrl } = await API.oidcLink();
        goToOidcProvider(AuthorizationUrl);
    } catch (err) {
        toast(err.message, 'error');
    }
}

async function unlinkOidc(userId) {
    if (!confirm(t('oidc.unlink_confirm'))) return;
    try {
        await API.oidcUnlink(userId);
        toast(t('oidc.unlinked'));
        renderSettings();
    } catch (err) {
        toast(err.message, 'error');
    }
}

// The profile gets the result of a link as #settings/oidc-linked or #settings/oidc-error/<code>. It is toasted
// once, then the hash is cleared so that a reload does not repeat it.
function showOidcLinkResult(data) {
    const code = oidcErrorCode(data);
    if (code !== null) toast(oidcErrorText(code), 'error');
    else if (data === 'oidc-linked') toast(t('oidc.linked'));
    else return;
    history.replaceState(null, '', '#settings');
}

// The profile's "Single sign-on" card. Not linked: a link button, when OIDC is on. Linked: the provider login
// and Unlink, which is disabled with the reason when the server says CanUnlink is false.
function oidcCardHtml(enabled, account) {
    let body;
    if (account?.Linked) {
        const linkedAt = account.LinkedAt ? new Date(account.LinkedAt).toLocaleString() : '—';
        const canUnlink = Boolean(account.CanUnlink);
        const reason = account.HasPassword ? t('oidc.unlink_password_off') : t('oidc.unlink_needs_password');
        const linkedAs = t('oidc.linked_as', { name: account.ProviderUsername || '', date: linkedAt });
        body = `
            <p class="text-sm text-white/80">${escapeHtml(linkedAs)}</p>
            <div class="flex flex-wrap items-center gap-3 mt-4">
                <button type="button" class="jf-btn-ghost !py-2 !px-4 text-xs${canUnlink ? '' : ' !cursor-not-allowed !opacity-50 disabled:pointer-events-none'}" data-action="oidc-unlink" data-id="${escapeHtml(account.UserId)}"${canUnlink ? '' : ' disabled'}>${t('oidc.unlink')}</button>
                ${canUnlink ? '' : `<span class="text-xs text-white/40">${escapeHtml(reason)}</span>`}
            </div>`;
    } else {
        body = `
            <p class="text-sm text-white/60">${t('oidc.not_linked')}</p>
            ${enabled ? `<button type="button" class="jf-btn-primary !py-2 !px-4 text-xs mt-4" data-action="oidc-link">${t('oidc.link')}</button>` : ''}`;
    }
    return `
        <div class="jf-settings-card">
            <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                <span class="w-2 h-2 rounded-full bg-[#38bdf8]"></span>
                ${t('oidc.account_title')}
            </h3>
            ${body}
        </div>`;
}

// --- Login Page Renderer ---
let pendingSetup = null; // { username, code } of an imported user's one-time code; memory only
function renderLogin(data) {
    pendingSetup = null;
    html('page-login', `
    <div class="flex justify-center items-center min-h-[70vh] jf-page px-4">
        <div class="w-full max-w-sm p-8 rounded-2xl bg-[#131822] border border-white/10 shadow-2xl">
            <div class="text-center mb-8">
                <img src="/web/assets/logo.png" alt="Rustyfin" class="w-20 h-20 mx-auto rounded-2xl shadow-xl mb-3 p-1 bg-white/5 border border-white/10">
                <h2 class="text-2xl font-black text-white tracking-tight">${t('nav.sign_in')}</h2>
                <p class="text-white/40 text-xs mt-1">${t('login.subtitle')}</p>
            </div>
            <form id="login-form" onsubmit="handleLogin(event)" class="space-y-4">
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">${t('common.username')}</label>
                    <input type="text" id="login-user" class="jf-input w-full" placeholder="${escapeHtml(t('common.username'))}" required autofocus>
                </div>
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">${t('common.password')}</label>
                    <input type="password" id="login-pass" class="jf-input w-full" placeholder="${escapeHtml(t('common.password'))}">
                </div>
                <button type="submit" class="jf-btn-primary w-full !py-3 font-bold text-sm shadow-lg mt-2">
                    ${t('login.submit')}
                </button>
                <div id="login-error" class="text-red-400 text-xs text-center hidden p-2 rounded bg-red-500/10 border border-red-500/20"></div>
            </form>
            <div id="oidc-sign-in"></div>
        </div>
    </div>`);
    loadOidcSignIn();
    const oidcError = oidcErrorCode(data);
    if (data === 'oidc') completeOidcSignIn();
    else if (oidcError !== null) showLoginError(oidcErrorText(oidcError));
}

// The steps after any sign-in: password, setup code or OpenID Connect.
async function finishSignIn() {
    updateAuthUI();
    // Replace a socket or retry left from an earlier session, so the new token is used now
    stopScanProgressWebSocket();
    initScanProgressWebSocket();
    await loadSidebar();
    navigate('home');
    toast(t('login.welcome_back', { name: API.userName }));
}

async function handleLogin(e) {
    e.preventDefault();
    const username = $('login-user').value;
    const password = $('login-pass').value;
    try {
        await API.login(username, password);
    } catch (err) {
        if (err.setupRequired) {
            renderSetupPassword(username, password);
            return;
        }
        showLoginError(err.status === 403 ? t('oidc.password_sign_in_off') : err.message);
        return;
    }
    await finishSignIn();
}

// Shown when a sign-in answers "setup required" for an imported user. The code is kept in memory only.
function renderSetupPassword(username, code) {
    pendingSetup = { username, code };
    html('page-login', `
    <div class="flex justify-center items-center min-h-[70vh] jf-page px-4">
        <div class="w-full max-w-sm p-8 rounded-2xl bg-[#131822] border border-white/10 shadow-2xl">
            <div class="text-center mb-8">
                <img src="/web/assets/logo.png" alt="Rustyfin" class="w-20 h-20 mx-auto rounded-2xl shadow-xl mb-3 p-1 bg-white/5 border border-white/10">
                <h2 class="text-2xl font-black text-white tracking-tight">${t('jellyfin_import.setup_title')}</h2>
                <p class="text-white/40 text-xs mt-1">${t('jellyfin_import.setup_text', { name: escapeHtml(username) })}</p>
            </div>
            <form id="setup-form" onsubmit="handleSetupPassword(event)" class="space-y-4">
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">${t('jellyfin_import.new_password')}</label>
                    <input type="password" id="setup-pass" class="jf-input w-full" autocomplete="new-password" autofocus>
                </div>
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">${t('jellyfin_import.confirm_password')}</label>
                    <input type="password" id="setup-pass2" class="jf-input w-full" autocomplete="new-password">
                </div>
                <button type="submit" class="jf-btn-primary w-full !py-3 font-bold text-sm shadow-lg mt-2">
                    ${t('jellyfin_import.setup_submit')}
                </button>
                <div id="setup-error" class="text-red-400 text-xs text-center hidden p-2 rounded bg-red-500/10 border border-red-500/20"></div>
                <button type="button" id="setup-back" class="w-full text-xs text-white/50 hover:text-white transition py-2" onclick="pendingSetup = null; renderLogin()">${t('jellyfin_import.back')}</button>
            </form>
        </div>
    </div>`);
}

async function handleSetupPassword(e) {
    e.preventDefault();
    const pw = $('setup-pass').value;
    const pw2 = $('setup-pass2').value;
    const fail = message => { $('setup-error').textContent = message; show('setup-error'); };
    if (!pw) return fail(t('jellyfin_import.password_required'));
    if (pw !== pw2) return fail(t('jellyfin_import.password_mismatch'));
    if (!pendingSetup) { renderLogin(); return; }
    try {
        await API.loginWithSetupCode(pendingSetup.username, pendingSetup.code, pw);
        pendingSetup = null;
        await finishSignIn();
    } catch (err) {
        fail(err.status === 401 ? t('jellyfin_import.code_invalid') : err.message);
    }
}

function doLogout() {
    API.logout();
    navigate('home');
    toast(t('login.signed_out'));
}

// --- Quick Connect Modal Trigger ---
function showQuickConnectModal() {
    const dlg = $('qc-modal');
    if (!dlg) return;
    if (typeof dlg.showModal === 'function') dlg.showModal();
    else dlg.setAttribute('open', '');
}

async function authorizeQuickConnect() {
    const code = $('qc-code-input')?.value.trim();
    if (!code) {
        toast(t('quick_connect.enter_valid_code'), 'error');
        return;
    }
    try {
        await API.post('/QuickConnect/Authorize?code=' + encodeURIComponent(code));
        toast(t('quick_connect.authorized'));
        $('qc-modal')?.close();
    } catch (e) {
        toast(t('quick_connect.authorize_failed', { message: e.message }), 'error');
    }
}

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
                <form onsubmit="handleSaveConfig(event)" class="space-y-4">
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
                            <input id="cfg-tmdb" type="text" class="jf-input w-full" value="${escapeHtml(config.TmdbApiKey || config.tmdb_api_key || '')}" placeholder="${escapeHtml(t('common.optional'))}">
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">${t('settings.api_key', { service: 'OMDb' })}</label>
                            <input id="cfg-omdb" type="text" class="jf-input w-full" value="${escapeHtml(config.OmdbApiKey || config.omdb_api_key || '')}" placeholder="${escapeHtml(t('common.optional'))}">
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">${t('settings.api_key', { service: 'Fanart.tv' })}</label>
                            <input id="cfg-fanart" type="text" class="jf-input w-full" value="${escapeHtml(config.FanartApiKey || config.fanart_api_key || '')}" placeholder="${escapeHtml(t('common.optional'))}">
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

async function handleSaveConfig(e) {
    e.preventDefault();
    try {
        await API.updateConfig({
            ServerName: $('cfg-server-name')?.value || 'Rustyfin',
            PreferredMetadataLanguage: $('cfg-meta-lang')?.value || 'de',
            PublicPort: parseInt($('cfg-port')?.value) || 8096,
            TmdbApiKey: $('cfg-tmdb')?.value || null,
            OmdbApiKey: $('cfg-omdb')?.value || null,
            FanartApiKey: $('cfg-fanart')?.value || null,
            IsStartupWizardCompleted: true
        });
        toast(t('settings.server_config_updated'));
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- Admin Dashboard Renderer ---
// AccountStatus.UserId has hyphens and a user's Id may not, so account ids are compared without case and hyphens.
function oidcAccountKey(id) { return String(id).toLowerCase().replace(/-/g, ''); }

async function renderAdmin(tab) {
    const page = $('page-admin');
    if (!API.isAdmin) {
        page.innerHTML = `<div class="p-8 text-center text-yellow-400 jf-page">${t('admin.access_denied')}</div>`;
        return;
    }

    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    if (!(await API.recheckAdmin())) return;
    try {
        let libsError = null;
        const [users, libs, tasks, plugins, info, sessions, accounts] = await Promise.all([
            API.getUsers().catch(() => []),
            API.getLibraries().catch(err => { libsError = err; return []; }),
            API.getTasks().catch(() => []),
            API.getPlugins().catch(() => []),
            API.getFullInfo().catch(() => ({})),
            API.getSessions().catch(() => []),
            API.oidcAccounts().catch(() => [])
        ]);
        const accountById = new Map(accounts.map(a => [oidcAccountKey(a.UserId), a]));

        const tabs = [t('admin.overview'), t('admin.users'), t('admin.libraries'), t('admin.tasks'), t('admin.plugins'), t('admin.sessions'), t('admin.activity_log_tab'), t('admin.live_log'), t('oidc.admin_tab'), t('jellyfin_import.tab')];
        const tabIds = ['admin-overview', 'admin-users', 'admin-libs', 'admin-tasks', 'admin-plugins', 'admin-sessions', 'admin-activity', 'admin-livelog', 'admin-signin', 'admin-import'];

        page.innerHTML = `
        <div class="max-w-6xl mx-auto jf-page">
            <div class="flex items-center justify-between mb-6">
                <div>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white">${t('nav.server_dashboard')}</h1>
                    <p class="text-xs text-white/40 mt-1">${t('admin.subtitle', { name: escapeHtml(info.ServerName || 'Rustyfin') })}</p>
                </div>
                <div class="flex gap-2">
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs" data-action="scan-all">
                        ${t('admin.scan_all')}
                    </button>
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs text-amber-400 hover:text-amber-300" onclick="if(confirm(t('admin.restart_confirm')))API.restart().then(()=>toast(t('admin.restarting')))">
                        ${t('admin.restart')}
                    </button>
                </div>
            </div>

            <!-- Tab pills -->
            <div class="flex gap-2 mb-6 overflow-x-auto pb-2">
                ${tabs.map((label, i) => `
                    <button class="px-4 py-2 rounded-xl text-xs font-semibold whitespace-nowrap transition ${i === 0 ? 'bg-[#00a4dc] text-white shadow-lg' : 'bg-white/5 text-white/60 hover:bg-white/10'}"
                            data-action="admin-tab" data-tab="${escapeHtml(tabIds[i])}">${label}</button>
                `).join('')}
            </div>

            <!-- Tab 0: Overview -->
            <div id="admin-overview" class="space-y-6">
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4">
                    <div class="jf-settings-card text-center">
                        <p class="text-3xl font-black text-[#00a4dc]">${escapeHtml(info.ServerName || 'Rustyfin')}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">${t('nav.server')}</p>
                    </div>
                    <div class="jf-settings-card text-center">
                        <p class="text-3xl font-black text-white">v${escapeHtml(info.RustyfinVersion || info.Version || serverVersion)}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">${t('admin.release_version')}</p>
                    </div>
                    <div class="jf-settings-card text-center">
                        <p class="text-3xl font-black text-emerald-400">${users.length}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">${t('admin.registered_users')}</p>
                    </div>
                    <div class="jf-settings-card text-center">
                        <p class="text-3xl font-black text-amber-400">${libsError ? '—' : libs.length}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">${t('admin.libraries')}</p>
                    </div>
                </div>

                <div class="jf-settings-card">
                    <h3 class="font-bold text-white text-base mb-4">${t('admin.system_details')}</h3>
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs">
                        <div class="flex justify-between py-2 border-b border-white/5"><span class="text-white/50">${t('admin.operating_system')}</span> <span class="font-mono text-white">${escapeHtml(info.OperatingSystem || 'Linux')}</span></div>
                        <div class="flex justify-between py-2 border-b border-white/5"><span class="text-white/50">${t('admin.architecture')}</span> <span class="font-mono text-white">${escapeHtml(info.SystemArchitecture || 'x86_64')}</span></div>
                        <div class="flex justify-between py-2 border-b border-white/5"><span class="text-white/50">${t('admin.program_data')}</span> <span class="font-mono text-white truncate max-w-xs">${escapeHtml(info.ProgramDataPath || '/data')}</span></div>
                        <div class="flex justify-between py-2 border-b border-white/5"><span class="text-white/50">${t('admin.log_path')}</span> <span class="font-mono text-white truncate max-w-xs">${escapeHtml(info.LogPath || '/data/logs')}</span></div>
                    </div>
                </div>
            </div>

            <!-- Tab 1: Users -->
            <div id="admin-users" class="hidden jf-settings-card">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="font-bold text-white text-base">${t('admin.user_accounts')}</h3>
                    <button class="jf-btn-primary text-xs" onclick="openAddUser()">${t('admin.new_user')}</button>
                </div>
                <div class="overflow-x-auto">
                    <table class="w-full text-left text-xs">
                        <thead>
                            <tr class="text-white/40 border-b border-white/10 pb-2">
                                <th class="pb-3 font-semibold">${t('common.name')}</th>
                                <th class="pb-3 font-semibold">${t('admin.role')}</th>
                                <th class="pb-3 font-semibold">${escapeHtml(t('admin.disabled_column'))}</th>
                                <th class="pb-3 font-semibold">${t('admin.last_login')}</th>
                                <th class="pb-3 font-semibold">${escapeHtml(t('oidc.provider_column'))}</th>
                                <th class="pb-3 font-semibold text-right">${t('admin.action')}</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-white/5">
                            ${users.map(u => {
                                const name = u.Name || u.name;
                                const isAdmin = u.IsAdministrator ?? u.Policy?.IsAdministrator ?? u.policy?.is_administrator;
                                const account = accountById.get(oidcAccountKey(u.Id || u.id));
                                const id = u.Id || u.id;
                                const isOwner = u.IsOwner === true;
                                const isSelf = oidcAccountKey(id) === oidcAccountKey(API.userId);
                                const locked = isOwner || isSelf;
                                const promoteBlocked = !locked && !isAdmin && u.IsDisabled === true;
                                const disableBlocked = !locked && isAdmin && u.IsDisabled !== true;
                                return `
                            <tr>
                                <td class="py-3 font-bold text-white">${escapeHtml(name)}</td>
                                <td class="py-3"><label class="inline-flex items-center gap-2 text-white/80"><input type="checkbox" data-action="set-user-admin" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}" aria-label="${escapeHtml(t('admin.administrator_for', { name }))}" ${isAdmin ? 'checked' : ''} ${locked || promoteBlocked ? 'disabled' : ''} class="w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]"><span>${escapeHtml(t('admin.role_admin'))}</span></label>${isOwner ? `<span class="ml-1.5 px-2 py-0.5 rounded text-[10px] font-bold bg-[#00a4dc]/20 text-[#38bdf8]">${escapeHtml(t('admin.role_owner'))}</span>` : ''}${account?.AdminFromGroup ? `<span class="ml-1.5 text-[10px] text-white/40">${escapeHtml(t('oidc.from_group'))}</span><p class="text-[10px] text-white/40 mt-1">${escapeHtml(t('admin.group_admin_hint'))}</p>` : ''}${locked ? `<p class="text-[10px] text-white/40 mt-1">${escapeHtml(t(isOwner ? 'admin.owner_locked' : 'admin.self_locked'))}</p>` : ''}${promoteBlocked ? `<p class="text-[10px] text-white/40 mt-1">${escapeHtml(t('admin.promote_disabled_locked'))}</p>` : ''}</td>
                                <td class="py-3"><input type="checkbox" data-action="set-user-disabled" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}" aria-label="${escapeHtml(t('admin.disabled_for', { name }))}" ${u.IsDisabled ? 'checked' : ''} ${locked || disableBlocked ? 'disabled' : ''} class="w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]">${disableBlocked ? `<p class="text-[10px] text-white/40 mt-1">${escapeHtml(t('admin.disable_admin_locked'))}</p>` : ''}</td>
                                <td class="py-3 text-white/40">${escapeHtml(u.LastLoginDate || u.last_login_date || '–')}</td>
                                <td class="py-3 text-white/60">${account?.Linked ? `${escapeHtml(account.ProviderUsername || '—')} <button type="button" class="ml-2 text-[#00a4dc] hover:text-[#38bdf8] font-semibold" data-action="oidc-remove-link" data-id="${escapeHtml(account.UserId)}" data-name="${escapeHtml(name)}">${escapeHtml(t('oidc.remove_link'))}</button>` : '<span class="text-white/40">—</span>'}</td>
                                <td class="py-3 text-right">
                                    <button class="text-red-400 hover:text-red-300 font-semibold" data-action="delete-user" data-id="${escapeHtml(u.Id || u.id)}">${t('common.delete')}</button>
                                </td>
                            </tr>`;
                            }).join('')}
                        </tbody>
                    </table>
                </div>
            </div>

            <!-- Tab 2: Libraries -->
            <div id="admin-libs" class="hidden jf-settings-card">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="font-bold text-white text-base">${t('admin.libraries_count', { number: libsError ? '—' : libs.length })}</h3>
                    <button class="jf-btn-primary text-xs" onclick="openAddLibrary()">${t('admin.new_library')}</button>
                </div>
                ${libsError ? `<p class="text-xs text-red-400">${t('library.list_failed', { message: escapeHtml(libsError.message) })}</p>` : `
                <div class="overflow-x-auto">
                    <table class="w-full text-left text-xs">
                        <thead>
                            <tr class="text-white/40 border-b border-white/10">
                                <th class="pb-3 font-semibold">${t('common.name')}</th>
                                <th class="pb-3 font-semibold">${t('common.type')}</th>
                                <th class="pb-3 font-semibold">${t('admin.paths')}</th>
                                <th class="pb-3 font-semibold">${t('admin.realtime_monitor')}</th>
                                <th class="pb-3 font-semibold text-right">${t('admin.actions')}</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-white/5">
                            ${libs.map(l => `
                            <tr>
                                <td class="py-3 font-bold text-white"><span class="inline-flex items-center gap-1.5">${escapeHtml(l.name || l.Name)}${scanRing(l.id || l.Id, l.name || l.Name)}</span></td>
                                <td class="py-3"><span class="px-2 py-0.5 rounded bg-white/10 text-white/70">${escapeHtml(l.content_type || l.ContentType || t('admin.mixed'))}</span></td>
                                <td class="py-3 text-white/40 font-mono">${escapeHtml((l.paths || l.Paths || []).join(', '))}</td>
                                <td class="py-3"><input type="checkbox" data-action="toggle-realtime-monitor" data-id="${escapeHtml(l.id || l.Id)}" aria-label="${escapeHtml(t('admin.realtime_monitor_for', { name: l.name || l.Name }))}" ${(l.LibraryOptions || {}).EnableRealtimeMonitor === false ? '' : 'checked'} class="w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]"></td>
                                <td class="py-3 text-right space-x-2">
                                    <button class="text-[#00a4dc] hover:text-[#38bdf8]" data-action="scan-library-admin" data-id="${escapeHtml(l.id || l.Id)}">${t('admin.scan')}</button>
                                    <button class="text-red-400 hover:text-red-300" data-action="delete-library" data-id="${escapeHtml(l.id || l.Id)}">${t('common.delete')}</button>
                                </td>
                            </tr>`).join('')}
                        </tbody>
                    </table>
                </div>`}
            </div>

            <!-- Tab 3: Tasks -->
            <div id="admin-tasks" class="hidden">
                <div class="flex items-center justify-between mb-4">
                    <h3 class="font-bold text-white text-base">${t('admin.background_tasks')}</h3>
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs" onclick="refreshTasks()">${t('admin.refresh')}</button>
                </div>
                <div id="admin-tasks-list" class="grid gap-3">
                    ${tasks.map(task => renderTaskCard(task)).join('')}
                </div>
            </div>

            <!-- Tab 4: Plugins -->
            <div id="admin-plugins" class="hidden jf-settings-card">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="font-bold text-white text-base">${t('admin.plugin_list')}</h3>
                </div>
                ${plugins.length === 0 ? `<p class="text-xs text-white/40">${t('admin.plugin_no_plugins')}</p>` :
                `<div class="space-y-3">
                    ${plugins.map(p => {
                        const statusClass = p.Status === 'Active' ? 'bg-emerald-500/20 text-emerald-400' : p.Status === 'Disabled' ? 'bg-red-500/20 text-red-400' : 'bg-amber-500/20 text-amber-400';
                        const statusLabel = p.Status === 'Active' ? t('admin.plugin_status_active') : p.Status === 'Disabled' ? t('admin.plugin_status_disabled') : t('admin.plugin_status_malfunctioned');
                        const typeLabel = p.PluginType === 'MetadataProvider' ? t('admin.plugin_type_metadata') : p.PluginType === 'ImageProvider' ? t('admin.plugin_type_image') : p.PluginType === 'SubtitleProvider' ? t('admin.plugin_type_subtitle') : p.PluginType === 'NotificationProvider' ? t('admin.plugin_type_notification') : p.PluginType === 'AuthenticationProvider' ? t('admin.plugin_type_auth') : p.PluginType === 'Task' ? t('admin.plugin_type_task') : (p.PluginType || '—');
                        return `
                    <div class="rounded-xl border border-white/10 bg-white/5 p-4">
                        <div class="flex items-start justify-between gap-4">
                            <div class="flex-1 min-w-0">
                                <div class="flex items-center gap-2 flex-wrap">
                                    <h4 class="font-bold text-white">${escapeHtml(p.Name)}</h4>
                                    <span class="px-2 py-0.5 rounded text-[10px] font-semibold ${statusClass}">${escapeHtml(statusLabel)}</span>
                                    ${p.IsBuiltin ? `<span class="px-2 py-0.5 rounded text-[10px] font-semibold bg-blue-500/20 text-blue-400">${t('admin.plugin_builtin')}</span>` : ''}
                                </div>
                                <p class="text-xs text-white/40 mt-1">${escapeHtml(p.Description || '')}</p>
                                <div class="flex gap-4 mt-2 text-[11px] text-white/30">
                                    <span>${t('admin.plugin_version')}: ${escapeHtml(p.Version || '—')}</span>
                                    <span>${t('admin.plugin_type')}: ${escapeHtml(typeLabel)}</span>
                                    <span>${t('admin.plugin_priority')}: <span class="plugin-priority-display" data-plugin-id="${escapeHtml(p.Id)}">${escapeHtml(String(p.Priority || 100))}</span></span>
                                </div>
                            </div>
                            <div class="flex gap-2 shrink-0">
                                <button class="jf-btn-ghost !py-1.5 !px-3 text-xs" data-action="plugin-edit-priority" data-plugin-id="${escapeHtml(p.Id)}">${t('admin.plugin_priority')}</button>
                                <button class="jf-btn-ghost !py-1.5 !px-3 text-xs" data-action="plugin-edit-config" data-plugin-id="${escapeHtml(p.Id)}">${t('admin.plugin_config')}</button>
                            </div>
                        </div>
                    </div>`;
                    }).join('')}
                </div>`}
            </div>

            <!-- Tab 5: Sessions -->
            <div id="admin-sessions" class="hidden jf-settings-card">
                <h3 class="font-bold text-white text-base mb-4">${t('admin.active_sessions')}</h3>
                <div class="overflow-x-auto">
                    <table class="w-full text-left text-xs">
                        <thead>
                            <tr class="text-white/40 border-b border-white/10">
                                <th class="pb-3 font-semibold">${t('admin.user')}</th>
                                <th class="pb-3 font-semibold">${t('admin.device')}</th>
                                <th class="pb-3 font-semibold">${t('admin.client')}</th>
                                <th class="pb-3 font-semibold">${t('admin.activity')}</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-white/5">
                            ${sessions.map(s => `
                            <tr>
                                <td class="py-3 font-bold text-white">${escapeHtml(s.UserName || s.user_name || '–')}</td>
                                <td class="py-3 text-white/70">${escapeHtml(s.DeviceName || s.device_name || t('admin.browser'))}</td>
                                <td class="py-3 text-white/50">${escapeHtml(s.Client || s.client || 'Rustyfin Web')}</td>
                                <td class="py-3 text-white/40">${escapeHtml(s.LastActivityDate || s.last_activity_date || '–')}</td>
                            </tr>`).join('')}
                        </tbody>
                    </table>
                </div>
            </div>

            <!-- Tab 5: Activity Log -->
            <div id="admin-activity" class="hidden jf-settings-card">
                <h3 class="font-bold text-white text-base mb-4">${t('admin.activity_log')}</h3>
                <div id="activity-content"><p class="text-white/40 text-xs">${t('admin.loading_activity')}</p></div>
            </div>

            <!-- Tab 6: Live Log -->
            <div id="admin-livelog" class="hidden">
                <div class="jf-settings-card">
                    <div class="flex items-center justify-between mb-4">
                        <h3 class="font-bold text-white text-base">${t('admin.live_log')}</h3>
                        <div class="flex items-center gap-3">
                            <span class="text-xs text-white/40">${t('admin.verbosity')}</span>
                            <select id="livelog-level" class="bg-white/10 text-xs text-white rounded-lg px-3 py-1.5 outline-none border border-white/10 cursor-pointer" onchange="setLogLevel(this.value)">
                                <option value="error">${t('admin.level_error')}</option>
                                <option value="warn">${t('admin.level_warn')}</option>
                                <option value="info" selected>${t('admin.level_info')}</option>
                                <option value="debug">${t('admin.level_debug')}</option>
                                <option value="trace">${t('admin.level_trace')}</option>
                            </select>
                            <button onclick="clearLiveLog()" class="text-xs text-white/40 hover:text-white transition px-2 py-1 rounded hover:bg-white/10">${t('admin.clear')}</button>
                            <div id="livelog-status" class="flex items-center gap-1.5 text-xs">
                                <span class="w-2 h-2 rounded-full bg-red-400"></span>
                                <span class="text-white/40">${t('admin.disconnected')}</span>
                            </div>
                        </div>
                    </div>
                    <div id="livelog-container" class="bg-black/50 rounded-xl border border-white/5 p-4 h-96 overflow-y-auto font-mono text-xs space-y-0.5">
                        <p class="text-white/30 italic">${t('admin.connecting_to_server')}</p>
                    </div>
                    <p class="text-[10px] text-white/20 mt-2">${t('admin.live_log_hint')}</p>
                </div>
            </div>

            <!-- Tab 7: Sign-in (OpenID Connect settings, filled by loadOidcAdminTab) -->
            <div id="admin-signin" class="hidden jf-settings-card"></div>

            ${jiPanelHtml()}
        </div>`;
        // #admin/import opens the import tab (the wizard's last step sends the admin there)
        if (tab === 'import') showAdminTab(page.querySelector('[data-tab="admin-import"]'), 'admin-import');
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

function showAdminTab(btn, tabId) {
    btn.parentElement.querySelectorAll('button').forEach(b => {
        b.className = 'px-4 py-2 rounded-xl text-xs font-semibold whitespace-nowrap transition bg-white/5 text-white/60 hover:bg-white/10';
    });
    btn.className = 'px-4 py-2 rounded-xl text-xs font-semibold whitespace-nowrap transition bg-[#00a4dc] text-white shadow-lg';

    ['admin-overview', 'admin-users', 'admin-libs', 'admin-tasks', 'admin-plugins', 'admin-sessions', 'admin-activity', 'admin-livelog', 'admin-signin', 'admin-import'].forEach(id => {
        const el = $(id);
        if (el) el.classList.toggle('hidden', id !== tabId);
    });

    if (tabId === 'admin-activity') loadActivityLog();
    if (tabId === 'admin-livelog') initLiveLog();
    if (tabId === 'admin-signin') loadOidcAdminTab();
    if (tabId === 'admin-import') loadJellyfinImport();
}

// An administrator changes another user's role or disabled state. The checkbox is disabled while the request runs, the users list cache is dropped, and the users tab is shown again after the page is rendered.
async function setUserPolicy(el, policy) {
    if (policy.IsDisabled && !confirm(t('admin.disable_user_confirm', { name: el.dataset.name }))) {
        el.checked = false;
        return;
    }
    el.disabled = true;
    try {
        await API.updatePolicy(el.dataset.id, policy);
        toast(t('admin.user_updated'));
    } catch (err) {
        toast(err.message, 'error');
    }
    API.invalidateCache('/Users');
    await renderAdmin();
    const tab = document.querySelector('[data-action="admin-tab"][data-tab="admin-users"]');
    if (tab) showAdminTab(tab, 'admin-users');
}

// An administrator removes another user's provider link. The users tab is shown again after the page is rendered.
async function removeOidcLinkAsAdmin(el) {
    if (!confirm(t('oidc.remove_link_confirm', { name: el.dataset.name }))) return;
    try {
        await API.oidcUnlink(el.dataset.id);
    } catch (err) {
        toast(err.message, 'error');
        return;
    }
    toast(t('oidc.unlinked'));
    await renderAdmin();
    const tab = document.querySelector('[data-action="admin-tab"][data-tab="admin-users"]');
    if (tab) showAdminTab(tab, 'admin-users');
}

// --- OpenID Connect settings (admin "Sign-in" tab) ---

async function loadOidcAdminTab() {
    let settings;
    try {
        settings = await API.oidcSettings();
    } catch {
        const box = $('admin-signin');
        if (box) box.innerHTML = `<p class="text-xs text-red-400">${escapeHtml(t('oidc.load_failed'))}</p>`;
        return;
    }
    renderOidcAdminSettings(settings);
}

// Renders the form from an OidcSettings object. The client secret field is always empty: a saved secret shows only in the placeholder.
function renderOidcAdminSettings(s) {
    const box = $('admin-signin');
    if (!box) return;
    const label = (id, key) => `<label for="${id}" class="block text-xs font-semibold text-white/70 mb-1.5">${escapeHtml(t(key))}</label>`;
    const hint = key => `<p class="text-xs text-white/40 mt-1">${escapeHtml(t(key))}</p>`;
    const warn = key => `<p class="text-xs text-amber-300/80 mt-1">${escapeHtml(t(key))}</p>`;
    const check = (id, key, checked = false) => `
            <div class="flex items-center gap-3">
                <input type="checkbox" id="${id}" class="w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]"${checked ? ' checked' : ''}>
                <label for="${id}" class="text-sm text-white/70">${escapeHtml(t(key))}</label>
            </div>`;
    const callbackRow = s.CallbackUrl
        ? `<div class="flex gap-2">
                <input id="oidc-callback-url" class="jf-input w-full" value="${escapeHtml(s.CallbackUrl)}" readonly>
                <button type="button" class="jf-btn-ghost !py-2 !px-4 text-xs whitespace-nowrap" data-action="oidc-copy-callback">${escapeHtml(t('oidc.copy'))}</button>
            </div>`
        : hint('oidc.callback_after_save');

    box.innerHTML = `
        <h3 class="font-bold text-white text-base mb-4">${escapeHtml(t('oidc.settings_title'))}</h3>
        <div class="space-y-4 max-w-2xl">
            ${check('oidc-enabled', 'oidc.enabled', s.Enabled)}
            <div>
                ${label('oidc-issuer-url', 'oidc.issuer_url')}
                <input id="oidc-issuer-url" type="text" class="jf-input w-full" value="${escapeHtml(s.IssuerUrl)}" spellcheck="false" autocomplete="off">
            </div>
            <div>
                ${label('oidc-client-id', 'oidc.client_id')}
                <input id="oidc-client-id" type="text" class="jf-input w-full" value="${escapeHtml(s.ClientId)}" spellcheck="false" autocomplete="off">
            </div>
            <div>
                ${label('oidc-client-secret', 'oidc.client_secret')}
                <input id="oidc-client-secret" type="password" class="jf-input w-full" autocomplete="new-password" placeholder="${escapeHtml(t(s.HasClientSecret ? 'oidc.secret_saved_placeholder' : 'oidc.secret_none_placeholder'))}">
                ${s.HasClientSecret ? `<div class="mt-2">${check('oidc-remove-secret', 'oidc.remove_secret')}</div>` : ''}
            </div>
            <div>
                ${label('oidc-scopes', 'oidc.scopes')}
                <input id="oidc-scopes" type="text" class="jf-input w-full" value="${escapeHtml(s.Scopes)}" spellcheck="false" autocomplete="off">
                ${hint('oidc.scopes_hint')}
                <div class="mt-3">
                    ${label('oidc-effective-scopes', 'oidc.effective_scopes')}
                    <input id="oidc-effective-scopes" type="text" class="jf-input w-full" value="${escapeHtml(s.EffectiveScopes)}" readonly>
                </div>
            </div>
            <div>
                ${label('oidc-button-label', 'oidc.button_label')}
                <input id="oidc-button-label" type="text" class="jf-input w-full" value="${escapeHtml(s.ButtonLabel)}" placeholder="${escapeHtml(t('oidc.sign_in_default'))}">
            </div>
            <div>
                ${label('oidc-username-claim', 'oidc.username_claim')}
                <input id="oidc-username-claim" type="text" class="jf-input w-full" value="${escapeHtml(s.UsernameClaim)}">
            </div>
            <div>
                ${label('oidc-group-claim', 'oidc.group_claim')}
                <input id="oidc-group-claim" type="text" class="jf-input w-full" value="${escapeHtml(s.GroupClaim)}">
            </div>
            <div>
                ${label('oidc-allowed-group', 'oidc.allowed_group')}
                <input id="oidc-allowed-group" type="text" class="jf-input w-full" value="${escapeHtml(s.AllowedGroup)}">
                ${hint('oidc.allowed_group_hint')}
            </div>
            <div>
                ${label('oidc-admin-group', 'oidc.admin_group')}
                <input id="oidc-admin-group" type="text" class="jf-input w-full" value="${escapeHtml(s.AdminGroup)}">
                ${hint('oidc.admin_group_hint')}
                ${warn('oidc.warn_groups')}
            </div>
            <div>
                ${label('oidc-public-url', 'oidc.public_url')}
                <input id="oidc-public-url" type="text" class="jf-input w-full" value="${escapeHtml(s.PublicUrl || location.origin)}">
            </div>
            <div>
                ${label('oidc-callback-url', 'oidc.callback_url')}
                ${callbackRow}
            </div>
            <div>
                ${check('oidc-allow-password', 'oidc.allow_password', s.AllowPasswordSignIn)}
                ${warn('oidc.warn_password_off')}
            </div>
            <div id="oidc-settings-error" class="hidden text-red-400 text-xs p-2 rounded bg-red-500/10 border border-red-500/20"></div>
            <div class="flex justify-end">
                <button type="button" class="jf-btn-primary text-xs" data-action="oidc-save-settings">${escapeHtml(t('oidc.save'))}</button>
            </div>
        </div>`;
}

// Copies the callback URL. navigator.clipboard is missing on plain-http LAN addresses, so the field is selected for execCommand.
async function copyOidcCallbackUrl() {
    const input = $('oidc-callback-url');
    if (!input) return;
    let copied = false;
    if (navigator.clipboard?.writeText) {
        copied = await navigator.clipboard.writeText(input.value).then(() => true, () => false);
    }
    if (!copied) {
        input.select();
        copied = document.execCommand('copy');
    }
    if (copied) toast(t('oidc.copied'));
    else toast(t('oidc.copy_failed'), 'error');
}

// The form stays as typed when the server refuses the save: the error text goes into the card, not over the form.
async function saveOidcAdminSettings() {
    const text = id => $(id).value;
    const body = {
        Enabled: $('oidc-enabled').checked,
        IssuerUrl: text('oidc-issuer-url'),
        ClientId: text('oidc-client-id'),
        Scopes: text('oidc-scopes'),
        ButtonLabel: text('oidc-button-label'),
        UsernameClaim: text('oidc-username-claim'),
        GroupClaim: text('oidc-group-claim'),
        AllowedGroup: text('oidc-allowed-group'),
        AdminGroup: text('oidc-admin-group'),
        PublicUrl: text('oidc-public-url'),
        AllowPasswordSignIn: $('oidc-allow-password').checked,
    };
    const secret = text('oidc-client-secret');
    if ($('oidc-remove-secret')?.checked) body.ClientSecret = '';
    else if (secret) body.ClientSecret = secret;

    hide('oidc-settings-error');
    let saved;
    try {
        saved = await API.saveOidcSettings(body);
    } catch (err) {
        $('oidc-settings-error').textContent = err.message;
        show('oidc-settings-error');
        return;
    }
    toast(t('oidc.settings_saved'));
    renderOidcAdminSettings(saved);
}

// --- Jellyfin import (B20) ---
// Admin tab: imports users, watch data and track choices from a Jellyfin data folder.
let jiPreviewed = false; // a preview is on screen; from then on the request carries the pairs and the checked users
let jiRunning = false;   // the last status read said Running
let jiTimer = null;      // the one status poll loop
const jiChoice = new Map();     // JellyfinId -> checked; kept across previews of the same folder and layout
const jiConfirm = new Map();    // JellyfinId -> UserId of a ticked suggested match; kept like jiChoice
let jiMine = '';                // JellyfinId picked for "Import into my account"; '' = no one
let jiPendingNames = new Map(); // UserId -> Name of the pending users (names stay out of onclick attributes)
const jiShownCodes = new Map(); // UserId -> { name, code } of a new code shown in the pending list
const jiLayoutKeys = { New: 'jellyfin_import.layout_new', Old: 'jellyfin_import.layout_old' };
const jiMetadataModeKeys = { FillEmpty: 'jellyfin_import.metadata_fill_empty', Replace: 'jellyfin_import.metadata_replace', None: 'jellyfin_import.metadata_none' };
const jiStageKeys = {
    Reading: 'jellyfin_import.stage_reading',
    Matching: 'jellyfin_import.stage_matching',
    Users: 'jellyfin_import.stage_users',
    WatchData: 'jellyfin_import.stage_watch_data',
    Metadata: 'jellyfin_import.stage_metadata',
    Scanning: 'jellyfin_import.stage_scanning'
};
const jiActionKeys = {
    Match: 'jellyfin_import.action_match',
    Create: 'jellyfin_import.action_create',
    Skip: 'jellyfin_import.action_skip'
};
const jiReasonKeys = {
    Ambiguous: 'jellyfin_import.reason_ambiguous',
    NotSelected: 'jellyfin_import.reason_not_selected',
    NotConfirmed: 'jellyfin_import.reason_not_confirmed',
    ConfirmedOtherAccount: 'jellyfin_import.reason_confirmed_other',
    AccountTakenByIntoMyAccount: 'jellyfin_import.reason_taken_by_mine'
};

function jiPanelHtml() {
    return `
    <div id="admin-import" class="hidden space-y-6">
        <div class="jf-settings-card">
            <h3 class="font-bold text-white text-base mb-2">${t('jellyfin_import.title')}</h3>
            <p class="text-xs text-white/60 mb-3">${t('jellyfin_import.help_intro')}</p>
            <p class="text-xs text-white/60 mb-2">${t('jellyfin_import.help_mount')}</p>
            <pre class="text-[11px] font-mono bg-black/50 border border-white/10 rounded-lg p-3 text-white/80 overflow-x-auto">-v /path/to/jellyfin/config:/import/jellyfin:ro</pre>
            <p class="text-xs text-white/60 mt-3">${t('jellyfin_import.help_path')}</p>
            <div class="grid gap-4 sm:grid-cols-2 mt-5">
                <label class="block">
                    <span class="block text-xs font-semibold text-white/70 mb-1.5">${t('jellyfin_import.folder')}</span>
                    <div class="path-field flex gap-2">
                        <input type="text" id="ji-source" class="jf-input flex-1 min-w-0" placeholder="/import/jellyfin" oninput="jiForget()" data-path-field list="path-suggestions" autocomplete="off" spellcheck="false">
                        <button type="button" class="jf-btn-ghost !px-3 !py-2 text-xs whitespace-nowrap" data-action="browse-path">${t('library.browse')}</button>
                    </div>
                </label>
                <label class="block">
                    <span class="block text-xs font-semibold text-white/70 mb-1.5">${t('jellyfin_import.layout')}</span>
                    <select id="ji-layout" class="jf-input w-full" onchange="jiForget()">
                        <option value="Auto">${t('jellyfin_import.layout_auto')}</option>
                        <option value="New">${t('jellyfin_import.layout_new')}</option>
                        <option value="Old">${t('jellyfin_import.layout_old')}</option>
                    </select>
                </label>
                <label class="block">
                    <span class="block text-xs font-semibold text-white/70 mb-1.5">${t('jellyfin_import.metadata')}</span>
                    <select id="ji-metadata" class="jf-input w-full">
                        <option value="FillEmpty">${t('jellyfin_import.metadata_fill_empty')}</option>
                        <option value="Replace">${t('jellyfin_import.metadata_replace')}</option>
                        <option value="None">${t('jellyfin_import.metadata_none')}</option>
                    </select>
                    <span class="block text-xs text-white/50 mt-1.5">${t('jellyfin_import.metadata_help')}</span>
                </label>
            </div>
            <button id="ji-preview-btn" class="jf-btn-primary !py-2 !px-4 text-sm mt-5" onclick="jiPreview()">${t('jellyfin_import.preview')}</button>
        </div>

        <div id="ji-error" class="hidden text-red-400 text-xs p-2 rounded bg-red-500/10 border border-red-500/20"></div>

        <div id="ji-preview" class="hidden jf-settings-card space-y-5">
            <p id="ji-found" class="text-xs text-white/70"></p>
            <div>
                <h4 class="font-semibold text-white text-sm">${t('jellyfin_import.pairs_title')}</h4>
                <p class="text-xs text-white/50 mb-3">${t('jellyfin_import.pairs_help')}</p>
                <div id="ji-pairs" class="space-y-2"></div>
                <button id="ji-add-pair" class="jf-btn-ghost !py-1.5 !px-3 text-xs mt-3" onclick="jiAddPair()">${t('jellyfin_import.add_pair')}</button>
                <div id="ji-unpaired" class="mt-3 space-y-1"></div>
            </div>
            <div>
                <h4 class="font-semibold text-white text-sm mb-2">${t('jellyfin_import.users_title')}</h4>
                <div id="ji-users" class="space-y-2"></div>
            </div>
            <div id="ji-counts" class="space-y-1 text-xs text-white/70"></div>
            <div id="ji-samples" class="space-y-1"></div>
            <button id="ji-run-btn" class="jf-btn-primary !py-2 !px-4 text-sm" onclick="jiRun()">${t('jellyfin_import.run')}</button>
        </div>

        <div id="ji-progress" class="hidden jf-settings-card space-y-3">
            <p id="ji-progress-text" class="text-sm text-white"></p>
            <div id="ji-progress-detail">
                <div class="h-2 rounded-full bg-white/10 overflow-hidden"><div id="ji-bar" class="h-2 bg-[#00a4dc]" style="width: 0%"></div></div>
                <p id="ji-progress-count" class="text-xs text-white/50 mt-2"></p>
            </div>
        </div>

        <div id="ji-result" class="hidden jf-settings-card space-y-4">
            <p id="ji-result-text" class="text-sm font-semibold text-white"></p>
            <div id="ji-result-report" class="space-y-2 text-xs text-white/70"></div>
            <div id="ji-codes-box" class="hidden rounded-xl border border-amber-400/40 bg-amber-500/10 p-4 space-y-3">
                <h4 class="font-bold text-amber-300 text-sm">${t('jellyfin_import.codes_title')}</h4>
                <p class="text-xs font-semibold text-amber-200">${t('jellyfin_import.codes_warning')}</p>
                <table id="ji-codes" class="w-full text-sm"></table>
                <button id="ji-copy-codes" class="jf-btn-primary !py-2 !px-4 text-sm" onclick="jiCopyCodes()">${t('jellyfin_import.copy_codes')}</button>
            </div>
            <p id="ji-former-admins" class="hidden text-xs text-amber-300"></p>
        </div>

        <div id="ji-pending" class="jf-settings-card">
            <h4 class="font-semibold text-white text-sm">${t('jellyfin_import.pending_title')}</h4>
            <p class="text-xs text-white/50 mb-3">${t('jellyfin_import.pending_help')}</p>
            <div id="ji-pending-list"></div>
        </div>
    </div>`;
}

function jiPairRowHtml(from, to) {
    return `
    <div class="ji-pair flex items-center gap-2">
        <input type="text" class="ji-from jf-input flex-1 font-mono text-xs" placeholder="${escapeHtml(t('jellyfin_import.pair_from'))}" value="${escapeHtml(from)}">
        <span class="text-white/40 text-xs">→</span>
        <input type="text" class="ji-to jf-input flex-1 font-mono text-xs" placeholder="${escapeHtml(t('jellyfin_import.pair_to'))}" value="${escapeHtml(to)}">
        <button type="button" class="ji-pair-remove jf-btn-ghost !py-1 !px-2 text-xs" title="${escapeHtml(t('jellyfin_import.remove_pair'))}" onclick="this.closest('.ji-pair').remove()">×</button>
    </div>`;
}

function jiAddPair() {
    $('ji-pairs').insertAdjacentHTML('beforeend', jiPairRowHtml('', ''));
}

// The pairs with both sides filled, trimmed.
function jiPairs() {
    return [...document.querySelectorAll('#ji-pairs .ji-pair')]
        .map(row => ({ From: row.querySelector('.ji-from').value.trim(), To: row.querySelector('.ji-to').value.trim() }))
        .filter(p => p.From && p.To);
}

// Preview and Run send the folder, layout and metadata mode; once a preview is on screen they also send the pairs, the checked users, the ticked matches and the "Import into my account" pick (IntoMyAccount).
function jiRequest() {
    const req = { SourcePath: $('ji-source').value.trim(), Layout: $('ji-layout').value, MetadataMode: $('ji-metadata').value };
    if (jiPreviewed) {
        req.PathMappings = jiPairs();
        req.UserIds = [...document.querySelectorAll('#ji-users .ji-user:checked:not(:disabled)')].map(el => el.value);
        req.ConfirmedMatches = [...document.querySelectorAll('#ji-users .ji-confirm:checked:not(:disabled)')]
            .map(el => ({ JellyfinId: el.value, UserId: el.dataset.userId }));
        if (jiMine) req.IntoMyAccount = jiMine;
    }
    return req;
}

function jiBusy(busy) {
    ['ji-preview-btn', 'ji-run-btn'].forEach(id => { const b = $(id); if (b) b.disabled = busy; });
}

function jiShowError(message) {
    $('ji-error').textContent = message;
    show('ji-error');
}

// A changed folder or layout forgets the preview, its pairs and the checkbox choices.
function jiForget() {
    jiPreviewed = false;
    jiChoice.clear();
    jiConfirm.clear();
    jiMine = '';
    $('ji-pairs').innerHTML = '';
    $('ji-users').innerHTML = '';
    hide('ji-preview');
}

// Same JellyfinId, ignoring hyphens and case (user ids come both ways). Empty ids never match.
function jiSameId(a, b) {
    return !!a && !!b && oidcAccountKey(a) === oidcAccountKey(b);
}

// A row's action as Run will see it. An unpicked row is skipped; the "Import into my account" row
// is Import into your account; a row whose name matches the caller's account is skipped while
// jiMine is set (that account takes the other user's data); a name match goes in only when ticked;
// other rows keep the last Preview's text.
function jiRowText(row) {
    const action = row.querySelector('.ji-action');
    const user = row.querySelector('.ji-user');
    const confirm = row.querySelector('.ji-confirm');
    if (confirm) confirm.disabled = jiSameId(user.value, jiMine);
    if (!user.checked) action.textContent = jiActionText({ Action: 'Skip', Reason: 'NotSelected' });
    else if (jiSameId(user.value, jiMine)) action.textContent = t('jellyfin_import.action_into_mine');
    else if (jiMine && confirm && jiSameId(confirm.dataset.userId, API.userId)) action.textContent = jiActionText({ Action: 'Skip', Reason: 'AccountTakenByIntoMyAccount' });
    else if (!confirm) action.textContent = action.dataset.text;
    else action.textContent = jiActionText(confirm.checked ? { Action: 'Match' } : { Action: 'Skip', Reason: 'NotConfirmed' });
}

// Re-runs the text only of rows the pick changes: the old pick's row, the new pick's row and rows
// whose confirm box names the caller. Other rows keep the server's text.
function jiRefreshMine(old) {
    document.querySelectorAll('#ji-users .ji-user').forEach(el => {
        const row = el.closest('div');
        const confirm = row.querySelector('.ji-confirm');
        if (jiSameId(el.value, old) || jiSameId(el.value, jiMine) || (confirm && jiSameId(confirm.dataset.userId, API.userId))) jiRowText(row);
    });
}

function jiSetMine(id) {
    const old = jiMine;
    jiMine = id;
    if (id) {
        const box = [...document.querySelectorAll('#ji-users .ji-user')].find(el => el.value === id);
        if (box) box.checked = true;
        jiChoice.set(id, true);
    }
    jiRefreshMine(old);
}

function jiSetChoice(el) {
    jiChoice.set(el.value, el.checked);
    if (!el.checked && jiSameId(el.value, jiMine)) {
        const old = jiMine;
        jiMine = '';
        $('ji-mine').value = '';
        jiRefreshMine(old);
    } else {
        jiRowText(el.closest('div'));
    }
}

function jiSetConfirm(el) {
    if (el.checked) jiConfirm.set(el.value, el.dataset.userId);
    else jiConfirm.delete(el.value);
    jiRowText(el.closest('div'));
}

async function jiPreview() {
    const btn = $('ji-preview-btn');
    btn.disabled = true;
    btn.textContent = t('jellyfin_import.previewing');
    hide('ji-error');
    try {
        jiRenderPreview(await API.jellyfinImportPreview(jiRequest()));
    } catch (err) {
        jiShowError(err.message);
    } finally {
        btn.disabled = jiRunning;
        btn.textContent = t('jellyfin_import.preview');
    }
}

function jiRenderPreview(r) {
    jiPreviewed = true;
    $('ji-found').innerHTML = t('jellyfin_import.found', { layout: t(jiLayoutKeys[r.Layout]), path: escapeHtml(r.DataPath) });
    $('ji-pairs').innerHTML = (r.PathMappings || []).map(m => jiPairRowHtml(m.From, m.To)).join('');
    $('ji-unpaired').innerHTML = jiUnpairedHtml(r);
    $('ji-users').innerHTML = jiUsersHtml(r.Users || []);
    $('ji-counts').innerHTML = jiCountsHtml(r);
    $('ji-samples').innerHTML = jiSamplesHtml(r);
    show('ji-preview');
}

// The "Import into my account" picker sits above the rows; its hint shows only when no selected row
// is name-matched to the caller's own account.
function jiUsersHtml(users) {
    if (!users.length) return '';
    const ownNameMatched = users.some(u => u.SuggestedMatch && jiSameId(u.SuggestedMatch.UserId, API.userId));
    const picker = `
    <div class="flex flex-wrap items-center gap-2 text-sm text-white">
        <label for="ji-mine" class="text-xs font-semibold text-white/70">${t('jellyfin_import.into_mine')}</label>
        <select id="ji-mine" class="jf-input text-xs" onchange="jiSetMine(this.value)">
            <option value="">${t('jellyfin_import.into_mine_none')}</option>
            ${users.filter(u => u.Reason !== 'Ambiguous').map(u => `<option value="${escapeHtml(u.JellyfinId)}" ${jiSameId(u.JellyfinId, jiMine) ? 'selected' : ''}>${escapeHtml(u.Name)}</option>`).join('')}
        </select>
    </div>${ownNameMatched ? '' : `
    <p class="text-xs text-white/50">${t('jellyfin_import.into_mine_hint')}</p>`}`;
    return picker + users.map(u => {
        const ambiguous = u.Reason === 'Ambiguous';
        const checked = !ambiguous && (jiChoice.has(u.JellyfinId) ? jiChoice.get(u.JellyfinId) : true);
        // The row's text without the choice: an "Import into my account" row without a name match is a Create.
        const plainText = u.MatchKind === 'IntoMyAccount' && !u.SuggestedMatch ? jiActionText({ Action: 'Create' }) : jiActionText(u);
        return `
        <div>
            <label class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-white">
                <input type="checkbox" class="ji-user" value="${escapeHtml(u.JellyfinId)}" onchange="jiSetChoice(this)" ${checked ? 'checked' : ''} ${ambiguous ? 'disabled' : ''}>
                <span class="font-semibold">${escapeHtml(u.Name)}</span>
                ${u.WasAdministrator ? `<span class="jf-badge text-[10px]">${t('jellyfin_import.was_admin')}</span>` : ''}
                <span class="ji-action text-xs text-white/50" data-text="${escapeHtml(plainText)}">${jiActionText(u)}</span>
            </label>${jiConfirmHtml(u)}
        </div>`;
    }).join('');
}

// The import page's one date style, with the time, escaped.
function jiDate(iso) {
    return escapeHtml(new Date(iso).toLocaleString(uiLocale(), { dateStyle: 'medium', timeStyle: 'short' }));
}

// A name match goes into the existing account only when its box is ticked. It starts unticked, and
// stays ticked only while the suggestion names the same account. Account names are escaped.
function jiConfirmHtml(u) {
    const s = u.SuggestedMatch;
    if (!s) return '';
    const checked = jiConfirm.get(u.JellyfinId) === s.UserId;
    const facts = [
        s.IsAdministrator ? t('jellyfin_import.match_admin') : '',
        t('jellyfin_import.match_created', { date: jiDate(s.CreatedAt) }),
        s.LastLoginDate ? t('jellyfin_import.match_last_login', { date: jiDate(s.LastLoginDate) }) : t('jellyfin_import.match_never_signed_in'),
        s.SetupCodePending ? t('jellyfin_import.match_setup_pending') : '',
    ].filter(Boolean).join(' · ');
    const others = s.OtherCaseNames || [];
    return `
            <label class="flex items-center gap-2 ml-7 mt-1 text-xs text-white/70">
                <input type="checkbox" class="ji-confirm" value="${escapeHtml(u.JellyfinId)}" data-user-id="${escapeHtml(s.UserId)}" onchange="jiSetConfirm(this)" ${checked ? 'checked' : ''} ${jiSameId(u.JellyfinId, jiMine) ? 'disabled' : ''}>
                <span>${t('jellyfin_import.confirm_match', { name: escapeHtml(s.Name) })}</span>
            </label>
            <p class="ml-7 text-xs text-white/40">${facts}</p>${others.length > 0 ? `
            <p class="ml-7 text-xs text-amber-400">${t('jellyfin_import.match_other_case', { names: others.map(escapeHtml).join(', ') })}</p>` : ''}`;
}

// "Action", or "Action: reason" when there is a reason. The translations are static text.
function jiActionText(u) {
    if (u.MatchKind === 'IntoMyAccount') return t('jellyfin_import.action_into_mine');
    const action = t(jiActionKeys[u.Action]);
    return u.Reason ? `${action}: ${t(jiReasonKeys[u.Reason])}` : action;
}

// Counts are numbers and are not escaped: escapeHtml(0) would be ''.
function jiCountsHtml(r) {
    const { Items: i, WatchData: w, Tracks: k, Metadata: m } = r;
    let metadata = '';
    if (m && m.Mode === 'None') {
        metadata = `<p>${t('jellyfin_import.metadata_summary_none')}</p>`;
    } else if (m) {
        metadata = `<p>${t('jellyfin_import.metadata_summary', { mode: t(jiMetadataModeKeys[m.Mode]), items: m.Items, fields: m.Fields, people: m.People, images: m.Images, missing: m.ImagesMissing, locked: m.Locked })}</p>`;
    }
    return `
        <p class="text-white/40">${t('jellyfin_import.counts_from_preview')}</p>
        <p>${t('jellyfin_import.items_summary', { total: i.Total, by_path: i.MatchedByPath, by_id: i.MatchedById, unmatched: i.Unmatched })}</p>
        <p>${t('jellyfin_import.watch_summary', { rows: w.Rows, imported: w.Imported, unchanged: w.Unchanged, unmatched: w.Unmatched })}</p>
        <p>${t('jellyfin_import.tracks_summary', { audio: k.Audio, subtitle: k.Subtitle, off: k.SubtitleOff, unmapped: k.Unmapped })}</p>${metadata}`;
}

function jiUnpairedHtml(r) {
    const paths = r.UnpairedLibraryPaths || [];
    if (!paths.length) return '';
    return `
        <p class="text-xs text-amber-300">${t('jellyfin_import.unpaired')}</p>
        <ul class="font-mono text-[11px] text-white/60 space-y-0.5">${paths.map(p => `<li>${escapeHtml(p)}</li>`).join('')}</ul>`;
}

function jiSamplesHtml(r) {
    const samples = r.Items.UnmatchedSamples || [];
    if (!samples.length) return '';
    return `
        <p class="text-xs font-semibold text-white/70 mt-2">${t('jellyfin_import.samples_title')}</p>
        <ul class="text-xs text-white/60 space-y-0.5">${samples.map(s => `<li>${escapeHtml(s.Type)} · ${escapeHtml(s.Name)} · <span class="font-mono text-[11px]">${escapeHtml(s.Path)}</span></li>`).join('')}</ul>`;
}

// A finished run's action. Any administrator may read it, so another administrator's "Import into my
// account" names the account (`name`, escaped); otherwise it is jiActionText.
function jiResultText(u, name) {
    if (!name) return jiActionText(u);
    if (u.MatchKind === 'IntoMyAccount') return t('jellyfin_import.action_into_account', { name });
    if (u.Reason === 'AccountTakenByIntoMyAccount') return `${t(jiActionKeys.Skip)}: ${t('jellyfin_import.reason_taken_by_account', { name })}`;
    return jiActionText(u);
}

function jiUserActionsHtml(users) {
    if (!users.length) return '';
    const pick = users.find(u => u.MatchKind === 'IntoMyAccount' && u.TargetAccount);
    const name = pick && !jiSameId(pick.TargetAccount.UserId, API.userId) ? escapeHtml(pick.TargetAccount.Name) : '';
    return `
        <p class="font-semibold text-white/80">${t('jellyfin_import.users_title')}</p>
        <ul class="space-y-0.5">${users.map(u => `<li><span class="text-white">${escapeHtml(u.Name)}</span>${u.WasAdministrator ? ` <span class="jf-badge text-[10px]">${t('jellyfin_import.was_admin')}</span>` : ''} · ${jiResultText(u, name)}</li>`).join('')}</ul>`;
}

async function jiRun() {
    const req = jiRequest();
    jiBusy(true);
    hide('ji-error');
    try {
        await API.jellyfinImportRun(req);
    } catch (err) {
        jiShowError(err.message);
        jiBusy(false);
        return;
    }
    jiRunning = true;
    jiShowProgress({ Stage: null, Done: 0, Total: 0 });
    jiPoll();
}

// Running: the stage, or "starting" before the first one; a bar and a count when the total is known.
function jiShowProgress(st) {
    show('ji-progress');
    hide('ji-result');
    $('ji-progress-text').innerHTML = st.Stage
        ? t('jellyfin_import.running', { stage: t(jiStageKeys[st.Stage]) })
        : t('jellyfin_import.starting');
    if (st.Total > 0) {
        $('ji-bar').style.width = `${Math.min(100, Math.round(100 * st.Done / st.Total))}%`;
        $('ji-progress-count').innerHTML = t('jellyfin_import.progress', { done: st.Done, total: st.Total });
        show('ji-progress-detail');
    } else {
        hide('ji-progress-detail');
    }
}

// Completed: the report and the new users' codes (the first read after completion carries them). Failed: the error.
function jiShowResult(st) {
    hide('ji-progress');
    if (st.State === 'Failed') {
        show('ji-result');
        $('ji-result-text').innerHTML = t('jellyfin_import.failed', { error: escapeHtml(st.Error) });
        $('ji-result-report').innerHTML = '';
        hide('ji-codes-box');
        hide('ji-former-admins');
        return;
    }
    if (st.State !== 'Completed' || !st.Result) {
        hide('ji-result');
        return;
    }
    const r = st.Result;
    const users = r.Users || [];
    show('ji-result');
    $('ji-result-text').innerHTML = t('jellyfin_import.done');
    $('ji-result-report').innerHTML = jiCountsHtml(r) + jiUnpairedHtml(r) + jiSamplesHtml(r) + jiUserActionsHtml(users);
    const coded = users.filter(u => u.Code);
    $('ji-codes').innerHTML = coded.map(u => `
        <tr><td class="py-1 pr-4 text-white">${escapeHtml(u.Name)}</td><td class="ji-code py-1 font-mono select-all text-amber-200">${escapeHtml(u.Code)}</td></tr>`).join('');
    if (coded.length) show('ji-codes-box');
    else hide('ji-codes-box');
    const admins = users.filter(u => u.Action === 'Create' && u.WasAdministrator);
    $('ji-former-admins').innerHTML = t('jellyfin_import.former_admins', { names: admins.map(u => escapeHtml(u.Name)).join(', ') });
    if (admins.length) show('ji-former-admins');
    else hide('ji-former-admins');
}

// One loop: read the status and show it; while it runs, the next read follows in one second.
async function jiPoll() {
    clearTimeout(jiTimer);
    jiTimer = null;
    if (!$('admin-import')) return; // the panel left the page
    let st;
    try {
        st = await API.jellyfinImportStatus();
    } catch (err) {
        jiShowError(err.message);
        if (jiRunning) jiSchedule();
        return;
    }
    hide('ji-error');
    const wasRunning = jiRunning;
    jiRunning = st.State === 'Running';
    if (jiRunning) {
        jiBusy(true);
        jiShowProgress(st);
        jiSchedule();
        return;
    }
    jiBusy(false);
    jiShowResult(st);
    if (wasRunning) jiLoadPending();
}

function jiSchedule() {
    clearTimeout(jiTimer);
    jiTimer = $('admin-import') ? setTimeout(jiPoll, 1000) : null;
}

async function jiLoadPending() {
    try {
        jiRenderPending(await API.jellyfinImportPendingUsers());
    } catch (err) {
        jiShowError(err.message);
    }
}

function jiRenderPending(users) {
    const list = $('ji-pending-list');
    if (!list) return;
    jiPendingNames = new Map(users.map(u => [u.UserId, u.Name]));
    for (const id of [...jiShownCodes.keys()]) {
        if (!jiPendingNames.has(id)) jiShownCodes.delete(id);
    }
    list.innerHTML = users.length
        ? users.map(u => jiPendingRowHtml(u)).join('')
        : `<p class="text-xs text-white/50">${t('jellyfin_import.pending_none')}</p>`;
}

function jiPendingRowHtml(u) {
    const shown = jiShownCodes.get(u.UserId);
    const action = shown
        ? jiCodeLineHtml(shown.name, shown.code)
        : `<button class="jf-btn-ghost !py-1.5 !px-3 text-xs ji-new-code" data-user-id="${escapeHtml(u.UserId)}" onclick="jiNewCode(this)">${t('jellyfin_import.new_code')}</button>`;
    return `
        <div class="ji-pending-row flex flex-wrap items-center justify-between gap-3 py-3 border-b border-white/5">
            <div>
                <p class="text-sm font-semibold text-white">${escapeHtml(u.Name)}</p>
                <p class="text-xs text-white/40">${t('jellyfin_import.pending_since', { date: jiDate(u.CreatedAt) })}</p>
            </div>
            <div class="ji-row-action">${action}</div>
        </div>`;
}

function jiCodeLineHtml(name, code) {
    return `
        <p class="text-xs text-amber-300">${t('jellyfin_import.new_code_shown', { name: escapeHtml(name) })}</p>
        <div class="ji-code font-mono text-sm text-amber-200 select-all">${escapeHtml(code)}</div>`;
}

async function jiNewCode(btn) {
    const id = btn.dataset.userId;
    const name = jiPendingNames.get(id) || '';
    if (!confirm(t('jellyfin_import.new_code_confirm', { name }))) return;
    try {
        const res = await API.jellyfinImportNewCode(id);
        const shownName = res.Name || name;
        jiShownCodes.set(id, { name: shownName, code: res.Code });
        const cell = btn.closest('.ji-row-action');
        if (cell) cell.innerHTML = jiCodeLineHtml(shownName, res.Code);
    } catch (err) {
        toast(err.message, 'error');
        if (err.status === 409) {
            jiShownCodes.delete(id);
            jiLoadPending();
        }
    }
}

async function jiCopyCodes() {
    const table = $('ji-codes');
    if (window.isSecureContext && navigator.clipboard) {
        try {
            const text = [...table.rows].map(row => `${row.cells[0].textContent}\t${row.cells[1].textContent}`).join('\n');
            await navigator.clipboard.writeText(text);
            toast(t('jellyfin_import.copied'));
            return;
        } catch (err) {
            // Not allowed here: fall through and select the codes.
        }
    }
    window.getSelection().selectAllChildren(table);
    toast(t('jellyfin_import.copy_manually'));
}

// Opening the tab reloads the pending users. The status is read once per rendered panel, so
// switching tabs never re-renders a result or drops a code on screen.
async function loadJellyfinImport() {
    const panel = $('admin-import');
    if (!panel) return;
    const first = !panel.dataset.loaded;
    panel.dataset.loaded = '1';
    if (first) {
        // A new panel has no preview on screen, so the old pairs, choices and codes go.
        jiPreviewed = false;
        jiChoice.clear();
        jiConfirm.clear();
        jiMine = '';
        jiShownCodes.clear();
    }
    jiLoadPending();
    if (first) await jiPoll();
}

// --- Scheduled Tasks UI ---

function getTaskStateLabel(state) {
    if (!state) return { text: t('admin.state_ready'), cls: 'bg-emerald-500/20 text-emerald-300' };
    const s = state.toLowerCase();
    if (s === 'running') return { text: t('admin.state_running'), cls: 'bg-amber-500/20 text-amber-300 animate-pulse' };
    if (s === 'cancelling') return { text: t('admin.state_stopping'), cls: 'bg-red-500/20 text-red-300' };
    return { text: t('admin.state_ready'), cls: 'bg-emerald-500/20 text-emerald-300' };
}

function getTaskCategoryIcon(category) {
    if (!category) return '⚙️';
    const c = category.toLowerCase();
    if (c.includes('scan') || c.includes('library')) return '📚';
    if (c.includes('metadata') || c.includes('nfo')) return '🏷️';
    if (c.includes('image') || c.includes('cover')) return '🖼️';
    if (c.includes('cache')) return '💾';
    if (c.includes('cleanup') || c.includes('maintenance')) return '🧹';
    return '⚙️';
}

function renderTaskCard(task) {
    const id = task.Id || task.id;
    const name = task.Name || task.name || id;
    const description = task.Description || task.description || '';
    const state = task.State || task.state || 'Idle';
    const progress = task.CurrentProgressPercentage || task.current_progress_percentage;
    const category = task.Category || task.category || '';
    const lastResult = task.LastExecutionResult || task.last_execution_result;
    const triggers = task.Triggers || task.triggers || [];
    const stateInfo = getTaskStateLabel(state);
    const icon = getTaskCategoryIcon(category);
    const isRunning = state.toLowerCase() === 'running';
    const isCancelling = state.toLowerCase() === 'cancelling';

    // Format last execution info
    let lastExecHtml = '–';
    if (lastResult) {
        const endTime = lastResult.EndTimeUtc || lastResult.end_time_utc;
        const status = lastResult.Status || lastResult.status;
        const statusColors = {
            completed: 'text-emerald-400',
            failed: 'text-red-400',
            cancelled: 'text-yellow-400',
            aborted: 'text-orange-400'
        };
        const statusColor = statusColors[(status || '').toLowerCase()] || 'text-white/40';
        const timeStr = endTime ? new Date(endTime).toLocaleString(uiLocale(), { day: '2-digit', month: '2-digit', year: '2-digit', hour: '2-digit', minute: '2-digit' }) : '–';
        lastExecHtml = `<span class="${statusColor}">${escapeHtml(status || '–')}</span> <span class="text-white/30">(${timeStr})</span>`;
    }

    // Trigger info
    let triggerHtml = '';
    if (triggers.length) {
        const triggerTypes = triggers.map(tr => {
            const type = tr.Type || tr.type || '';
            if (type.includes('Interval')) {
                const ticks = tr.IntervalTicks || tr.interval_ticks;
                if (ticks) {
                    const hours = Math.round(ticks / 3600000);
                    return t('admin.every_hours', { count: hours });
                }
            }
            if (type.includes('TimeOfDay')) {
                const ticks = tr.TimeOfDayTicks || tr.time_of_day_ticks;
                if (ticks) {
                    const hour = Math.floor(ticks / 3600000) % 24;
                    return `${hour.toString().padStart(2, '0')}:00`;
                }
            }
            return escapeHtml(type);
        });
        triggerHtml = `<span class="text-white/40 text-[10px]">🕐 ${triggerTypes.join(', ')}</span>`;
    }

    // Progress bar
    let progressHtml = '';
    if (isRunning && progress !== undefined && progress !== null) {
        const pct = Math.round(progress);
        progressHtml = `
        <div class="mt-3">
            <div class="flex justify-between text-[10px] text-white/40 mb-1">
                <span>${t('admin.progress')}</span>
                <span>${pct}%</span>
            </div>
            <div class="w-full h-1.5 bg-white/10 rounded-full overflow-hidden">
                <div class="h-full bg-gradient-to-r from-[#00a4dc] to-[#38bdf8] rounded-full transition-all duration-500" style="width:${pct}%"></div>
            </div>
        </div>`;
    }

    return `
    <div class="jf-settings-card p-4" id="task-card-${escapeHtml(id)}">
        <div class="flex items-start gap-3">
            <span class="text-2xl flex-shrink-0 mt-0.5">${icon}</span>
            <div class="flex-1 min-w-0">
                <div class="flex items-center gap-2 flex-wrap">
                    <h4 class="font-bold text-white text-sm">${escapeHtml(name)}</h4>
                    <span class="px-2 py-0.5 rounded text-[10px] font-bold ${stateInfo.cls}">${stateInfo.text}</span>
                    ${category ? `<span class="px-2 py-0.5 rounded text-[10px] bg-white/5 text-white/40">${escapeHtml(category)}</span>` : ''}
                </div>
                ${description ? `<p class="text-xs text-white/40 mt-1">${escapeHtml(description)}</p>` : ''}
                <div class="flex items-center gap-3 mt-2 flex-wrap">
                    ${triggerHtml}
                    <span class="text-[10px] text-white/30">${t('admin.last_run', { result: lastExecHtml })}</span>
                </div>
                ${progressHtml}
                <div class="flex items-center gap-2 mt-3">
                    ${isRunning || isCancelling ? `
                        <button class="jf-btn-danger text-xs !py-1.5 !px-3" data-action="stop-task" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}">
                            ${t('admin.stop')}
                        </button>
                    ` : `
                        <button class="jf-btn-primary text-xs !py-1.5 !px-3" data-action="start-task" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}">
                            ${t('admin.start')}
                        </button>
                    `}
                    <button class="jf-btn-ghost text-xs !py-1.5 !px-3 text-white/40" onclick="refreshTasks()" title="${escapeHtml(t('admin.refresh_status'))}">
                        🔄
                    </button>
                </div>
            </div>
        </div>
    </div>`;
}

async function startTaskAction(id, name) {
    try {
        await API.startTask(id);
        toast(t('admin.starting_task', { name }));
        await refreshTasks();
    } catch (e) {
        toast(t('common.error_message', { message: e.message }), 'error');
    }
}

async function stopTaskAction(id, name) {
    try {
        await API.stopTask(id);
        toast(t('admin.stopping_task', { name }));
        await refreshTasks();
    } catch (e) {
        toast(t('common.error_message', { message: e.message }), 'error');
    }
}

async function refreshTasks() {
    try {
        const tasks = await API.getTasks();
        const list = $('admin-tasks-list');
        if (list) {
            list.innerHTML = tasks.map(task => renderTaskCard(task)).join('');
        }
    } catch (e) {
        toast(t('admin.tasks_load_failed', { message: e.message }), 'error');
    }
}

async function loadActivityLog() {
    try {
        const data = await API.getActivityLog(0, 50);
        const items = data.Items || data.items || [];
        if (!items.length) {
            html('activity-content', `<p class="text-white/40 text-xs">${t('admin.no_activity')}</p>`);
            return;
        }
        html('activity-content', `
        <div class="overflow-x-auto">
            <table class="w-full text-left text-xs">
                <thead>
                    <tr class="text-white/40 border-b border-white/10">
                        <th class="pb-2 font-semibold">${t('admin.time')}</th>
                        <th class="pb-2 font-semibold">${t('common.type')}</th>
                        <th class="pb-2 font-semibold">${t('admin.details')}</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-white/5">
                    ${items.map(a => `
                    <tr>
                        <td class="py-2.5 text-white/40 whitespace-nowrap font-mono">${escapeHtml(a.Date || a.date || '–')}</td>
                        <td class="py-2.5"><span class="px-2 py-0.5 rounded bg-white/10 text-white/70">${escapeHtml(a.Type || a.type || t('admin.activity_info'))}</span></td>
                        <td class="py-2.5 text-white">${escapeHtml(a.ShortOverview || a.short_overview || a.Name || a.name || '–')}</td>
                    </tr>`).join('')}
                </tbody>
            </table>
        </div>`);
    } catch (e) {
        html('activity-content', `<p class="text-red-400 text-xs">${escapeHtml(e.message)}</p>`);
    }
}

// --- Sidebar Loader ---
async function loadSidebar() {
    if (!API.isLoggedIn()) {
        hide('sidebar');
        return;
    }
    show('sidebar');
    if (API.isAdmin) {
        show('sidebar-add-lib');
        show('sidebar-admin-section');
    } else {
        hide('sidebar-add-lib');
        hide('sidebar-admin-section');
    }
    let libs;
    try {
        libs = await API.getLibraries();
    } catch (err) {
        toast(t('library.list_failed', { message: err.message }), 'error');
        return;
    }
    libraries = libs;
    const list = $('sidebar-lib-list');
    if (!list) return;
    list.innerHTML = '';
    const icons = {
        movies: '🎬', tvshows: '📺', music: '🎵', photos: '🖼️', mixed: '📁',
        Movies: '🎬', TvShows: '📺', Music: '🎵', Photos: '🖼️', Mixed: '📁'
    };
    libraries.forEach(lib => {
        const id = lib.id || lib.Id;
        const name = lib.name || lib.Name;
        const type = lib.content_type || lib.ContentType || 'Mixed';
        list.innerHTML += `
            <div class="flex items-center gap-1 group">
                <a data-action="navigate" data-page="library" data-id="${escapeHtml(id)}" class="jf-drawer-item ${currentLibraryId === id && currentPage === 'library' ? 'active' : ''} flex-1">
                    <span class="text-base">${icons[type] || '📁'}</span>
                    <span class="truncate">${escapeHtml(name)}</span>
                    ${scanRing(id, name)}
                </a>
                <button type="button" class="jf-icon-btn !w-6 !h-6 text-white/20 hover:text-white opacity-0 group-hover:opacity-100 transition flex-shrink-0" data-action="scan-library" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}" title="${escapeHtml(t('library.scan_library'))}">
                    <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/></svg>
                </button>
            </div>`;
    });
}

// --- Add Library Modal Handlers ---
function openAddLibrary() {
    if (!API.isLoggedIn()) {
        toast(t('login.sign_in_first'), 'error');
        navigate('login');
        return;
    }
    const dlg = $('add-library-modal');
    if (!dlg) return;
    const form = $('add-library-form');
    if (form) form.reset();
    dlg.classList.add('modal-open');
    if (typeof dlg.showModal === 'function') {
        try { dlg.showModal(); } catch { dlg.setAttribute('open', ''); }
    } else {
        dlg.setAttribute('open', '');
    }
}

function closeAddLibrary() {
    const dlg = $('add-library-modal');
    if (dlg) {
        dlg.classList.remove('modal-open');
        if (typeof dlg.close === 'function') {
            try { dlg.close(); } catch { dlg.removeAttribute('open'); }
        } else {
            dlg.removeAttribute('open');
        }
    }
}

async function deleteLibraryFromAdmin(id) {
    if (!confirm(t('admin.remove_library_confirm'))) return;
    try {
        await API.deleteLibrary(id);
    } catch (err) {
        toast(t('library.delete_failed', { message: err.message }), 'error');
        return;
    }
    await loadSidebar();
    renderAdmin();
}

// Saves the checkbox's new state; a failure puts the checkbox back and shows a toast
async function toggleRealtimeMonitor(el) {
    const enabled = el.checked;
    el.disabled = true;
    try {
        const options = await API.getLibraryOptions(el.dataset.id);
        await API.saveLibraryOptions(el.dataset.id, { ...options, EnableRealtimeMonitor: enabled });
    } catch (err) {
        el.checked = !enabled;
        toast(t('admin.realtime_monitor_failed', { message: err.message }), 'error');
    } finally {
        el.disabled = false;
    }
}

// Shows a message inside the add-library dialog; an empty message hides the box
function setAddLibraryError(message) {
    const el = $('add-lib-error');
    if (!el) return;
    el.textContent = message || '';
    el.classList.toggle('hidden', !message);
}

async function submitAddLibrary(e) {
    e.preventDefault();
    const f = e.target;
    const name = f.name.value.trim();
    const type = f.type.value;
    const paths = [...new Set([...f.querySelectorAll('[name="paths"]')].map(i => i.value.trim()).filter(Boolean))];
    const submitBtn = $('add-lib-submit');

    setAddLibraryError('');
    if (!name || !paths.length) {
        setAddLibraryError(t('library.name_and_path_required'));
        return;
    }
    let lib;
    if (submitBtn) submitBtn.disabled = true;
    try {
        lib = await API.createLibrary(name, type, paths, { EnableRealtimeMonitor: f.realtimeMonitor.checked });
    } catch (err) {
        setAddLibraryError(t('library.create_failed', { message: err.message }));
        return;
    } finally {
        if (submitBtn) submitBtn.disabled = false;
    }
    toast(t('library.created', { name }));
    closeAddLibrary();
    // Auto-start scan after creating library
    API.scanLibrary(lib.id || lib.Id).catch(err => toast(t('library.scan_start_failed', { message: err.message }), 'error'));
    // The library exists at this point, so a reload error is not a creation failure: toast it as before
    try {
        await loadSidebar();
        if (currentPage === 'admin') renderAdmin();
        else if (currentPage === 'home') renderHome();
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- Server path fields: autofill and Browse ---
// The parent of an absolute path ("/" for top-level folders and for "/" itself)
function folderParent(path) {
    const trimmed = path.replace(/\/+$/, '');
    return trimmed.slice(0, trimmed.lastIndexOf('/')) || '/';
}

// Suggestions: one shared timer, so only the last field typed in gets its suggestions
let pathSuggestTimer = null;
document.addEventListener('input', e => {
    if (!e.target.matches?.('[data-path-field]')) return;
    const input = e.target;
    clearTimeout(pathSuggestTimer);
    pathSuggestTimer = setTimeout(() => suggestPaths(input), 200);
});

// Per field: busy while a suggestion request is in flight, again when the field changed meanwhile
const pathSuggestStates = new WeakMap();

// At most one request per field: a change during a request queues one more run after it
async function suggestPaths(input) {
    const state = pathSuggestStates.get(input) || { busy: false, again: false };
    pathSuggestStates.set(input, state);
    if (state.busy) {
        state.again = true;
        return;
    }
    state.busy = true;
    try {
        await loadPathSuggestions(input);
    } finally {
        state.busy = false;
    }
    if (state.again) {
        state.again = false;
        suggestPaths(input);
    }
}

async function loadPathSuggestions(input) {
    const list = $('path-suggestions');
    if (!list) return;
    const path = input.value.trim();
    const raw = input.value;
    if (!path.startsWith('/')) {
        list.replaceChildren();
        return;
    }
    const cut = path.lastIndexOf('/');
    let entries;
    try {
        entries = await API.directoryContents(cut === 0 ? '/' : path.slice(0, cut), path.slice(cut + 1));
    } catch {
        entries = [];
    }
    if (input.value !== raw) return; // the field changed while the request ran
    list.replaceChildren(...entries.slice(0, 50).map(entry => {
        const option = document.createElement('option');
        option.value = entry.Path;
        return option;
    }));
}

// The folder browser dialog fills the field it was opened from
let folderBrowserInput = null;
// The folder the dialog lists; '' until a listing succeeds
let folderBrowserPath = '';
// Numbers each listing request; only the latest may touch the dialog
let folderBrowserSeq = 0;

async function openFolderBrowser(input) {
    const dlg = $('folder-browser');
    if (!dlg || !input) return;
    folderBrowserInput = input;
    folderBrowserPath = '';
    dlg.classList.add('modal-open');
    if (typeof dlg.showModal === 'function') {
        try { dlg.showModal(); } catch { dlg.setAttribute('open', ''); }
    } else {
        dlg.setAttribute('open', '');
    }
    // Start at the typed folder, else its parent, else the root; stops when the dialog closes or another load starts
    let start = input.value.trim();
    if (!start.startsWith('/')) start = '/';
    for (const path of new Set([start, folderParent(start), '/'])) {
        const load = folderBrowserLoad(path);
        const seq = folderBrowserSeq; // folderBrowserLoad takes its number synchronously
        if (await load) return;
        if (seq !== folderBrowserSeq || !dlg.open) return;
    }
}

// Lists the folders in path; false if it cannot be read (the error is shown in the list) or a newer load superseded it
async function folderBrowserLoad(path) {
    const list = $('folder-browser-list');
    const dlg = $('folder-browser');
    if (!list || !dlg) return false;
    const seq = ++folderBrowserSeq;
    const isCurrent = () => seq === folderBrowserSeq && dlg.open;
    const upBtn = dlg.querySelector('[data-action="folder-up"]');
    const selectBtn = dlg.querySelector('[data-action="folder-select"]');
    list.innerHTML = '<div class="p-3 text-center text-white/40 text-xs"><div class="w-4 h-4 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin mx-auto"></div></div>';
    let entries;
    try {
        entries = await API.directoryContents(path);
    } catch (e) {
        if (!isCurrent()) return false;
        // The failed path becomes the dialog's path, so Up goes to its parent and Select stays off
        folderBrowserPath = path;
        $('folder-browser-path').textContent = path;
        upBtn.disabled = path === '/';
        selectBtn.disabled = true;
        list.innerHTML = `<div class="p-3 text-center text-red-400 text-xs">${escapeHtml(e.message)}</div>`;
        (upBtn.disabled ? dlg.querySelector('[data-action="folder-cancel"]') : upBtn).focus();
        return false;
    }
    if (!isCurrent()) return false;
    folderBrowserPath = path;
    $('folder-browser-path').textContent = path;
    upBtn.disabled = path === '/';
    selectBtn.disabled = false;
    if (!entries.length) {
        list.innerHTML = `<div class="p-3 text-center text-white/30 text-xs">${t('library.no_subfolders')}</div>`;
    } else {
        list.innerHTML = entries.map(entry => `<button type="button" class="w-full text-left px-3 py-2 text-xs text-white hover:bg-white/5 flex items-center gap-2 rounded" data-action="folder-open" data-path="${escapeHtml(entry.Path)}">📁 <span class="truncate min-w-0">${escapeHtml(entry.Name)}</span></button>`).join('');
    }
    const first = list.querySelector('[data-action="folder-open"]');
    (first || $('folder-browser').querySelector('[data-action="folder-up"]')).focus();
    return true;
}

function folderBrowserUp() {
    if (folderBrowserPath) folderBrowserLoad(folderParent(folderBrowserPath));
}

// Puts the listed folder into the field and closes the dialog
function folderBrowserSelect() {
    const input = folderBrowserInput;
    if (input && folderBrowserPath) {
        input.value = folderBrowserPath;
        input.dispatchEvent(new Event('input', { bubbles: true }));
    }
    closeFolderBrowser();
}

function closeFolderBrowser() {
    const dlg = $('folder-browser');
    if (dlg?.open) dlg.close();
}

// However the dialog closes (Cancel, Select or Escape), focus returns to the field it filled
document.addEventListener('close', e => {
    if (e.target.id !== 'folder-browser') return;
    e.target.classList.remove('modal-open');
    folderBrowserInput?.focus();
}, true);

// Adds a path row to Add Library: a copy of the first row (its Browse button included) with a remove button
function addLibraryPathRow(list) {
    const first = list?.querySelector('.path-field');
    if (!first) return;
    const row = first.cloneNode(true);
    const input = row.querySelector('input');
    input.value = '';
    input.removeAttribute('id');
    input.removeAttribute('aria-invalid');
    input.classList.remove('ring-1', 'ring-red-500');
    const remove = document.createElement('button');
    remove.type = 'button';
    remove.className = 'jf-btn-ghost !px-3 !py-2 text-xs';
    remove.dataset.action = 'remove-lib-path';
    remove.setAttribute('aria-label', t('library.remove_path'));
    remove.textContent = '×';
    row.appendChild(remove);
    list.appendChild(row);
    input.focus();
}

function removeLibraryPathRow(el) {
    el.closest('.path-field')?.remove();
}

// Leaves only the first path row, empty
function resetAddLibraryPaths() {
    const rows = $('add-lib-paths')?.querySelectorAll('.path-field') || [];
    rows.forEach((row, i) => {
        if (i) row.remove();
        else row.querySelector('input').value = '';
    });
}

// Override openAddLibrary to reset state
const _origOpenAddLibrary = openAddLibrary;
openAddLibrary = function() {
    setAddLibraryError('');
    const submitBtn = $('add-lib-submit');
    if (submitBtn) submitBtn.disabled = false;
    resetAddLibraryPaths();
    _origOpenAddLibrary();
};

// --- WebSocket for scan progress ---
// The scan progress socket while it connects or is open, or null
let scanSocket = null;
// The pending retry timer, or null when no retry is scheduled
let scanSocketRetry = null;
// Delay before the next retry in ms; doubles up to 30 s and returns to 1 s when a socket opens
let scanSocketDelay = 1000;
// True once the current outage has been logged, so the warning is printed only once
let scanSocketWarned = false;

function initScanProgressWebSocket() {
    if (typeof WebSocket === 'undefined') return;
    if (!API.isLoggedIn() || scanSocket || scanSocketRetry) return;

    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/socket?api_key=${encodeURIComponent(API.token)}`;

    let ws;
    try {
        ws = new WebSocket(wsUrl);
    } catch (e) {
        console.error('Opening the scan progress socket failed:', e);
        return;
    }
    scanSocket = ws;

    ws.onopen = () => {
        scanSocketDelay = 1000;
        scanSocketWarned = false;
        // The server then sends one frame per scan in progress, so the map is rebuilt from empty
        scanStatuses = {};
        updateScanRings();
    };

    ws.onmessage = (event) => {
        try {
            const data = JSON.parse(event.data);
            if (data.MessageType === 'ScheduledTaskStarted') {
                // Refresh tasks tab if visible
                if (currentPage === 'admin') refreshTasks();
            } else if (data.MessageType === 'ScheduledTaskCompleted') {
                const libId = data.ItemId;
                if (data.State === 'Failed') {
                    toast(t('admin.task_failed', { name: data.Name || t('admin.task'), error: data.Error || t('common.unknown_error') }), 'error');
                } else if (data.State === 'Completed' && libId) {
                    // Name the library; the event's Name is an English server label
                    const lib = libraries.find(l => (l.id || l.Id) === libId);
                    toast(lib ? t('library.scan_finished_for', { name: lib.name || lib.Name }) : t('library.scan_finished'));
                }
                // Refresh tasks tab if visible
                if (currentPage === 'admin') refreshTasks();
            } else if (data.MessageType === 'RefreshProgress') {
                // Scan progress: State "Queued" while it waits its turn, "Active" with each whole percent, "Idle" at "100" when done
                const id = data.Data?.ItemId;
                const p = Number(data.Data?.Progress ?? NaN);
                const state = data.Data?.State;
                if (id && Number.isFinite(p)) {
                    // Taken as is, not maxed: a new run of the same library can start lower than the last one
                    if (state === 'Idle' || p >= 100) delete scanStatuses[id];
                    else scanStatuses[id] = { percent: p, queued: state === 'Queued' };
                    updateScanRings(id);
                }
            } else if (data.MessageType === 'LibraryChanged') {
                // Library changed - refresh current view if on library page
                // Drop the cached lists first, so the re-render is not served from the 30 s cache
                API.invalidateCache('/Items');
                API.invalidateCache('/Shows');
                if (currentPage === 'library' && currentLibraryId) {
                    setTimeout(() => renderLibrary(currentLibraryId), 500);
                } else if (currentPage === 'home') {
                    setTimeout(() => renderHome(), 500);
                }
            }
        } catch (e) {
            console.error('Handling a server event failed:', e);
        }
    };

    ws.onclose = () => {
        // A socket stopped on purpose is no longer the current one
        if (scanSocket !== ws) return;
        scanSocket = null;
        if (!API.isLoggedIn()) return;
        if (!scanSocketWarned) {
            console.warn('Scan progress socket closed, retrying with backoff');
            scanSocketWarned = true;
        }
        // Reconnect with exponential backoff
        scanSocketRetry = setTimeout(() => {
            scanSocketRetry = null;
            initScanProgressWebSocket();
        }, scanSocketDelay);
        scanSocketDelay = Math.min(scanSocketDelay * 2, 30000);
    };
}

// Called on logout and before a login starts a new socket: closes the socket and cancels any pending retry
function stopScanProgressWebSocket() {
    clearTimeout(scanSocketRetry);
    scanSocketRetry = null;
    scanSocketDelay = 1000;
    scanSocketWarned = false;
    const ws = scanSocket;
    scanSocket = null;
    if (ws) ws.close();
}

// --- Add User Modal Handlers ---
function openAddUser() {
    if (!API.isLoggedIn() || !API.isAdmin) {
        toast(t('admin.only_admins_create_users'), 'error');
        return;
    }
    const dlg = $('add-user-modal');
    if (!dlg) return;
    const form = $('add-user-form');
    if (form) form.reset();
    dlg.classList.add('modal-open');
    if (typeof dlg.showModal === 'function') {
        try { dlg.showModal(); } catch { dlg.setAttribute('open', ''); }
    } else {
        dlg.setAttribute('open', '');
    }
}

function closeAddUser() {
    const dlg = $('add-user-modal');
    if (dlg) {
        dlg.classList.remove('modal-open');
        if (typeof dlg.close === 'function') {
            try { dlg.close(); } catch { dlg.removeAttribute('open'); }
        } else {
            dlg.removeAttribute('open');
        }
    }
}

async function submitAddUser(e) {
    e.preventDefault();
    const f = e.target;
    const name = f.name.value.trim();
    const password = f.password.value;
    const isAdmin = f.isAdministrator.checked;

    if (!name || !password) {
        toast(t('admin.name_and_password_required'), 'error');
        return;
    }
    try {
        await API.createUser(name, password, isAdmin);
        toast(t('admin.user_created', { name }));
        closeAddUser();
        if (currentPage === 'admin') renderAdmin();
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- Auth UI Sync ---
function updateAuthUI() {
    if (API.isLoggedIn()) {
        hide('nav-login-btn');
        show('nav-user-menu');
        if ($('nav-avatar-letter')) $('nav-avatar-letter').textContent = (API.userName || 'U')[0].toUpperCase();
        if ($('nav-user-name')) $('nav-user-name').textContent = API.userName || '';
        if ($('dropdown-username')) $('dropdown-username').textContent = API.userName || '';
        show('nav-add-lib');
        if (API.isAdmin) {
            show('nav-admin-link');
            show('sidebar-add-lib');
            show('sidebar-admin-section');
        } else {
            hide('nav-admin-link');
            show('sidebar-add-lib');
            hide('sidebar-admin-section');
        }
    } else {
        show('nav-login-btn');
        hide('nav-user-menu');
        hide('nav-add-lib');
        hide('sidebar');
    }
}

// Reacts to lost administrator rights: hides the admin UI, says so, and leaves the Dashboard.
function followAdminLoss() {
    updateAuthUI();
    toast(t('admin.rights_removed'), 'error');
    if (currentPage === 'admin') navigate('home');
}
API.onAdminLost = followAdminLoss;

// Runs after every sign-out, the user's or a 401's: drops what the signed-out user left in this tab, such as the
// import page's setup codes and the server log, so the next user can't read them.
function afterSignOut() {
    stopScanProgressWebSocket();
    LiveLog.disconnect();
    LiveLog.messages = [];
    jiShownCodes.clear();
    jiPendingNames = new Map();
    jiChoice.clear();
    jiConfirm.clear();
    jiMine = '';
    $('page-admin').innerHTML = ''; // the import preview (#ji-users), its codes box and the live log
    libraries = [];
    updateAuthUI();
}
API.onSignedOut = afterSignOut;

// Another tab signed in or out: reload, so this tab drops the old session's token, cache and pages.
window.addEventListener('storage', e => {
    if (e.key === 'rustyfin_token') location.reload();
});

// --- Live Log Functions ---
let liveLogInitialized = false;

function initLiveLog() {
    if (liveLogInitialized) return;
    liveLogInitialized = true;

    // Connect WebSocket
    if (typeof LiveLog !== 'undefined') {
        LiveLog.onMessage((entry) => {
            appendLogEntry(entry);
        });
        LiveLog.connect();

        // Update status indicator
        const checkStatus = () => {
            const statusEl = $('livelog-status');
            if (!statusEl) return;
            if (LiveLog.connected) {
                statusEl.innerHTML = `<span class="w-2 h-2 rounded-full bg-emerald-400"></span><span class="text-emerald-400">${t('admin.connected')}</span>`;
            } else {
                statusEl.innerHTML = `<span class="w-2 h-2 rounded-full bg-amber-400"></span><span class="text-amber-400">${t('admin.connecting')}</span>`;
            }
        };
        const statusInterval = setInterval(() => {
            checkStatus();
            if (LiveLog.connected) clearInterval(statusInterval);
        }, 1000);
        checkStatus();
    }

    // Load existing messages
    const container = $('livelog-container');
    if (container && LiveLog.messages.length) {
        container.innerHTML = '';
        LiveLog.messages.forEach(entry => appendLogEntry(entry, false));
    }
}

function appendLogEntry(entry, scroll = true) {
    const container = $('livelog-container');
    if (!container) return;

    // Remove loading text on first entry
    if (container.querySelector('p')) {
        container.innerHTML = '';
    }

    const line = document.createElement('div');
    line.className = 'flex gap-2 leading-relaxed';

    // Parse timestamp to local time
    let timeStr = '';
    if (entry.Timestamp) {
        try {
            const d = new Date(entry.Timestamp);
            timeStr = d.toLocaleTimeString(uiLocale(), { hour: '2-digit', minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3 });
        } catch {
            timeStr = entry.Timestamp;
        }
    }

    // Level badge
    const levelColors = {
        'ERROR': 'text-red-400',
        'WARN': 'text-amber-400',
        'INFO': 'text-blue-400',
        'DEBUG': 'text-purple-400',
        'TRACE': 'text-white/30'
    };
    const levelColor = levelColors[entry.Level] || 'text-white/50';
    const levelShort = (entry.Level || 'INFO').substring(0, 4).toUpperCase();

    line.innerHTML = `
        <span class="text-white/25 shrink-0">${escapeHtml(timeStr)}</span>
        <span class="font-bold ${levelColor} shrink-0 w-10">${escapeHtml(levelShort)}</span>
        <span class="text-white/70 break-all">${escapeHtml(entry.Message || '')}</span>
    `;

    container.appendChild(line);

    // Keep max 500 DOM elements
    while (container.children.length > 500) {
        container.removeChild(container.firstChild);
    }

    if (scroll) {
        container.scrollTop = container.scrollHeight;
    }
}

function clearLiveLog() {
    const container = $('livelog-container');
    if (container) {
        container.innerHTML = `<p class="text-white/30 italic">${t('admin.log_cleared')}</p>`;
    }
    if (typeof LiveLog !== 'undefined') {
        LiveLog.messages = [];
    }
}

async function setLogLevel(level) {
    try {
        await API.setLogLevel(level);
        toast(t('admin.log_level_set', { level }));
    } catch (e) {
        toast(t('admin.log_level_failed'), 'error');
    }
}

// --- Plugin Configuration Modal ---
let currentPluginConfigId = null;
let currentPluginConfigOriginal = null;
let currentPluginPriorityId = null;

async function openPluginConfig(pluginId) {
    currentPluginConfigId = pluginId;
    const dlg = $('plugin-config-modal');
    const err = $('plugin-config-error');
    if (!dlg) return;
    hide(err);
    try {
        const config = await API.getPluginConfig(pluginId);
        currentPluginConfigOriginal = JSON.stringify(config, null, 2);
        $('plugin-config-textarea').value = currentPluginConfigOriginal;
    } catch {
        $('plugin-config-textarea').value = '{}';
        currentPluginConfigOriginal = '{}';
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
}

async function savePluginConfig() {
    const err = $('plugin-config-error');
    const textarea = $('plugin-config-textarea');
    let parsed;
    try {
        parsed = JSON.parse(textarea.value);
    } catch (e) {
        err.textContent = t('admin.plugin_config_failed', { message: 'Invalid JSON' });
        show(err);
        return;
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

// --- Hash Router ---
function handleHash() {
    const hash = window.location.hash.slice(1);
    if (!hash) {
        navigate('home');
        return;
    }
    const [page, ...rest] = hash.split('/');
    navigate(page, rest.join('/') || undefined);
}

// --- Bootstrap & Lifecycle ---
window.addEventListener('hashchange', handleHash);

document.addEventListener('DOMContentLoaded', async () => {
    // Every way of closing the player goes through here, so the session is stopped and its position reported once
    $('player-modal')?.addEventListener('close', () => stopPlayback());
    window.addEventListener('pagehide', () => stopPlayback());
    updateAuthUI();
    const wizardActive = await checkSetupWizard();
    if (!wizardActive) {
        await loadSidebar();
        handleHash();
    }
    // Initialize WebSocket for scan progress notifications
    if (API.isLoggedIn()) {
        initScanProgressWebSocket();
    }
});
