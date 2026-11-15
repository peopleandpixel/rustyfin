// The first-run setup wizard
import { $, show, hide, toast, escapeHtml } from './util.js';
import { navigate } from './router.js';
import { updateAuthUI } from './auth.js';
import { loadSidebar, initScanProgressWebSocket, stopScanProgressWebSocket } from './library-admin.js';
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

// --- Setup Wizard ---
async function checkSetupWizard() {
    try {
        const info = await API.getPublicInfo();
        const done = info.StartupWizardCompleted ?? info.startup_wizard_completed ?? true;
        // Update version badge with server version
        if (info.Version || info.version) {
            serverVersion = info.Version || info.version;
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
    el.querySelector('.path-field button span').textContent = t('library.browse');
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

export {
    serverVersion, checkSetupWizard, wizSetUiLanguage, wizApplyPath, wizAddManualLibrary, wizNext, wizPrev,
    wizSkipLibrary, wizFinishWithoutImport, wizComplete,
};
