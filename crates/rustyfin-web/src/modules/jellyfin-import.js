// The Jellyfin import panel of the dashboard
import { $, show, hide, icon, toast, escapeHtml } from './util.js';
import { oidcAccountKey } from './settings.js';

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
    AccountTakenByIntoMyAccount: 'jellyfin_import.reason_taken_by_mine',
    InvalidName: 'jellyfin_import.reason_invalid_name'
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
                        <button type="button" class="jf-btn-ghost !px-3 !py-2 text-xs whitespace-nowrap inline-flex items-center gap-1.5" data-action="browse-path">${icon('FolderOpen', { size: 14 })}<span>${t('library.browse')}</span></button>
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
            <p id="ji-parental-kept" class="hidden text-xs text-amber-300"></p>
            <p id="ji-parental-unknown" class="hidden text-xs text-amber-300"></p>
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

// The one-time codes box for the users in `users` that carry a code.
function jiShowCodes(users) {
    const coded = users.filter(u => u.Code);
    $('ji-codes').innerHTML = coded.map(u => `
        <tr><td class="py-1 pr-4 text-white">${escapeHtml(u.Name)}</td><td class="ji-code py-1 font-mono select-all text-amber-200">${escapeHtml(u.Code)}</td></tr>`).join('');
    if (coded.length) show('ji-codes-box');
    else hide('ji-codes-box');
}

// Completed: the report and the new users' codes (the first read after completion carries them). Failed: the error,
// and the codes of the users the run created before it failed.
function jiShowResult(st) {
    hide('ji-progress');
    if (st.State === 'Failed') {
        show('ji-result');
        $('ji-result-text').innerHTML = t('jellyfin_import.failed', { error: escapeHtml(st.Error) });
        $('ji-result-report').innerHTML = '';
        jiShowCodes(st.CreatedUsers || []);
        hide('ji-former-admins');
        hide('ji-parental-kept');
        hide('ji-parental-unknown');
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
    jiShowCodes(users);
    const admins = users.filter(u => u.Action === 'Create' && u.WasAdministrator);
    $('ji-former-admins').innerHTML = t('jellyfin_import.former_admins', { names: admins.map(u => escapeHtml(u.Name)).join(', ') });
    if (admins.length) show('ji-former-admins');
    else hide('ji-former-admins');
    // Parental limits the import didn't give an account: kept off an existing account, or not
    // readable from this Jellyfin's database.
    for (const [id, status, key] of [
        ['ji-parental-kept', 'NotCarried', 'jellyfin_import.parental_not_carried'],
        ['ji-parental-unknown', 'Unknown', 'jellyfin_import.parental_unknown'],
    ]) {
        const names = users.filter(u => u.Action !== 'Skip' && u.ParentalLimits === status);
        $(id).innerHTML = t(key, { names: names.map(u => escapeHtml(u.Name)).join(', ') });
        if (names.length) show(id);
        else hide(id);
    }
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

export {
    jiChoice, jiConfirm, jiShownCodes, jiPanelHtml, jiAddPair, jiForget, jiSetMine, jiSetChoice, jiSetConfirm,
    jiPreview, jiRun, jiNewCode, jiCopyCodes, loadJellyfinImport,
};

// Shared state that inline handlers or other modules assign: reached through window
Object.defineProperties(window, {
    jiMine: { get: () => jiMine, set: value => { jiMine = value; } },
    jiPendingNames: { get: () => jiPendingNames, set: value => { jiPendingNames = value; } },
});
