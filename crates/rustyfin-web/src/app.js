/* Rustyfin Web Client - Main App */
let currentPage = 'home';
let libraries = [];
let wizStep = 0;

// --- Helpers ---
function $(id) { return document.getElementById(id); }
function html(id, c) { const el = $(id); if (el) el.innerHTML = c; }
function show(id) { $(id)?.classList.remove('hidden'); }
function hide(id) { $(id)?.classList.add('hidden'); }

function toast(msg, type = 'success') {
    const c = $('toast-container');
    const div = document.createElement('div');
    div.className = 'jf-toast flex items-center gap-3 px-4 py-3 rounded-lg shadow-lg text-sm ' +
        (type === 'error' ? 'bg-red-500/90 text-white' : 'bg-[#00a4dc]/90 text-white');
    div.innerHTML = `<span>${msg}</span>`;
    c.appendChild(div);
    setTimeout(() => { div.classList.add('removing'); setTimeout(() => div.remove(), 300); }, 3000);
}

// --- Setup Wizard ---
async function checkSetupWizard() {
    try {
        const info = await API.getPublicInfo();
        const done = info.StartupWizardCompleted ?? info.startup_wizard_completed ?? true;
        if (!done) { show('setup-wizard'); hide('main-navbar'); return true; }
    } catch {}
    return false;
}

function wizSetStep(s) {
    wizStep = s;
    for (let i = 0; i < 4; i++) {
        const el = $('wiz-step-' + i);
        if (el) { el.classList.toggle('active', i === s); }
        const dot = $('wiz-dot-' + i);
        if (dot) { dot.style.background = i <= s ? '#00a4dc' : 'rgba(255,255,255,0.2)'; }
    }
    if (s === 3) {
        $('wiz-summary-user').textContent = $('wiz-username').value || 'admin';
        const langEl = $('wiz-lang');
        $('wiz-summary-lang').textContent = langEl.options[langEl.selectedIndex].text;
    }
}

async function wizNext() {
    if (wizStep === 0) {
        try {
            await API.updateStartupConfig({
                preferred_metadata_language: $('wiz-lang').value,
                metadata_country_code: $('wiz-country').value
            });
        } catch {}
    } else if (wizStep === 1) {
        const pw = $('wiz-password').value, pw2 = $('wiz-password2').value;
        const user = $('wiz-username').value.trim();
        if (!user) { $('wiz-user-error').textContent = 'Benutzername erforderlich'; show('wiz-user-error'); return; }
        if (pw && pw !== pw2) { $('wiz-user-error').textContent = 'Passwörter stimmen nicht überein'; show('wiz-user-error'); return; }
        hide('wiz-user-error');
        try { await API.createStartupUser(user, pw || ''); } catch {}
        // Automatisch einloggen, damit nachfolgende Schritte (Library) authentifiziert sind
        try { await API.login(user, pw || ''); } catch (e) { console.warn('Auto-Login im Wizard fehlgeschlagen:', e); }
    } else if (wizStep === 2) {
        const name = $('wiz-lib-name').value.trim();
        const path = $('wiz-lib-path').value.trim();
        if (name && path) {
            if (!API.isLoggedIn()) {
                toast('Bibliothek wird übersprungen – bitte nach dem Setup im Admin-Bereich anlegen', 'error');
            } else {
                try { await API.createLibrary(name, $('wiz-lib-type').value, [path]); }
                catch (e) { toast('Bibliothek konnte nicht angelegt werden: ' + e.message, 'error'); }
            }
        }
    }
    wizSetStep(Math.min(wizStep + 1, 3));
}

function wizPrev() { wizSetStep(Math.max(wizStep - 1, 0)); }

async function wizComplete() {
    try { await API.completeStartup(); } catch {}
    hide('setup-wizard');
    show('main-navbar');
    navigate('login');
}

// --- Navigation ---
function navigate(page, data) {
    document.querySelectorAll('[id^="page-"]').forEach(el => el.classList.add('hidden'));
    const el = $('page-' + page);
    if (el) { el.classList.remove('hidden'); el.classList.add('jf-page'); }
    currentPage = page;
    window.location.hash = data ? `${page}/${data}` : page;
    const pages = { home: renderHome, login: renderLogin, library: () => renderLibrary(data),
        detail: () => renderDetail(data), search: () => renderSearch(data),
        settings: renderSettings, admin: renderAdmin };
    if (pages[page]) pages[page]();
    closeUserMenu();
}

// --- UI ---
function toggleUserMenu() { $('user-dropdown')?.classList.toggle('hidden'); }
function closeUserMenu() { $('user-dropdown')?.classList.add('hidden'); }
document.addEventListener('click', e => {
    if (!e.target.closest('#nav-user-menu')) closeUserMenu();
});

function toggleSidebar() {
    const m = $('sidebar-mobile'), o = $('sidebar-overlay');
    if (m.classList.contains('hidden')) {
        m.classList.remove('hidden');
        o.classList.remove('hidden');
        setTimeout(() => m.style.transform = 'translateX(0)', 10);
        m.innerHTML = '<div class="p-4">' + ($('sidebar')?.querySelector('.p-4')?.innerHTML || '') + '</div>';
    } else {
        m.style.transform = 'translateX(-100%)';
        o.classList.add('hidden');
        setTimeout(() => m.classList.add('hidden'), 200);
    }
}

