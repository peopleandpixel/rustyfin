// The admin dashboard: users, single sign-on settings, tasks, activity and the live log
import { $, html, show, hide, icon, toast, escapeHtml, scanRing } from './util.js';
import { currentPage } from './router.js';
import { serverVersion } from './wizard.js';
import { oidcAccountKey } from './settings.js';
import { jiPanelHtml, loadJellyfinImport } from './jellyfin-import.js';
import { sessionsPanelHtml, followSessions, unfollowSessions } from './sessions.js';
import { playbackPanelHtml, loadPlaybackAdmin } from './playback-admin.js';

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

        const tabs = [t('admin.overview'), t('admin.users'), t('admin.libraries'), t('admin.playback'), t('admin.tasks'), t('admin.plugins'), t('admin.sessions'), t('admin.activity_log_tab'), t('admin.live_log'), t('oidc.admin_tab'), t('jellyfin_import.tab')];
        const tabIds = ['admin-overview', 'admin-users', 'admin-libs', 'admin-playback', 'admin-tasks', 'admin-plugins', 'admin-sessions', 'admin-activity', 'admin-livelog', 'admin-signin', 'admin-import'];

        page.innerHTML = `
        <div class="max-w-6xl mx-auto jf-page">
            <div class="flex items-center justify-between mb-6">
                <div>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white">${t('nav.server_dashboard')}</h1>
                    <p class="text-xs text-white/40 mt-1">${t('admin.subtitle', { name: escapeHtml(info.ServerName || 'Rustyfin') })}</p>
                </div>
                <div class="flex gap-2">
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs inline-flex items-center gap-1.5" data-action="scan-all">
                        ${icon('Library', { size: 14 })}${t('admin.scan_all')}
                    </button>
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs text-amber-400 hover:text-amber-300 inline-flex items-center gap-1.5" onclick="if(confirm(t('admin.restart_confirm')))API.restart().then(()=>toast(t('admin.restarting')))">
                        ${icon('RotateCcw', { size: 14 })}${t('admin.restart')}
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
                                <td class="py-3"><label class="inline-flex items-center gap-2 text-white/80"><input type="checkbox" data-action="set-user-admin" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}" aria-label="${escapeHtml(t('admin.administrator_for', { name }))}" ${isAdmin ? 'checked' : ''} ${locked || promoteBlocked ? 'disabled' : ''} class="w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]">${isAdmin ? `<span class="px-2 py-0.5 rounded text-[10px] font-bold bg-[#00a4dc]/20 text-[#38bdf8]">${escapeHtml(t('admin.role_admin'))}</span>` : `<span class="text-white/40">${escapeHtml(t('admin.role_user'))}</span>`}</label>${isOwner ? `<span class="ml-1.5 px-2 py-0.5 rounded text-[10px] font-bold bg-[#00a4dc]/20 text-[#38bdf8]">${escapeHtml(t('admin.role_owner'))}</span>` : ''}${account?.AdminFromGroup ? `<span class="ml-1.5 text-[10px] text-white/40">${escapeHtml(t('oidc.from_group'))}</span><p class="text-[10px] text-white/40 mt-1">${escapeHtml(t('admin.group_admin_hint'))}</p>` : ''}${locked ? `<p class="text-[10px] text-white/40 mt-1">${escapeHtml(t(isOwner ? 'admin.owner_locked' : 'admin.self_locked'))}</p>` : ''}${promoteBlocked ? `<p class="text-[10px] text-white/40 mt-1">${escapeHtml(t('admin.promote_disabled_locked'))}</p>` : ''}</td>
                                <td class="py-3"><input type="checkbox" data-action="set-user-disabled" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}" aria-label="${escapeHtml(t('admin.disabled_for', { name }))}" ${u.IsDisabled ? 'checked' : ''} ${locked || disableBlocked ? 'disabled' : ''} class="w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]">${disableBlocked ? `<p class="text-[10px] text-white/40 mt-1">${escapeHtml(t('admin.disable_admin_locked'))}</p>` : ''}</td>
                                <td class="py-3 text-white/40">${escapeHtml(u.LastLoginDate || u.last_login_date || '–')}</td>
                                <td class="py-3 text-white/60">${account?.Linked ? `${escapeHtml(account.ProviderUsername || '—')} <button type="button" class="ml-2 text-[#00a4dc] hover:text-[#38bdf8] font-semibold" data-action="oidc-remove-link" data-id="${escapeHtml(account.UserId)}" data-name="${escapeHtml(name)}">${escapeHtml(t('oidc.remove_link'))}</button>` : '<span class="text-white/40">—</span>'}</td>
                                <td class="py-3 text-right space-x-2">
                                    <button class="text-[#00a4dc] hover:text-[#38bdf8] font-semibold" data-action="edit-user" data-id="${escapeHtml(id)}" aria-label="${escapeHtml(t('user_editor.edit_for', { name }))}">${t('user_editor.edit')}</button>
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
                                    <button class="text-[#00a4dc] hover:text-[#38bdf8]" data-action="library-options" data-id="${escapeHtml(l.id || l.Id)}" data-name="${escapeHtml(l.name || l.Name)}" aria-label="${escapeHtml(t('library.options_for', { name: l.name || l.Name }))}">${t('library.options_button')}</button>
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
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs inline-flex items-center gap-1.5" onclick="refreshTasks()">${icon('RefreshCw', { size: 14 })}${t('admin.refresh')}</button>
                </div>
                <div id="admin-tasks-list" class="grid gap-3">
                    ${sortTasks(tasks).map(task => renderTaskCard(task)).join('')}
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
                ${sessionsPanelHtml(sessions)}
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

            <!-- Playback: transcoding, resume and streaming settings, filled by loadPlaybackAdmin -->
            ${playbackPanelHtml()}

            ${jiPanelHtml()}
        </div>`;
        // #admin/import opens the import tab (the wizard's last step sends the admin there), and
        // #admin/playback the playback tab
        if (tab === 'import') showAdminTab(page.querySelector('[data-tab="admin-import"]'), 'admin-import');
        if (tab === 'playback') showAdminTab(page.querySelector('[data-tab="admin-playback"]'), 'admin-playback');
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

