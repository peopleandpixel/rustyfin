// The administrator's user editor, Jellyfin Web's dashboard user pages in one dialog: the profile
// (name and the policy switches), library access, parental control with access schedules, and a
// password reset. Opened from the users table's Edit button.
import { $, toast, escapeHtml, icon } from './util.js';
import { renderAdmin, showAdminTab } from './admin.js';

// Jellyfin's UnratedItem values, in Jellyfin Web's order
const UNRATED_KINDS = ['Book', 'ChannelContent', 'LiveTvChannel', 'Movie', 'Music', 'Trailer', 'Series', 'Other'];
const SCHEDULE_DAYS = ['Everyday', 'Weekday', 'Weekend', 'Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
// The profile tab's switches: policy key and label key
const PROFILE_SWITCHES = [
    ['IsAdministrator', 'user_editor.is_admin'],
    ['IsHidden', 'user_editor.is_hidden'],
    ['IsDisabled', 'user_editor.is_disabled'],
    ['EnableRemoteAccess', 'user_editor.remote_access'],
    ['EnableContentDeletion', 'user_editor.content_deletion'],
];
const PLAYBACK_SWITCHES = [
    ['EnableMediaPlayback', 'user_editor.media_playback'],
    ['EnableAudioPlaybackTranscoding', 'user_editor.audio_transcoding'],
    ['EnableVideoPlaybackTranscoding', 'user_editor.video_transcoding'],
    ['EnablePlaybackRemuxing', 'user_editor.remuxing'],
    ['ForceRemoteSourceTranscoding', 'user_editor.force_remote_transcoding'],
];
const TABS = ['profile', 'access', 'parental', 'password'];

// The open editor: the user, and the lists edited in place (schedules and tags) until Save
let editor = null;

// 8.5 -> "08:30"
function hourLabel(hour) {
    const h = Math.floor(hour);
    const m = Math.round((hour - h) * 60);
    return `${String(h).padStart(2, '0')}:${String(m).padStart(2, '0')}`;
}

function hourOptions(name, selected) {
    let options = '';
    for (let h = 0; h <= 24; h += 0.5) {
        options += `<option value="${h}" ${h === selected ? 'selected' : ''}>${hourLabel(h)}</option>`;
    }
    return `<select name="${name}" class="jf-input !py-1 text-xs">${options}</select>`;
}

function checkbox(name, label, checked, attrs = '') {
    return `<label class="flex items-center gap-2 text-xs text-white/80 py-1"><input type="checkbox" name="${name}" ${checked ? 'checked' : ''} ${attrs} class="w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]">${escapeHtml(label)}</label>`;
}

// The rating select's value: "score:subScore", "" for no limit
function ratingValue(score, sub) {
    return score === null || score === undefined ? '' : `${score}:${sub ?? ''}`;
}

function schedulesHtml(schedules) {
    if (!schedules.length) return `<p class="text-xs text-white/40">${escapeHtml(t('user_editor.no_schedules'))}</p>`;
    return schedules.map((s, i) => `
        <div class="flex items-center justify-between py-1 text-xs text-white/80">
            <span>${escapeHtml(t(`user_editor.day_${s.DayOfWeek.toLowerCase()}`))} ${hourLabel(s.StartHour)}–${hourLabel(s.EndHour)}</span>
            <button type="button" class="jf-icon-btn !w-6 !h-6" data-action="user-editor-remove-schedule" data-index="${i}" aria-label="${escapeHtml(t('common.delete'))}">${icon('Trash2', { size: 14 })}</button>
        </div>`).join('');
}

function tagsHtml(kind, tags) {
    if (!tags.length) return `<p class="text-xs text-white/40">${escapeHtml(t('user_editor.no_tags'))}</p>`;
    return tags.map((tag, i) => `
        <span class="inline-flex items-center gap-1 px-2 py-0.5 mr-1 mb-1 rounded bg-white/10 text-xs text-white/80">${escapeHtml(tag)}
            <button type="button" data-action="user-editor-remove-tag" data-kind="${kind}" data-index="${i}" aria-label="${escapeHtml(t('common.delete'))}">${icon('X', { size: 12 })}</button>
        </span>`).join('');
}

// The editor's markup. `libs` are the libraries (/Library/VirtualFolders), `ratings` the
// /Localization/ParentalRatings list; `self` and `owner` lock the switches the server refuses.
function userEditorHtml(user, libs, ratings, { self = false, owner = false } = {}) {
    const p = user.Policy || {};
    const locked = self || owner;
    const deletion = p.EnableContentDeletion ?? p.IsAdministrator;
    const enabled = new Set((p.EnabledFolders || []).map(id => String(id).replace(/-/g, '').toLowerCase()));
    const current = ratingValue(p.MaxParentalRating, p.MaxParentalSubRating);
    const ratingOptions = ratings.filter(r => r.RatingScore).map(r => {
        const value = ratingValue(r.RatingScore.score, r.RatingScore.subScore);
        return `<option value="${escapeHtml(value)}">${escapeHtml(r.Name)}</option>`;
    });
    // The first option of the stored score is the one shown
    const selectedIndex = ratings.filter(r => r.RatingScore)
        .findIndex(r => ratingValue(r.RatingScore.score, r.RatingScore.subScore) === current
            || (p.MaxParentalSubRating == null && r.RatingScore.score === p.MaxParentalRating));
    if (selectedIndex >= 0) ratingOptions[selectedIndex] = ratingOptions[selectedIndex].replace('<option ', '<option selected ');
    const tab = (name, i) => `<button type="button" class="px-3 py-1.5 rounded-lg text-xs font-semibold ${i === 0 ? 'bg-[#00a4dc] text-white' : 'bg-white/5 text-white/60'}" data-action="user-editor-tab" data-tab="${name}">${escapeHtml(t(`user_editor.tab_${name}`))}</button>`;
    return `
    <form id="user-editor-form" class="modal-box max-w-xl jf-menu rounded-2xl p-6" onsubmit="return false">
        <h3 class="font-bold text-white text-base mb-3">${escapeHtml(user.Name)}</h3>
        <div class="flex gap-2 mb-4">${TABS.map(tab).join('')}</div>

        <section data-tab-panel="profile" class="space-y-1">
            <label class="block text-xs text-white/60 mb-2">${escapeHtml(t('common.name'))}
                <input name="Name" value="${escapeHtml(user.Name)}" maxlength="255" class="jf-input w-full mt-1" required></label>
            ${PROFILE_SWITCHES.map(([key, label]) => checkbox(key, t(label),
                key === 'EnableContentDeletion' ? deletion : p[key],
                locked && (key === 'IsAdministrator' || key === 'IsDisabled') ? 'disabled' : '')).join('')}
            ${locked ? `<p class="text-[10px] text-white/40">${escapeHtml(t(owner ? 'admin.owner_locked' : 'admin.self_locked'))}</p>` : ''}
            <h4 class="text-xs font-semibold text-white pt-3">${escapeHtml(t('user_editor.playback'))}</h4>
            ${PLAYBACK_SWITCHES.map(([key, label]) => checkbox(key, t(label), p[key])).join('')}
            <label class="block text-xs text-white/60 pt-2">${escapeHtml(t('user_editor.max_sessions'))}
                <input type="number" name="MaxActiveSessions" min="0" step="1" value="${Number(p.MaxActiveSessions) || 0}" class="jf-input w-32 mt-1"></label>
            <label class="block text-xs text-white/60 pt-2">${escapeHtml(t('user_editor.bitrate_limit'))}
                <input type="number" name="RemoteClientBitrateLimit" min="0" step="0.1" value="${p.RemoteClientBitrateLimit ? p.RemoteClientBitrateLimit / 1e6 : ''}" class="jf-input w-32 mt-1"></label>
        </section>

        <section data-tab-panel="access" class="hidden space-y-1">
            ${checkbox('EnableAllFolders', t('user_editor.all_libraries'), p.EnableAllFolders !== false, 'data-action="user-editor-all-folders"')}
            <div id="user-editor-folders" class="${p.EnableAllFolders !== false ? 'hidden' : ''} pl-6">
                ${libs.length ? libs.map(l => {
                    const id = String(l.ItemId || l.Id || l.id);
                    return checkbox('folder', l.Name || l.name, enabled.has(id.replace(/-/g, '').toLowerCase()), `value="${escapeHtml(id)}"`);
                }).join('') : `<p class="text-xs text-white/40">${escapeHtml(t('user_editor.no_libraries'))}</p>`}
            </div>
        </section>

        <section data-tab-panel="parental" class="hidden space-y-3">
            <label class="block text-xs text-white/60">${escapeHtml(t('user_editor.max_rating'))}
                <select name="MaxParentalRating" class="jf-input w-full mt-1">
                    <option value="">${escapeHtml(t('user_editor.no_rating_limit'))}</option>${ratingOptions.join('')}
                </select></label>
            <div>
                <h4 class="text-xs font-semibold text-white">${escapeHtml(t('user_editor.block_unrated'))}</h4>
                ${UNRATED_KINDS.map(kind => checkbox('unrated', t(`user_editor.unrated_${kind.toLowerCase()}`), (p.BlockUnratedItems || []).includes(kind), `value="${kind}"`)).join('')}
            </div>
            ${['AllowedTags', 'BlockedTags'].map(kind => `
            <div>
                <h4 class="text-xs font-semibold text-white">${escapeHtml(t(kind === 'AllowedTags' ? 'user_editor.allowed_tags' : 'user_editor.blocked_tags'))}</h4>
                <p class="text-[10px] text-white/40 mb-1">${escapeHtml(t(kind === 'AllowedTags' ? 'user_editor.allowed_tags_hint' : 'user_editor.blocked_tags_hint'))}</p>
                <div id="user-editor-${kind}">${tagsHtml(kind, editor?.[kind] || p[kind] || [])}</div>
                <div class="flex gap-2 mt-1"><input name="new-${kind}" class="jf-input !py-1 text-xs flex-1" aria-label="${escapeHtml(t('user_editor.add_tag'))}">
                    <button type="button" class="jf-btn-ghost !py-1 !px-3 text-xs" data-action="user-editor-add-tag" data-kind="${kind}">${escapeHtml(t('user_editor.add_tag'))}</button></div>
            </div>`).join('')}
            <div>
                <h4 class="text-xs font-semibold text-white">${escapeHtml(t('user_editor.schedules'))}</h4>
                <p class="text-[10px] text-white/40 mb-1">${escapeHtml(t('user_editor.schedules_hint'))}</p>
                <div id="user-editor-schedules">${schedulesHtml(editor?.AccessSchedules || p.AccessSchedules || [])}</div>
                <div class="flex flex-wrap items-center gap-2 mt-1">
                    <select name="schedule-day" class="jf-input !py-1 text-xs">${SCHEDULE_DAYS.map(d => `<option value="${d}">${escapeHtml(t(`user_editor.day_${d.toLowerCase()}`))}</option>`).join('')}</select>
                    ${hourOptions('schedule-start', 0)} – ${hourOptions('schedule-end', 24)}
                    <button type="button" class="jf-btn-ghost !py-1 !px-3 text-xs" data-action="user-editor-add-schedule">${escapeHtml(t('user_editor.add_schedule'))}</button>
                </div>
            </div>
        </section>

        <section data-tab-panel="password" class="hidden space-y-2">
            <label class="block text-xs text-white/60">${escapeHtml(t('user_editor.new_password'))}
                <input type="password" name="NewPw" autocomplete="new-password" class="jf-input w-full mt-1"></label>
            <label class="block text-xs text-white/60">${escapeHtml(t('user_editor.confirm_password'))}
                <input type="password" name="ConfirmPw" autocomplete="new-password" class="jf-input w-full mt-1"></label>
            <button type="button" class="jf-btn-primary text-xs" data-action="user-editor-password">${escapeHtml(t('user_editor.set_password'))}</button>
            <p class="text-[10px] text-white/40">${escapeHtml(t('user_editor.password_hint'))}</p>
        </section>

        <div class="modal-action">
            <button type="button" class="btn btn-sm" data-action="user-editor-close">${escapeHtml(t('common.cancel'))}</button>
            <button type="button" class="jf-btn-primary text-xs" data-action="user-editor-save">${escapeHtml(t('user_editor.save'))}</button>
        </div>
    </form>`;
}

// The name and the policy the form sets, over the user's stored policy, which Jellyfin Web posts
// back whole. `lists` holds the schedules and tags edited in place.
function readUserEditor(form, policy, lists) {
    const el = name => form.elements[name];
    const out = { ...policy };
    for (const [key] of [...PROFILE_SWITCHES, ...PLAYBACK_SWITCHES]) {
        if (!el(key).disabled) out[key] = el(key).checked;
    }
    out.MaxActiveSessions = Math.max(0, parseInt(el('MaxActiveSessions').value, 10) || 0);
    out.RemoteClientBitrateLimit = Math.max(0, Math.round((parseFloat(el('RemoteClientBitrateLimit').value) || 0) * 1e6));
    out.EnableAllFolders = el('EnableAllFolders').checked;
    out.EnabledFolders = Array.from(form.querySelectorAll('input[name="folder"]:checked'), i => i.value);
    const [score, sub] = el('MaxParentalRating').value.split(':');
    out.MaxParentalRating = score === undefined || score === '' ? null : Number(score);
    out.MaxParentalSubRating = sub === undefined || sub === '' ? null : Number(sub);
    out.BlockUnratedItems = Array.from(form.querySelectorAll('input[name="unrated"]:checked'), i => i.value);
    out.AllowedTags = [...lists.AllowedTags];
    out.BlockedTags = [...lists.BlockedTags];
    out.AccessSchedules = lists.AccessSchedules.map(s => ({ ...s }));
    return { name: el('Name').value.trim(), policy: out };
}

function editorDialog() {
    let dlg = $('user-editor');
    if (!dlg) {
        dlg = document.createElement('dialog');
        dlg.id = 'user-editor';
        dlg.className = 'modal bg-black/70 backdrop-blur-md';
        document.body.appendChild(dlg);
    }
    return dlg;
}

async function openUserEditor(id) {
    let user, libs, ratings;
    try {
        [user, libs, ratings] = await Promise.all([
            API.get(`/Users/${encodeURIComponent(id)}`),
            API.getLibraries().catch(() => []),
            API.get('/Localization/ParentalRatings').catch(() => []),
        ]);
    } catch (err) {
        toast(err.message, 'error');
        return;
    }
    const p = user.Policy || {};
    editor = {
        user,
        AllowedTags: [...(p.AllowedTags || [])],
        BlockedTags: [...(p.BlockedTags || [])],
        AccessSchedules: (p.AccessSchedules || []).map(s => ({ DayOfWeek: s.DayOfWeek, StartHour: s.StartHour, EndHour: s.EndHour })),
    };
    const dlg = editorDialog();
    dlg.innerHTML = userEditorHtml(user, libs, ratings, {
        self: String(user.Id).replace(/-/g, '').toLowerCase() === String(API.userId).replace(/-/g, '').toLowerCase(),
        owner: user.IsOwner === true,
    });
    if (!dlg.open) dlg.showModal();
}

function closeUserEditor() {
    $('user-editor')?.close();
    editor = null;
}

function showUserEditorTab(btn) {
    const form = $('user-editor-form');
    form.querySelectorAll('[data-action="user-editor-tab"]').forEach(b => {
        const on = b === btn;
        b.classList.toggle('bg-[#00a4dc]', on);
        b.classList.toggle('text-white', on);
        b.classList.toggle('bg-white/5', !on);
        b.classList.toggle('text-white/60', !on);
    });
    form.querySelectorAll('[data-tab-panel]').forEach(s => s.classList.toggle('hidden', s.dataset.tabPanel !== btn.dataset.tab));
}

function addUserEditorTag(kind) {
    const input = $('user-editor-form').elements[`new-${kind}`];
    const tag = input.value.trim();
    if (tag && !editor[kind].some(t => t.toLowerCase() === tag.toLowerCase())) editor[kind].push(tag);
    input.value = '';
    $(`user-editor-${kind}`).innerHTML = tagsHtml(kind, editor[kind]);
}

function removeUserEditorTag(kind, index) {
    editor[kind].splice(index, 1);
    $(`user-editor-${kind}`).innerHTML = tagsHtml(kind, editor[kind]);
}

function addUserEditorSchedule() {
    const f = $('user-editor-form').elements;
    const start = Number(f['schedule-start'].value);
    const end = Number(f['schedule-end'].value);
    if (end <= start) {
        toast(t('user_editor.schedule_order'), 'error');
        return;
    }
    editor.AccessSchedules.push({ DayOfWeek: f['schedule-day'].value, StartHour: start, EndHour: end });
    $('user-editor-schedules').innerHTML = schedulesHtml(editor.AccessSchedules);
}

function removeUserEditorSchedule(index) {
    editor.AccessSchedules.splice(index, 1);
    $('user-editor-schedules').innerHTML = schedulesHtml(editor.AccessSchedules);
}

// Saves the name (when changed) and the policy, then shows the users tab again
async function saveUserEditor(btn) {
    const { user } = editor;
    const { name, policy } = readUserEditor($('user-editor-form'), user.Policy || {}, editor);
    if (!name) {
        toast(t('admin.name_and_password_required'), 'error');
        return;
    }
    btn.disabled = true;
    try {
        if (name !== user.Name) await API.post(`/Users/${user.Id}`, { Name: name });
        await API.updatePolicy(user.Id, policy);
        toast(t('admin.user_updated'));
        closeUserEditor();
    } catch (err) {
        toast(err.message, 'error');
        btn.disabled = false;
        return;
    }
    API.invalidateCache('/Users');
    await renderAdmin();
    const tab = document.querySelector('[data-action="admin-tab"][data-tab="admin-users"]');
    if (tab) showAdminTab(tab, 'admin-users');
}

async function setUserEditorPassword(btn) {
    const f = $('user-editor-form').elements;
    if (!f.NewPw.value) {
        toast(t('user_editor.password_required'), 'error');
        return;
    }
    if (f.NewPw.value !== f.ConfirmPw.value) {
        toast(t('user_editor.password_mismatch'), 'error');
        return;
    }
    btn.disabled = true;
    try {
        await API.post(`/Users/${editor.user.Id}/Password`, { NewPw: f.NewPw.value });
        f.NewPw.value = '';
        f.ConfirmPw.value = '';
        toast(t('user_editor.password_set'));
    } catch (err) {
        toast(err.message, 'error');
    }
    btn.disabled = false;
}

export {
    openUserEditor, closeUserEditor, showUserEditorTab, addUserEditorTag, removeUserEditorTag, addUserEditorSchedule,
    removeUserEditorSchedule, saveUserEditor, setUserEditorPassword,
};