// --- Item Card ---
function itemCard(item) {
    const name = item.Name || item.name || 'Unbekannt';
    const year = item.ProductionYear || item.production_year || '';
    const type = item.Type || item.type || '';
    const id = item.Id || item.id;
    const played = item.UserData?.Played || item.user_data?.played || false;
    const fav = item.UserData?.IsFavorite || item.user_data?.is_favorite || false;
    const progress = item.UserData?.PlayedPercentage || item.user_data?.played_percentage || 0;
    const imgSrc = API.imageUrl(id, 'Primary', { maxWidth: 300 });
    const canPlay = ['Movie','Episode','Audio','MusicVideo','Video'].includes(type);
    const streamType = type === 'Audio' ? 'Audio' : 'Videos';

    return `<div class="jf-card" onclick="navigate('detail','${id}')">
        <div class="jf-poster">
            <img src="${imgSrc}" alt="${name}" class="w-full h-full object-cover" loading="lazy"
                 onerror="this.style.display='none';this.nextElementSibling.style.display='flex'">
            <div class="hidden items-center justify-center h-full text-4xl text-white/20 absolute inset-0" style="display:none">
                ${type === 'Movie' ? '🎬' : type === 'Series' ? '📺' : type === 'Audio' ? '🎵' : '📁'}
            </div>
            ${canPlay ? `<div class="jf-play-overlay" onclick="event.stopPropagation();playItem('${id}','${streamType}')">
                <div class="w-12 h-12 rounded-full bg-white/20 backdrop-blur flex items-center justify-center">
                    <svg class="w-6 h-6 text-white ml-0.5" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
                </div>
            </div>` : ''}
            ${played ? '<div class="absolute top-2 right-2 w-5 h-5 rounded-full bg-[#00a4dc] flex items-center justify-center"><svg class="w-3 h-3 text-white" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="3" d="M5 13l4 4L19 7"/></svg></div>' : ''}
            ${fav ? '<div class="absolute top-2 left-2 text-red-400 text-sm">♥</div>' : ''}
            ${progress > 0 && progress < 100 ? `<div class="jf-progress"><div class="jf-progress-bar" style="width:${progress}%"></div></div>` : ''}
        </div>
        <div class="p-2.5">
            <p class="text-sm font-medium truncate text-white/90">${name}</p>
            <p class="text-xs text-white/40 mt-0.5">${year}${year && type ? ' · ' : ''}${type}</p>
        </div>
    </div>`;
}

function sectionRow(title, items, moreAction) {
    if (!items || items.length === 0) return '';
    return `<div class="mb-8">
        <div class="flex items-center justify-between mb-3">
            <h2 class="text-xl font-semibold text-white">${title}</h2>
            ${moreAction ? `<button class="text-sm text-[#00a4dc] hover:underline" onclick="${moreAction}">Alle anzeigen</button>` : ''}
        </div>
        <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4">
            ${items.map(itemCard).join('')}
        </div>
    </div>`;
}

// --- Pages ---
async function renderHome() {
    const page = $('page-home');
    if (!API.isLoggedIn()) {
        try {
            const info = await API.getPublicInfo();
            const sn = info.ServerName || info.server_name || 'Rustyfin';
            const ver = info.Version || info.version || '';
            $('nav-version').textContent = ver ? `v${ver}` : '';
            page.innerHTML = `<div class="flex flex-col items-center justify-center min-h-[70vh] text-center jf-page">
                <img src="/web/assets/logo.webp" alt="Rustyfin" class="w-32 h-32 rounded-3xl shadow-2xl mb-6">
                <h1 class="text-4xl font-bold mb-2">${sn}</h1>
                <p class="text-white/50 mb-8 max-w-md">Dein persönlicher Medienserver – schnell, sicher, in Rust gebaut.</p>
                <div class="flex gap-6 mb-8">
                    <div class="text-center"><p class="text-2xl font-bold text-[#00a4dc]">Online</p><p class="text-xs text-white/40">Status</p></div>
                    <div class="text-center"><p class="text-2xl font-bold">${ver}</p><p class="text-xs text-white/40">Version</p></div>
                </div>
                <button class="px-8 py-3 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white font-medium text-lg transition" onclick="navigate('login')">Anmelden</button>
            </div>`;
        } catch { page.innerHTML = '<div class="p-8 text-center text-red-400">Server nicht erreichbar</div>'; }
        return;
    }
    page.innerHTML = '<div class="flex justify-center py-16"><div class="w-8 h-8 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    try {
        const [info, resumeData, latestData] = await Promise.all([
            API.getPublicInfo(), API.getResume(8).catch(() => ({ Items: [] })), API.getLatest(14).catch(() => [])
        ]);
        $('nav-version').textContent = `v${info.Version || info.version || ''}`;
        const resume = resumeData.Items || resumeData.items || [];
        const latest = Array.isArray(latestData) ? latestData : (latestData.Items || latestData.items || []);
        let c = '<div class="jf-page">';
        c += sectionRow('Weiterschauen', resume);
        c += sectionRow('Zuletzt hinzugefügt', latest);
        if (!resume.length && !latest.length) {
            const adminCta = API.isAdmin
                ? '<button class="mt-6 px-6 py-3 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white font-medium transition" onclick="openAddLibrary()">＋ Bibliothek hinzufügen</button>'
                : '<p class="text-sm mt-2">Bitte einen Administrator kontaktieren, um Bibliotheken hinzuzufügen.</p>';
            c += `<div class="text-center py-16 text-white/40"><img src="/web/assets/logo.webp" class="w-16 h-16 mx-auto rounded-xl opacity-30 mb-4"><p class="text-lg">Willkommen! Deine Bibliothek ist noch leer.</p>${adminCta}</div>`;
        }
        c += '</div>';
        page.innerHTML = c;
    } catch (e) { page.innerHTML = `<div class="p-4 rounded-lg bg-red-500/10 text-red-400">${e.message}</div>`; }
}

function renderLogin() {
    html('page-login', `<div class="flex justify-center items-center min-h-[70vh] jf-page">
        <div class="w-full max-w-sm">
            <div class="text-center mb-8">
                <img src="/web/assets/logo.webp" alt="Rustyfin" class="w-20 h-20 mx-auto rounded-2xl shadow-lg mb-4">
                <h2 class="text-2xl font-bold">Anmelden</h2>
                <p class="text-white/40 text-sm mt-1">Melde dich bei deinem Server an</p>
            </div>
            <form id="login-form" onsubmit="handleLogin(event)" class="space-y-4">
                <div>
                    <label class="block text-sm text-white/60 mb-1">Benutzername</label>
                    <input type="text" id="login-user" class="w-full px-4 py-3 rounded-lg bg-white/10 border border-white/10 text-white placeholder-white/30 focus:outline-none focus:border-[#00a4dc] transition" placeholder="Benutzername" required>
                </div>
                <div>
                    <label class="block text-sm text-white/60 mb-1">Passwort</label>
                    <input type="password" id="login-pass" class="w-full px-4 py-3 rounded-lg bg-white/10 border border-white/10 text-white placeholder-white/30 focus:outline-none focus:border-[#00a4dc] transition" placeholder="Passwort" required>
                </div>
                <button type="submit" class="w-full py-3 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white font-medium transition">Einloggen</button>
                <div id="login-error" class="text-red-400 text-sm text-center hidden"></div>
            </form>
        </div>
    </div>`);
}