function showAdminTab(btn, tabId) {
    btn.parentElement.querySelectorAll('button').forEach(b => {
        b.className = 'px-4 py-2 rounded-xl text-xs font-semibold whitespace-nowrap transition bg-white/5 text-white/60 hover:bg-white/10';
    });
    btn.className = 'px-4 py-2 rounded-xl text-xs font-semibold whitespace-nowrap transition bg-[#00a4dc] text-white shadow-lg';

    ['admin-overview', 'admin-users', 'admin-libs', 'admin-playback', 'admin-tasks', 'admin-plugins', 'admin-sessions', 'admin-activity', 'admin-livelog', 'admin-signin', 'admin-import'].forEach(id => {
        const el = $(id);
        if (el) el.classList.toggle('hidden', id !== tabId);
    });

    if (tabId === 'admin-tasks') refreshTasks(true);
    else stopTaskPolling();
    if (tabId === 'admin-activity') loadActivityLog();
    if (tabId === 'admin-livelog') initLiveLog();
    if (tabId === 'admin-sessions') followSessions();
    else unfollowSessions();
    if (tabId === 'admin-signin') loadOidcAdminTab();
    if (tabId === 'admin-playback') loadPlaybackAdmin();
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
    // B156: the provider sends users back to the public URL only, so a sign-in from another address is refused
    const publicOrigin = (() => { try { return new URL(s.PublicUrl).origin; } catch { return ''; } })();
    const originWarn = (key, origin) => `<p class="text-xs text-amber-300/80 mt-1" role="alert">${escapeHtml(t(key, { origin, url: s.PublicUrl }))}</p>`;
    const originWarnings = [
        publicOrigin && publicOrigin !== location.origin ? originWarn('oidc.public_url_other_page', location.origin) : '',
        s.MismatchedOrigin && s.MismatchedOrigin !== location.origin ? originWarn('oidc.public_url_other_signin', s.MismatchedOrigin) : ''
    ].join('');

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
                ${originWarnings}
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

// --- Scheduled Tasks UI ---

function getTaskStateLabel(state) {
    if (!state) return { text: t('admin.state_ready'), cls: 'bg-emerald-500/20 text-emerald-300' };
    const s = state.toLowerCase();
    if (s === 'running') return { text: t('admin.state_running'), cls: 'bg-amber-500/20 text-amber-300 animate-pulse' };
    if (s === 'cancelling') return { text: t('admin.state_stopping'), cls: 'bg-red-500/20 text-red-300' };
    return { text: t('admin.state_ready'), cls: 'bg-emerald-500/20 text-emerald-300' };
}

function getTaskCategoryIcon(category) {
    if (!category) return 'Settings';
    const c = category.toLowerCase();
    if (c.includes('scan') || c.includes('library')) return 'Library';
    if (c.includes('metadata') || c.includes('nfo')) return 'Tag';
    if (c.includes('image') || c.includes('cover')) return 'Image';
    if (c.includes('cache')) return 'HardDrive';
    if (c.includes('cleanup') || c.includes('maintenance')) return 'BrushCleaning';
    return 'Settings';
}

function renderTaskCard(task) {
    const id = task.Id || task.id;
    const name = task.Name || task.name || id;
    const description = task.Description || task.description || '';
    const state = task.State || task.state || 'Idle';
    const progress = task.CurrentProgressPercentage ?? task.current_progress_percentage;
    const category = task.Category || task.category || '';
    const lastResult = task.LastExecutionResult || task.last_execution_result;
    const triggers = task.Triggers || task.triggers || [];
    const stateInfo = getTaskStateLabel(state);
    const categoryIcon = getTaskCategoryIcon(category);
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
        // The task's message says what it did, or why it did nothing. It belongs to the last run,
        // so it is left out while a new run goes, which could be read as that run's outcome.
        const message = isRunning || isCancelling ? '' : lastResult.ErrorMessage || lastResult.error_message;
        const messageHtml = message ? ` <span class="text-white/50" data-task-message>· ${escapeHtml(message)}</span>` : '';
        lastExecHtml = `<span class="${statusColor}">${escapeHtml(status || '–')}</span> <span class="text-white/30">(${timeStr})</span>${messageHtml}`;
    }

    // Trigger info
    let triggerHtml = '';
    if (triggers.length) {
        // Trigger times are .NET ticks: 10,000,000 a second
        const TICKS_PER_MINUTE = 600000000;
        const TICKS_PER_HOUR = 60 * TICKS_PER_MINUTE;
        const triggerTypes = triggers.map(tr => {
            const type = tr.Type || tr.type || '';
            if (type.includes('Interval')) {
                const ticks = tr.IntervalTicks || tr.interval_ticks;
                if (ticks) {
                    const hours = Math.round(ticks / TICKS_PER_HOUR);
                    return t('admin.every_hours', { count: hours });
                }
            }
            // Jellyfin's DailyTrigger and WeeklyTrigger carry the time of day
            if (type.includes('TimeOfDay') || type.includes('Daily') || type.includes('Weekly')) {
                const ticks = tr.TimeOfDayTicks ?? tr.time_of_day_ticks;
                if (typeof ticks === 'number') {
                    const hour = Math.floor(ticks / TICKS_PER_HOUR) % 24;
                    const minute = Math.floor(ticks / TICKS_PER_MINUTE) % 60;
                    return `${hour.toString().padStart(2, '0')}:${minute.toString().padStart(2, '0')}`;
                }
            }
            return escapeHtml(type);
        });
        triggerHtml = `<span class="text-white/40 text-[10px]">${icon('Clock', { size: 10, class: 'inline-block align-[-1px]' })} ${triggerTypes.join(', ')}</span>`;
    }

    // Progress bar: the percent when the task reports one, else a sliding stripe
    let progressHtml = '';
    if (isRunning || isCancelling) {
        const known = typeof progress === 'number' && Number.isFinite(progress);
        // The bar takes the exact percent; the label one decimal below 10% and whole numbers from
        // there, both rounded down, so a running task never reads 10.0% early or 100% before it ends
        const exact = known ? Math.max(0, Math.min(100, progress)) : null;
        const pct = !known ? null : exact < 10 ? (Math.floor(exact * 10) / 10).toFixed(1) : String(Math.floor(exact));
        const bar = known
            ? `<div class="h-full bg-gradient-to-r from-[#00a4dc] to-[#38bdf8] rounded-full transition-all duration-500" style="width:${exact}%"></div>`
            : '<div class="h-full bg-gradient-to-r from-[#00a4dc] to-[#38bdf8] rounded-full jf-progress-indeterminate"></div>';
        const aria = known ? `aria-valuemin="0" aria-valuemax="100" aria-valuenow="${exact}" aria-valuetext="${pct}%"` : '';
        progressHtml = `
        <div class="mt-3" data-task-progress="${known ? pct : 'indeterminate'}">
            <div class="flex justify-between text-[10px] text-white/40 mb-1">
                <span>${t('admin.progress')}</span>
                <span>${known ? `${pct}%` : ''}</span>
            </div>
            <div class="w-full h-1.5 bg-white/10 rounded-full overflow-hidden" role="progressbar" aria-label="${escapeHtml(t('admin.progress'))}" ${aria}>
                ${bar}
            </div>
        </div>`;
    }

    return `
    <div class="jf-settings-card p-4" id="task-card-${escapeHtml(id)}">
        <div class="flex items-start gap-3">
            <span class="inline-flex flex-shrink-0 mt-0.5">${icon(categoryIcon, { size: 16 })}</span>
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
                        <button class="jf-btn-danger text-xs !py-1.5 !px-3 inline-flex items-center gap-1.5" data-action="stop-task" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}"${isCancelling ? ' disabled' : ''}>
                            ${icon('Square', { size: 12, class: 'fill-current' })}${t('admin.stop')}
                        </button>
                    ` : `
                        <button class="jf-btn-primary text-xs !py-1.5 !px-3 inline-flex items-center gap-1.5" data-action="start-task" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}">
                            ${icon('Play', { size: 12, class: 'fill-current' })}${t('admin.start')}
                        </button>
                    `}
                    <button class="jf-btn-ghost text-xs !py-1.5 !px-3 text-white/40" onclick="refreshTasks()" title="${escapeHtml(t('admin.refresh_status'))}" aria-label="${escapeHtml(t('admin.refresh_status'))}">
                        ${icon('RefreshCw', { size: 14 })}
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

// The tasks in a steady order, by category and then name, so that cards don't move between reloads.
function sortTasks(tasks) {
    const key = task => [task.Category || task.category || '', task.Name || task.name || ''];
    return [...tasks].sort((a, b) => {
        const [ca, na] = key(a);
        const [cb, nb] = key(b);
        return ca.localeCompare(cb) || na.localeCompare(nb);
    });
}

// While the tasks tab is open its list reloads: often while a task runs or stops, slowly otherwise.
const TASK_POLL_BUSY_MS = 1500;
const TASK_POLL_IDLE_MS = 10000;
let taskPollTimer = null;

function tasksBusy(tasks) {
    return tasks.some(task => (task.State || task.state || 'Idle').toLowerCase() !== 'idle');
}

// Whether the tasks tab is on screen: it is on the page and not hidden.
function tasksTabOpen() {
    const tab = $('admin-tasks');
    return !!tab && tab.isConnected && !tab.classList.contains('hidden');
}

function stopTaskPolling() {
    clearTimeout(taskPollTimer);
    taskPollTimer = null;
}

// Plans the next reload, unless the tab has been left.
function scheduleTaskPoll(busy) {
    stopTaskPolling();
    if (!tasksTabOpen()) return;
    taskPollTimer = setTimeout(() => {
        taskPollTimer = null;
        if (tasksTabOpen()) refreshTasks(true);
    }, busy ? TASK_POLL_BUSY_MS : TASK_POLL_IDLE_MS);
}