async function handleLogin(e) {
    e.preventDefault();
    try {
        await API.login($('login-user').value, $('login-pass').value);
        updateAuthUI(); await loadSidebar(); navigate('home');
    } catch (err) { $('login-error').textContent = err.message; show('login-error'); }
}

function doLogout() { API.logout(); updateAuthUI(); navigate('home'); }

async function doSearch(term) { if (term.trim()) navigate('search', encodeURIComponent(term)); }

async function renderSearch(term) {
    term = decodeURIComponent(term || '');
    $('search-input').value = term;
    const page = $('page-search');
    page.innerHTML = '<div class="flex justify-center py-8"><div class="w-8 h-8 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    try {
        const data = await API.search(term, 50);
        const hints = data.SearchHints || data.search_hints || [];
        if (!hints.length) { page.innerHTML = `<div class="text-center py-16 text-white/40 jf-page">Keine Ergebnisse für „${term}"</div>`; return; }
        const items = hints.map(h => ({ Id: h.Id || h.id || h.ItemId, Name: h.Name || h.name, Type: h.Type || h.type, ProductionYear: h.ProductionYear || h.production_year }));
        page.innerHTML = `<div class="jf-page"><h2 class="text-2xl font-bold mb-4">Suchergebnisse für „${term}"</h2>
            <div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4">${items.map(itemCard).join('')}</div></div>`;
    } catch (e) { page.innerHTML = `<div class="p-4 rounded-lg bg-red-500/10 text-red-400">${e.message}</div>`; }
}

async function renderLibrary(libId) {
    const page = $('page-library');
    page.innerHTML = '<div class="flex justify-center py-8"><div class="w-8 h-8 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    try {
        const data = await API.getItems({ LibraryId: libId, Limit: 200, SortBy: 'SortName', SortOrder: 'Ascending' });
        const items = data.Items || data.items || [];
        const total = data.TotalRecordCount || data.total_record_count || items.length;
        const lib = libraries.find(l => (l.id || l.Id) === libId);
        const libName = lib ? (lib.name || lib.Name) : 'Bibliothek';
        page.innerHTML = `<div class="jf-page">
            <div class="flex items-center justify-between mb-6">
                <h2 class="text-2xl font-bold">${libName}</h2>
                <div class="flex items-center gap-3">
                    <span class="text-sm text-white/40">${total} Einträge</span>
                    <button class="px-3 py-1.5 rounded-lg bg-white/10 hover:bg-white/15 text-sm transition" onclick="API.scanLibrary('${libId}').then(()=>toast('Scan gestartet'))">🔄 Scannen</button>
                </div>
            </div>
            ${items.length ? `<div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4">${items.map(itemCard).join('')}</div>` :
                '<div class="text-center py-16 text-white/40">Keine Medien in dieser Bibliothek.</div>'}
        </div>`;
    } catch (e) { page.innerHTML = `<div class="p-4 rounded-lg bg-red-500/10 text-red-400">${e.message}</div>`; }
}

async function renderDetail(itemId) {
    const page = $('page-detail');
    page.innerHTML = '<div class="flex justify-center py-8"><div class="w-8 h-8 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    try {
        const item = await API.getItem(itemId);
        const name = item.Name || item.name || '';
        const overview = item.Overview || item.overview || '';
        const year = item.ProductionYear || item.production_year || '';
        const type = item.Type || item.type || '';
        const runtime = item.RunTimeTicks || item.run_time_ticks;
        const genres = item.Genres || item.genres || [];
        const rating = item.CommunityRating || item.community_rating;
        const played = item.UserData?.Played || item.user_data?.played || false;
        const fav = item.UserData?.IsFavorite || item.user_data?.is_favorite || false;
        const id = item.Id || item.id;
        const imgSrc = API.imageUrl(id, 'Primary', { maxWidth: 400 });
        const backdropSrc = API.imageUrl(id, 'Backdrop', { maxWidth: 1920 });
        const runtimeMin = runtime ? Math.round(runtime / 600000000) : null;
        const canPlay = ['Movie','Episode','Audio','MusicVideo','Video'].includes(type);
        const streamType = type === 'Audio' ? 'Audio' : 'Videos';

        let childrenHtml = '';
        if (['Series','MusicAlbum','Folder','Season'].includes(type)) {
            try {
                const ch = await API.getChildren(id);
                const children = ch.Items || ch.items || [];
                if (children.length) childrenHtml = sectionRow(type === 'Series' ? 'Staffeln' : type === 'Season' ? 'Episoden' : 'Titel', children);
            } catch {}
        }

        let similarHtml = '';
        try {
            const sim = await API.getSimilar(id);
            const similar = sim.Items || sim.items || [];
            if (similar.length) similarHtml = sectionRow('Ähnliche Titel', similar.slice(0, 7));
        } catch {}

        page.innerHTML = `<div class="jf-page">
            <div class="jf-backdrop rounded-xl mb-6 -mx-4 lg:-mx-6 -mt-4 lg:-mt-6">
                <img src="${backdropSrc}" class="w-full h-[400px] object-cover opacity-40" onerror="this.style.display='none'">
                <div class="absolute inset-0" style="background:linear-gradient(to top,#101010 0%,transparent 60%),linear-gradient(to right,#101010 0%,transparent 50%)"></div>
                <div class="relative -mt-48 p-6 lg:p-8 flex flex-col md:flex-row gap-6 z-10">
                    <img src="${imgSrc}" alt="${name}" class="w-44 rounded-lg shadow-2xl self-start hidden md:block" onerror="this.style.display='none'">
                    <div class="flex-1">
                        <h1 class="text-3xl lg:text-4xl font-bold mb-3">${name}</h1>
                        <div class="flex flex-wrap gap-2 mb-4">
                            ${year ? `<span class="jf-badge">${year}</span>` : ''}
                            ${type ? `<span class="jf-badge-primary jf-badge">${type}</span>` : ''}
                            ${runtimeMin ? `<span class="jf-badge">${runtimeMin} Min.</span>` : ''}
                            ${rating ? `<span class="jf-badge">⭐ ${rating.toFixed(1)}</span>` : ''}
                        </div>
                        ${genres.length ? `<div class="flex flex-wrap gap-1.5 mb-4">${genres.map(g => `<span class="text-xs px-2 py-0.5 rounded bg-white/10 text-white/60">${g}</span>`).join('')}</div>` : ''}
                        <div class="flex gap-3 mb-4">
                            ${canPlay ? `<button class="px-6 py-2.5 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white font-medium flex items-center gap-2 transition" onclick="playItem('${id}','${streamType}')">
                                <svg class="w-5 h-5" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg> Abspielen</button>` : ''}
                            <button class="px-4 py-2.5 rounded-lg bg-white/10 hover:bg-white/15 transition" onclick="toggleFavorite('${id}',${fav})">${fav ? '♥' : '♡'}</button>
                            <button class="px-4 py-2.5 rounded-lg bg-white/10 hover:bg-white/15 text-sm transition" onclick="togglePlayed('${id}',${played})">${played ? '✓ Gesehen' : '○ Ungesehen'}</button>
                        </div>
                        ${overview ? `<p class="text-white/70 leading-relaxed max-w-3xl">${overview}</p>` : ''}
                    </div>
                </div>
            </div>
            ${childrenHtml}${similarHtml}
        </div>`;
    } catch (e) { page.innerHTML = `<div class="p-4 rounded-lg bg-red-500/10 text-red-400">${e.message}</div>`; }
}

// --- Player ---
function playItem(itemId, streamType) {
    const url = API.streamUrl(itemId, streamType);
    const video = $('video-player');
    video.src = url;
    $('player-modal').showModal();
    API.reportPlaying(itemId).catch(() => {});
    let interval = setInterval(() => {
        if (!video.paused && !video.ended) {
            API.reportProgress(itemId, Math.round(video.currentTime * 10000000), false).catch(() => {});
        }
    }, 10000);
    const cleanup = () => { clearInterval(interval); API.reportStopped(itemId, Math.round(video.currentTime * 10000000)).catch(() => {}); };
    video.onended = cleanup;
    $('player-modal').addEventListener('close', () => { video.pause(); video.src = ''; cleanup(); }, { once: true });
}

function closePlayer() { const m = $('player-modal'); if (m) { m.close(); } }

async function toggleFavorite(id, cur) {
    try { if (cur) await API.unsetFavorite(id); else await API.setFavorite(id); toast(cur ? 'Favorit entfernt' : 'Als Favorit markiert'); renderDetail(id); } catch (e) { toast(e.message, 'error'); }
}
async function togglePlayed(id, cur) {
    try { if (cur) await API.markUnplayed(id); else await API.markPlayed(id); toast(cur ? 'Als ungesehen markiert' : 'Als gesehen markiert'); renderDetail(id); } catch (e) { toast(e.message, 'error'); }
}

// --- Settings (vollständig) ---
async function renderSettings() {
    const page = $('page-settings');
    page.innerHTML = '<div class="flex justify-center py-8"><div class="w-8 h-8 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    try {
        const [me, config] = await Promise.all([API.getMe(), API.getConfig().catch(() => null)]);
        const name = me.Name || me.name || '';
        const id = me.Id || me.id;

        let serverSettingsHtml = '';
        if (API.isAdmin && config) {
            serverSettingsHtml = `
            <div class="jf-settings-card">
                <h3 class="text-lg font-semibold mb-4 flex items-center gap-2">
                    <svg class="w-5 h-5 text-[#00a4dc]" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 12h14M5 12a2 2 0 01-2-2V6a2 2 0 012-2h14a2 2 0 012 2v4a2 2 0 01-2 2M5 12a2 2 0 00-2 2v4a2 2 0 002 2h14a2 2 0 002-2v-4a2 2 0 00-2-2"/></svg>
                    Server-Konfiguration
                </h3>
                <form id="server-config-form" onsubmit="handleSaveConfig(event)" class="space-y-4">
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                        <div><label class="block text-sm text-white/60 mb-1">Servername</label>
                            <input type="text" id="cfg-server-name" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" value="${config.ServerName || config.server_name || 'Rustyfin'}"></div>
                        <div><label class="block text-sm text-white/60 mb-1">Metadaten-Sprache</label>
                            <select id="cfg-meta-lang" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]">
                                ${['de','en','fr','es','it','ja','zh'].map(l => `<option value="${l}" ${(config.PreferredMetadataLanguage || config.preferred_metadata_language) === l ? 'selected' : ''}>${l}</option>`).join('')}
                            </select></div>
                        <div><label class="block text-sm text-white/60 mb-1">Land</label>
                            <input type="text" id="cfg-country" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" value="${config.MetadataCountryCode || config.metadata_country_code || 'US'}"></div>
                        <div><label class="block text-sm text-white/60 mb-1">HTTP-Port</label>
                            <input type="number" id="cfg-port" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" value="${config.PublicPort || config.public_port || 8096}"></div>
                        <div><label class="block text-sm text-white/60 mb-1">HTTPS-Port</label>
                            <input type="number" id="cfg-https-port" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" value="${config.PublicHttpsPort || config.public_https_port || 8096}"></div>
                        <div><label class="block text-sm text-white/60 mb-1">Log-Aufbewahrung (Tage)</label>
                            <input type="number" id="cfg-log-days" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" value="${config.LogFileRetentionDays || config.log_file_retention_days || 3}"></div>
                    </div>
                    <div class="space-y-3 mt-4">
                        <label class="flex items-center gap-3 cursor-pointer"><input type="checkbox" id="cfg-quick-connect" class="toggle toggle-sm" ${(config.QuickConnectAvailable || config.quick_connect_available) ? 'checked' : ''}><span class="text-sm">Quick Connect aktivieren</span></label>
                        <label class="flex items-center gap-3 cursor-pointer"><input type="checkbox" id="cfg-metrics" class="toggle toggle-sm" ${(config.EnableMetrics ?? config.enable_metrics ?? true) ? 'checked' : ''}><span class="text-sm">Metriken aktivieren</span></label>
                        <label class="flex items-center gap-3 cursor-pointer"><input type="checkbox" id="cfg-auto-restart" class="toggle toggle-sm" ${(config.EnableAutomaticRestart || config.enable_automatic_restart) ? 'checked' : ''}><span class="text-sm">Automatischer Neustart</span></label>
                    </div>
                    <div class="border-t border-white/10 pt-4 mt-4">
                        <h4 class="text-sm font-semibold mb-3 text-white/70">API-Schlüssel (optional)</h4>
                        <div class="grid grid-cols-1 md:grid-cols-2 gap-4">
                            <div><label class="block text-sm text-white/60 mb-1">TMDB API Key</label>
                                <input type="text" id="cfg-tmdb" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" value="${config.TmdbApiKey || config.tmdb_api_key || ''}" placeholder="Optional"></div>
                            <div><label class="block text-sm text-white/60 mb-1">OMDB API Key</label>
                                <input type="text" id="cfg-omdb" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" value="${config.OmdbApiKey || config.omdb_api_key || ''}" placeholder="Optional"></div>
                        </div>
                    </div>
                    <button type="submit" class="px-6 py-2 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white text-sm font-medium transition">Konfiguration speichern</button>
                    <span id="cfg-msg" class="ml-2 text-sm hidden"></span>
                </form>
            </div>`;
        }

        page.innerHTML = `<div class="max-w-3xl mx-auto jf-page">
            <h2 class="text-2xl font-bold mb-6">Einstellungen</h2>

            <div class="jf-settings-card">
                <h3 class="text-lg font-semibold mb-4 flex items-center gap-2">
                    <svg class="w-5 h-5 text-[#00a4dc]" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M16 7a4 4 0 11-8 0 4 4 0 018 0zM12 14a7 7 0 00-7 7h14a7 7 0 00-7-7z"/></svg>
                    Profil
                </h3>
                <div class="flex items-center gap-4 mb-4">
                    <div class="w-16 h-16 rounded-full bg-[#00a4dc] flex items-center justify-center text-2xl font-bold">${name[0]?.toUpperCase() || 'U'}</div>
                    <div><p class="font-semibold text-lg">${name}</p><p class="text-xs text-white/40">ID: ${id}</p></div>
                </div>
            </div>

            <div class="jf-settings-card">
                <h3 class="text-lg font-semibold mb-4 flex items-center gap-2">
                    <svg class="w-5 h-5 text-[#00a4dc]" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M12 15v2m-6 4h12a2 2 0 002-2v-6a2 2 0 00-2-2H6a2 2 0 00-2 2v6a2 2 0 002 2zm10-10V7a4 4 0 00-8 0v4h8z"/></svg>
                    Passwort ändern
                </h3>
                <form onsubmit="handleChangePassword(event,'${id}')" class="space-y-3">
                    <input type="password" id="pw-current" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" placeholder="Aktuelles Passwort" required>
                    <input type="password" id="pw-new" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" placeholder="Neues Passwort" required>
                    <input type="password" id="pw-confirm" class="w-full px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-white text-sm focus:outline-none focus:border-[#00a4dc]" placeholder="Neues Passwort bestätigen" required>
                    <div class="flex items-center gap-3">
                        <button type="submit" class="px-6 py-2 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white text-sm font-medium transition">Speichern</button>
                        <span id="pw-msg" class="text-sm hidden"></span>
                    </div>
                </form>
            </div>

            <div class="jf-settings-card">
                <h3 class="text-lg font-semibold mb-4 flex items-center gap-2">
                    <svg class="w-5 h-5 text-[#00a4dc]" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M7 21a4 4 0 01-4-4V5a2 2 0 012-2h4a2 2 0 012 2v12a4 4 0 01-4 4zm0 0h12a2 2 0 002-2v-4a2 2 0 00-2-2h-2.343M11 7.343l1.657-1.657a2 2 0 012.828 0l2.829 2.829a2 2 0 010 2.828l-8.486 8.485M7 17h.01"/></svg>
                    Darstellung
                </h3>
                <label class="flex items-center gap-3 cursor-pointer">
                    <input type="checkbox" class="toggle toggle-sm" checked onchange="document.documentElement.setAttribute('data-theme',this.checked?'dark':'light')">
                    <span class="text-sm">Dunkles Theme</span>
                </label>
            </div>

            ${serverSettingsHtml}
        </div>`;
    } catch (e) { page.innerHTML = `<div class="p-4 rounded-lg bg-red-500/10 text-red-400">${e.message}</div>`; }
}

async function handleChangePassword(e, userId) {
    e.preventDefault();
    const newPw = $('pw-new').value, confirm = $('pw-confirm').value;
    if (newPw !== confirm) { $('pw-msg').textContent = 'Passwörter stimmen nicht überein'; $('pw-msg').className = 'text-sm text-red-400'; show('pw-msg'); return; }
    try {
        await API.changePassword(userId, $('pw-current').value, newPw);
        $('pw-msg').textContent = '✓ Gespeichert'; $('pw-msg').className = 'text-sm text-green-400'; show('pw-msg');
    } catch (err) { $('pw-msg').textContent = err.message; $('pw-msg').className = 'text-sm text-red-400'; show('pw-msg'); }
}

async function handleSaveConfig(e) {
    e.preventDefault();
    try {
        await API.updateConfig({
            ServerName: $('cfg-server-name').value,
            IsStartupWizardCompleted: true,
            PreferredMetadataLanguage: $('cfg-meta-lang').value,
            MetadataCountryCode: $('cfg-country').value,
            PublicPort: parseInt($('cfg-port').value) || 8096,
            PublicHttpsPort: parseInt($('cfg-https-port').value) || 8096,
            LogFileRetentionDays: parseInt($('cfg-log-days').value) || 3,
            QuickConnectAvailable: $('cfg-quick-connect').checked,
            EnableMetrics: $('cfg-metrics').checked,
            EnableAutomaticRestart: $('cfg-auto-restart').checked,
            EnableNormalizedItemByNameIds: true,
            IsPortAuthorized: true,
            TmdbApiKey: $('cfg-tmdb').value || null,
            OmdbApiKey: $('cfg-omdb').value || null,
        });
        $('cfg-msg').textContent = '✓ Gespeichert'; $('cfg-msg').className = 'ml-2 text-sm text-green-400'; show('cfg-msg');
        toast('Konfiguration gespeichert');
    } catch (err) { $('cfg-msg').textContent = err.message; $('cfg-msg').className = 'ml-2 text-sm text-red-400'; show('cfg-msg'); }
}

// --- Admin ---
async function renderAdmin() {
    const page = $('page-admin');
    if (!API.isAdmin) { page.innerHTML = '<div class="p-4 rounded-lg bg-yellow-500/10 text-yellow-400">Kein Zugriff</div>'; return; }
    page.innerHTML = '<div class="flex justify-center py-8"><div class="w-8 h-8 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    try {
        const [users, libs, tasks, plugins, info, sessions] = await Promise.all([
            API.getUsers().catch(() => []), API.getLibraries().catch(() => []),
            API.getTasks().catch(() => []), API.getPlugins().catch(() => []),
            API.getFullInfo().catch(() => ({})), API.getSessions().catch(() => [])
        ]);

        const tabs = ['Übersicht','Benutzer','Bibliotheken','Aufgaben','Plugins','Sitzungen','Aktivität'];
        const tabIds = ['admin-overview','admin-users','admin-libs','admin-tasks','admin-plugins','admin-sessions','admin-activity'];

        page.innerHTML = `<div class="max-w-5xl mx-auto jf-page">
            <h2 class="text-2xl font-bold mb-6">Administration</h2>
            <div class="flex gap-1 mb-6 overflow-x-auto pb-2">
                ${tabs.map((t, i) => `<button class="px-4 py-2 rounded-lg text-sm whitespace-nowrap transition ${i === 0 ? 'bg-[#00a4dc] text-white' : 'bg-white/10 text-white/60 hover:bg-white/15'}" onclick="showAdminTab(this,'${tabIds[i]}')">${t}</button>`).join('')}
            </div>

            <div id="admin-overview">
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4 mb-6">
                    <div class="jf-settings-card text-center"><p class="text-2xl font-bold text-[#00a4dc]">${info.ServerName || info.server_name || 'Rustyfin'}</p><p class="text-xs text-white/40 mt-1">Server</p></div>
                    <div class="jf-settings-card text-center"><p class="text-2xl font-bold">${info.Version || info.version || ''}</p><p class="text-xs text-white/40 mt-1">Version</p></div>
                    <div class="jf-settings-card text-center"><p class="text-2xl font-bold">${users.length}</p><p class="text-xs text-white/40 mt-1">Benutzer</p></div>
                    <div class="jf-settings-card text-center"><p class="text-2xl font-bold">${libs.length}</p><p class="text-xs text-white/40 mt-1">Bibliotheken</p></div>
                </div>
                <div class="jf-settings-card">
                    <h3 class="font-semibold mb-3">System-Info</h3>
                    <div class="grid grid-cols-2 gap-2 text-sm">
                        <p class="text-white/50">Betriebssystem</p><p>${info.OperatingSystem || info.operating_system || '–'}</p>
                        <p class="text-white/50">Datenverzeichnis</p><p class="text-xs break-all">${info.ProgramDataPath || info.program_data_path || '–'}</p>
                        <p class="text-white/50">Log-Verzeichnis</p><p class="text-xs break-all">${info.LogPath || info.log_path || '–'}</p>
                    </div>
                    <div class="flex gap-2 mt-4">
                        <button class="px-4 py-2 rounded-lg bg-yellow-500/20 text-yellow-400 text-sm hover:bg-yellow-500/30 transition" onclick="API.restart().then(()=>toast('Neustart…'))">🔄 Neustart</button>
                        <button class="px-4 py-2 rounded-lg bg-red-500/20 text-red-400 text-sm hover:bg-red-500/30 transition" onclick="if(confirm('Server herunterfahren?'))API.shutdown().then(()=>toast('Herunterfahren…'))">⏹ Herunterfahren</button>
                        <button class="px-4 py-2 rounded-lg bg-white/10 text-sm hover:bg-white/15 transition" onclick="API.scanAllLibraries().then(()=>toast('Scan gestartet'))">📚 Alle scannen</button>
                    </div>
                </div>
            </div>

            <div id="admin-users" class="hidden">
                <div class="jf-settings-card">
                    <div class="overflow-x-auto"><table class="w-full text-sm">
                        <thead><tr class="text-left text-white/40 border-b border-white/10"><th class="pb-3">Name</th><th class="pb-3">Rolle</th><th class="pb-3">Letzter Login</th><th class="pb-3">Aktionen</th></tr></thead>
                        <tbody>${users.map(u => `<tr class="border-b border-white/5">
                            <td class="py-3">${u.Name || u.name}</td>
                            <td class="py-3">${(u.Policy?.IsAdministrator || u.policy?.is_administrator) ? '<span class="text-xs px-2 py-0.5 rounded bg-[#00a4dc]/20 text-[#00a4dc]">Admin</span>' : '<span class="text-xs text-white/40">Benutzer</span>'}</td>
                            <td class="py-3 text-xs text-white/40">${u.LastLoginDate || u.last_login_date || '–'}</td>
                            <td class="py-3"><button class="text-xs text-red-400 hover:underline" onclick="if(confirm('Benutzer löschen?'))API.deleteUser('${u.Id || u.id}').then(()=>renderAdmin())">Löschen</button></td>
                        </tr>`).join('')}</tbody>
                    </table></div>
                    <div class="border-t border-white/10 pt-4 mt-4">
                        <h4 class="font-semibold text-sm mb-3">Neuer Benutzer</h4>
                        <form class="flex gap-2 flex-wrap" onsubmit="event.preventDefault();API.createUser(this.n.value,this.p.value).then(()=>{toast('Erstellt');renderAdmin()}).catch(e=>toast(e.message,'error'))">
                            <input name="n" class="px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-sm text-white" placeholder="Name" required>
                            <input name="p" type="password" class="px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-sm text-white" placeholder="Passwort" required>
                            <button class="px-4 py-2 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white text-sm transition">Erstellen</button>
                        </form>
                    </div>
                </div>
            </div>

            <div id="admin-libs" class="hidden">
                <div class="jf-settings-card">
                    <div class="flex items-center justify-between mb-4">
                        <h3 class="font-semibold">Bibliotheken (${libs.length})</h3>
                        <button class="px-4 py-2 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white text-sm font-medium transition flex items-center gap-2" onclick="openAddLibrary()">
                            <span class="text-lg leading-none">＋</span><span>Neue Bibliothek</span>
                        </button>
                    </div>
                    <div class="overflow-x-auto"><table class="w-full text-sm">
                        <thead><tr class="text-left text-white/40 border-b border-white/10"><th class="pb-3">Name</th><th class="pb-3">Typ</th><th class="pb-3">Pfade</th><th class="pb-3">Aktionen</th></tr></thead>
                        <tbody>${libs.map(l => `<tr class="border-b border-white/5">
                            <td class="py-3">${l.name || l.Name}</td>
                            <td class="py-3"><span class="text-xs px-2 py-0.5 rounded bg-white/10">${l.content_type || l.ContentType || ''}</span></td>
                            <td class="py-3 text-xs text-white/40">${(l.paths || l.Paths || []).join(', ')}</td>
                            <td class="py-3 flex gap-2">
                                <button class="text-xs text-[#00a4dc] hover:underline" onclick="API.scanLibrary('${l.id || l.Id}').then(()=>toast('Scan gestartet'))">Scannen</button>
                                <button class="text-xs text-red-400 hover:underline" onclick="if(confirm('Löschen?'))API.deleteLibrary('${l.id || l.Id}').then(()=>renderAdmin())">Löschen</button>
                            </td>
                        </tr>`).join('')}</tbody>
                    </table></div>
                    <div class="border-t border-white/10 pt-4 mt-4">
                        <h4 class="font-semibold text-sm mb-3">Neue Bibliothek</h4>
                        <form class="flex gap-2 flex-wrap" onsubmit="event.preventDefault();API.createLibrary(this.n.value.trim(),this.t.value,this.p.value.split(',').map(p=>p.trim()).filter(Boolean)).then(()=>{toast('Bibliothek erstellt');renderAdmin()}).catch(e=>toast(e.message,'error'))">
                            <input name="n" class="px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-sm text-white" placeholder="Name" required>
                            <select name="t" class="px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-sm text-white"><option value="Movies">Filme</option><option value="TvShows">Serien</option><option value="Music">Musik</option><option value="Photos">Fotos</option><option value="Mixed">Gemischt</option></select>
                            <input name="p" class="px-3 py-2 rounded-lg bg-white/10 border border-white/10 text-sm text-white flex-1" placeholder="Pfade (kommagetrennt)" required>
                            <button class="px-4 py-2 rounded-lg bg-[#00a4dc] hover:bg-[#0090c4] text-white text-sm transition">Erstellen</button>
                        </form>
                    </div>
                </div>
            </div>

            <div id="admin-tasks" class="hidden">
                <div class="jf-settings-card">
                    <div class="overflow-x-auto"><table class="w-full text-sm">
                        <thead><tr class="text-left text-white/40 border-b border-white/10"><th class="pb-3">Aufgabe</th><th class="pb-3">Status</th><th class="pb-3">Letzte Ausführung</th></tr></thead>
                        <tbody>${tasks.map(t => `<tr class="border-b border-white/5">
                            <td class="py-3">${t.Name || t.name || t.Id || t.id}</td>
                            <td class="py-3"><span class="text-xs px-2 py-0.5 rounded ${(t.State || t.state) === 'Running' ? 'bg-yellow-500/20 text-yellow-400' : 'bg-green-500/20 text-green-400'}">${t.State || t.state || 'Idle'}</span></td>
                            <td class="py-3 text-xs text-white/40">${t.LastExecutionResult?.EndTimeUtc || t.last_execution_result?.end_time_utc || '–'}</td>
                        </tr>`).join('')}</tbody>
                    </table></div>
                </div>
            </div>

            <div id="admin-plugins" class="hidden">
                ${plugins.length ? `<div class="grid grid-cols-1 md:grid-cols-2 gap-4">${plugins.map(p => `<div class="jf-settings-card">
                    <h4 class="font-semibold">${p.Name || p.name}</h4>
                    <p class="text-sm text-white/50 mt-1">${p.Description || p.description || ''}</p>
                    <span class="text-xs text-white/30 mt-2 inline-block">${p.Version || p.version || ''}</span>
                </div>`).join('')}</div>` : '<div class="jf-settings-card text-center text-white/40">Keine Plugins installiert.</div>'}
            </div>

            <div id="admin-sessions" class="hidden">
                <div class="jf-settings-card">
                    <div class="overflow-x-auto"><table class="w-full text-sm">
                        <thead><tr class="text-left text-white/40 border-b border-white/10"><th class="pb-3">Benutzer</th><th class="pb-3">Gerät</th><th class="pb-3">Client</th><th class="pb-3">Letzte Aktivität</th></tr></thead>
                        <tbody>${sessions.map(s => `<tr class="border-b border-white/5">
                            <td class="py-3">${s.UserName || s.user_name || '–'}</td>
                            <td class="py-3">${s.DeviceName || s.device_name || '–'}</td>
                            <td class="py-3">${s.Client || s.client || '–'}</td>
                            <td class="py-3 text-xs text-white/40">${s.LastActivityDate || s.last_activity_date || '–'}</td>
                        </tr>`).join('')}</tbody>
                    </table></div>
                </div>
            </div>

            <div id="admin-activity" class="hidden">
                <div class="jf-settings-card" id="activity-content"><p class="text-white/40">Lade Aktivitätslog…</p></div>
            </div>
        </div>`;

    } catch (e) { page.innerHTML = `<div class="p-4 rounded-lg bg-red-500/10 text-red-400">${e.message}</div>`; }
}

function showAdminTab(el, tabId) {
    el.parentElement.querySelectorAll('button').forEach(b => { b.className = 'px-4 py-2 rounded-lg text-sm whitespace-nowrap transition bg-white/10 text-white/60 hover:bg-white/15'; });
    el.className = 'px-4 py-2 rounded-lg text-sm whitespace-nowrap transition bg-[#00a4dc] text-white';
    ['admin-overview','admin-users','admin-libs','admin-tasks','admin-plugins','admin-sessions','admin-activity'].forEach(id => {
        const e = $(id); if (e) e.classList.toggle('hidden', id !== tabId);
    });
    if (tabId === 'admin-activity') loadActivityLog();
}

async function loadActivityLog() {
    try {
        const data = await API.getActivityLog(0, 50);
        const items = data.Items || data.items || [];
        if (!items.length) { html('activity-content', '<p class="text-white/40">Keine Aktivitäten.</p>'); return; }
        html('activity-content', `<div class="overflow-x-auto"><table class="w-full text-sm">
            <thead><tr class="text-left text-white/40 border-b border-white/10"><th class="pb-3">Zeitpunkt</th><th class="pb-3">Typ</th><th class="pb-3">Beschreibung</th></tr></thead>
            <tbody>${items.map(a => `<tr class="border-b border-white/5">
                <td class="py-2 text-xs text-white/40">${a.Date || a.date || '–'}</td>
                <td class="py-2 text-xs">${a.Type || a.type || '–'}</td>
                <td class="py-2 text-sm">${a.ShortOverview || a.short_overview || a.Name || a.name || '–'}</td>
            </tr>`).join('')}</tbody>
        </table></div>`);
    } catch (e) { html('activity-content', `<p class="text-red-400">${e.message}</p>`); }
}

// --- Sidebar ---
async function loadSidebar() {
    if (!API.isLoggedIn()) { hide('sidebar'); return; }
    show('sidebar');
    try {
        libraries = await API.getLibraries();
        const list = $('sidebar-lib-list');
        if (!list) return;
        list.innerHTML = '';
        const icons = { movies: '🎬', tvshows: '📺', music: '🎵', photos: '🖼️', mixed: '📁',
                        Movies: '🎬', TvShows: '📺', Music: '🎵', Photos: '🖼️', Mixed: '📁' };
        libraries.forEach(lib => {
            const id = lib.id || lib.Id;
            const name = lib.name || lib.Name;
            const type = lib.content_type || lib.ContentType || 'mixed';
            list.innerHTML += `<a onclick="navigate('library','${id}')">${icons[type] || '📁'} ${name}</a>`;
        });
        if (!libraries.length) {
            list.innerHTML = '<p class="text-xs text-white/30 px-1 py-2">Keine Bibliotheken</p>';
        }
        if (API.isAdmin) {
            show('sidebar-add-lib');
            show('sidebar-add-lib-row');
        }
    } catch {}
}

// --- Add Library Modal ---
function openAddLibrary() {
    if (!API.isAdmin) { toast('Nur Administratoren können Bibliotheken anlegen', 'error'); return; }
    const dlg = $('add-library-modal');
    if (!dlg) return;
    const form = $('add-library-form');
    if (form) form.reset();
    if (typeof dlg.showModal === 'function') dlg.showModal();
    else dlg.setAttribute('open', '');
}

function closeAddLibrary() {
    const dlg = $('add-library-modal');
    if (!dlg) return;
    if (typeof dlg.close === 'function') dlg.close();
    else dlg.removeAttribute('open');
}

async function submitAddLibrary(e) {
    e.preventDefault();
    const f = e.target;
    const name = f.name.value.trim();
    const type = f.type.value;
    const paths = f.paths.value.split(',').map(p => p.trim()).filter(Boolean);
    if (!name || !paths.length) { toast('Name und mindestens ein Pfad erforderlich', 'error'); return; }
    try {
        await API.createLibrary(name, type, paths);
        toast('Bibliothek erstellt');
        closeAddLibrary();
        await loadSidebar();
        if (currentPage === 'admin') renderAdmin();
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- Auth UI ---
function updateAuthUI() {
    if (API.isLoggedIn()) {
        hide('nav-login-btn');
        show('nav-user-menu');
        $('nav-avatar-letter').textContent = (API.userName || 'U')[0].toUpperCase();
        $('nav-user-name').textContent = API.userName || '';
        $('dropdown-username').textContent = API.userName || '';
        if (API.isAdmin) { show('nav-admin-link'); show('nav-add-lib'); }
    } else {
        show('nav-login-btn');
        hide('nav-user-menu');
        hide('nav-add-lib');
        hide('sidebar');
    }
}

// --- Hash routing ---
function handleHash() {
    const hash = window.location.hash.slice(1);
    if (!hash) { navigate('home'); return; }
    const [page, ...rest] = hash.split('/');
    navigate(page, rest.join('/') || undefined);
}

// --- Init ---
(async () => {
    const needsSetup = await checkSetupWizard();
    if (!needsSetup) {
        updateAuthUI();
        if (API.isLoggedIn()) loadSidebar();
        window.addEventListener('hashchange', handleHash);
        handleHash();
    }
})();