// Reloads the task list. A reload by the timer (quiet) shows no error toast; it tries again later.
async function refreshTasks(quiet = false) {
    let busy = false;
    try {
        const tasks = await API.getTasks();
        busy = tasksBusy(tasks);
        const list = $('admin-tasks-list');
        if (list) {
            list.innerHTML = sortTasks(tasks).map(task => renderTaskCard(task)).join('');
        }
    } catch (e) {
        if (quiet !== true) toast(t('admin.tasks_load_failed', { message: e.message }), 'error');
    }
    scheduleTaskPoll(busy);
}

// Leaving the page stops the reloads.
window.addEventListener('hashchange', () => {
    if (!tasksTabOpen()) stopTaskPolling();
});

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
    let created;
    try {
        created = await API.createUser(name, password);
    } catch (err) {
        toast(err.message, 'error');
        return;
    }
    // The server creates every user without administrator rights; a ticked box grants them separately.
    if (isAdmin) {
        try {
            await API.updatePolicy(created.Id, { IsAdministrator: true });
            toast(t('admin.user_created', { name }));
        } catch (err) {
            toast(t('admin.user_created_not_admin', { name, error: err.message }), 'error');
        }
    } else {
        toast(t('admin.user_created', { name }));
    }
    closeAddUser();
    API.invalidateCache('/Users');
    if (currentPage === 'admin') renderAdmin();
}

// --- Live Log Functions ---
const LIVELOG_MAX_ROWS = 2000;
const LOG_LEVEL_RANK = { error: 1, warn: 2, info: 3, debug: 4, trace: 5 };
// The view's level; it starts at the server's, the most verbose level its console prints
let liveLogLevel = 'info';
let liveLogInitialized = false;

function initLiveLog() {
    if (typeof LiveLog === 'undefined') return;
    if (!liveLogInitialized) {
        liveLogInitialized = true;
        LiveLog.listeners.push({ add: entry => appendLogEntry(entry), reset: renderLiveLog });
    }
    // Also after a sign-out and back in, which disconnects it
    LiveLog.connect();
    const select = $('livelog-level');
    if (select) select.value = liveLogLevel;
    loadLiveLogLevel();

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

    // The lines received so far
    renderLiveLog();
}

async function loadLiveLogLevel() {
    try {
        const res = await API.getLogLevel();
        if (!LOG_LEVEL_RANK[res?.LogLevel] || res.LogLevel === liveLogLevel) return;
        liveLogLevel = res.LogLevel;
        const select = $('livelog-level');
        if (select) select.value = liveLogLevel;
        renderLiveLog();
    } catch {}
}

// Whether the view's level shows the line
function logShown(entry) {
    const rank = LOG_LEVEL_RANK[String(entry.Level || 'info').toLowerCase()] || LOG_LEVEL_RANK.info;
    return rank <= LOG_LEVEL_RANK[liveLogLevel];
}

// One line's row: time, level, target, and the message with its fields
function logRow(entry) {
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
        <span class="text-white/40 shrink-0">${escapeHtml(entry.Target || '')}</span>
        <span class="text-white/70 break-all whitespace-pre-wrap">${escapeHtml(entry.Message || '')}</span>
    `;
    return line;
}

// Whether the view is scrolled to its end, so that new lines keep it there
function liveLogAtEnd(container) {
    return container.scrollHeight - container.scrollTop - container.clientHeight < 32;
}

// Shows the received lines at the view's level, the newest LIVELOG_MAX_ROWS
function renderLiveLog() {
    const container = $('livelog-container');
    if (!container) return;
    const rows = LiveLog.messages.filter(logShown).slice(-LIVELOG_MAX_ROWS).map(logRow);
    if (!rows.length && !LiveLog.connected) {
        container.innerHTML = `<p class="text-white/30 italic">${t('admin.connecting_to_server')}</p>`;
        return;
    }
    container.replaceChildren(...rows);
    container.scrollTop = container.scrollHeight;
}

function appendLogEntry(entry) {
    const container = $('livelog-container');
    if (!container || !logShown(entry)) return;
    const atEnd = liveLogAtEnd(container);

    // Remove the placeholder on the first line
    if (container.querySelector('p')) {
        container.innerHTML = '';
    }

    container.appendChild(logRow(entry));
    while (container.children.length > LIVELOG_MAX_ROWS) {
        container.removeChild(container.firstChild);
    }

    // Follow new lines, unless the user scrolled up to read
    if (atEnd) {
        container.scrollTop = container.scrollHeight;
    }
}

function clearLiveLog() {
    const container = $('livelog-container');
    if (container) {
        container.innerHTML = `<p class="text-white/30 italic">${t('admin.log_cleared')}</p>`;
    }
    if (typeof LiveLog !== 'undefined') {
        LiveLog.clear();
    }
}

// Shows the lines at `level` and keeps it as the server's live log level
async function setLogLevel(level) {
    if (!LOG_LEVEL_RANK[level]) return;
    liveLogLevel = level;
    renderLiveLog();
    try {
        await API.setLogLevel(level);
        toast(t('admin.log_level_set', { level }));
    } catch (e) {
        toast(t('admin.log_level_failed'), 'error');
    }
}

export {
    renderAdmin, showAdminTab, setUserPolicy, removeOidcLinkAsAdmin, copyOidcCallbackUrl, saveOidcAdminSettings,
    startTaskAction, stopTaskAction, refreshTasks, openAddUser, closeAddUser, submitAddUser, clearLiveLog, setLogLevel,
};
