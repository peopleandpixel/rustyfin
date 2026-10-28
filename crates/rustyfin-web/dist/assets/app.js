/* Rustyfin Web Client - Jellyfin 12.x Architecture */
let currentPage = 'home';
let currentLibraryId = null;
let currentDetailId = null;
let libraries = [];
let wizStep = 0;
let serverVersion = '0.3.0';
let playerReportInterval = null;
let playerHideTimeout = null;
let activeItem = null;
// Track which libraries are currently scanning (libId -> true/false)
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

function initLazyImages() {
    // Hover-based full-res image loading: swap thumbnail for full-res on card hover
    document.querySelectorAll('img.jf-thumb-img[data-full-src]').forEach(img => {
        if (img.dataset.fullLoaded) return;
        const card = img.closest('.jf-card');
        if (!card) return;

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
    return String(str).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function formatRuntime(ticks) {
    if (!ticks) return '';
    const totalMinutes = Math.round(ticks / 600000000);
    if (totalMinutes < 60) return `${totalMinutes} Min.`;
    const hours = Math.floor(totalMinutes / 60);
    const mins = totalMinutes % 60;
    return `${hours} Std. ${mins > 0 ? mins + ' Min.' : ''}`;
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
        if (info.Version || info.version) {
            serverVersion = info.Version || info.version;
            const badge = $('version-badge');
            if (badge) badge.textContent = serverVersion;
        }
        if (!done) {
            show('setup-wizard');
            hide('main-navbar');
            return true;
        }
    } catch {}
    return false;
}

function wizSetStep(s) {
    wizStep = s;
    for (let i = 0; i < 4; i++) {
        const el = $('wiz-step-' + i);
        if (el) el.classList.toggle('active', i === s);
        const dot = $('wiz-dot-' + i);
        if (dot) {
            dot.className = i === s
                ? 'h-2 rounded-full transition-all duration-300 w-8 bg-[#00a4dc]'
                : (i < s ? 'h-2 rounded-full transition-all duration-300 w-4 bg-[#00a4dc]/50' : 'h-2 rounded-full transition-all duration-300 w-2 bg-white/20');
        }
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
        if (!user) {
            $('wiz-user-error').textContent = 'Benutzername ist erforderlich.';
            show('wiz-user-error');
            return;
        }
        if (pw && pw !== pw2) {
            $('wiz-user-error').textContent = 'Passwörter stimmen nicht überein.';
            show('wiz-user-error');
            return;
        }
        hide('wiz-user-error');
        try { await API.createStartupUser(user, pw || ''); } catch {}
        try { await API.login(user, pw || ''); } catch (e) { console.warn('Auto-Login im Wizard:', e); }
    } else if (wizStep === 2) {
        const name = $('wiz-lib-name').value.trim();
        const path = $('wiz-lib-path').value.trim();
        if (name && path) {
            if (API.isLoggedIn()) {
                try {
                    await API.createLibrary(name, $('wiz-lib-type').value, [path]);
                } catch (e) {
                    toast('Bibliothek konnte nicht erstellt werden: ' + e.message, 'error');
                }
            }
        }
    }
    wizSetStep(Math.min(wizStep + 1, 3));
}

function wizPrev() {
    wizSetStep(Math.max(wizStep - 1, 0));
}

async function wizComplete() {
    try { await API.completeStartup(); } catch {}
    hide('setup-wizard');
    show('main-navbar');
    navigate('login');
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
        login: renderLogin,
        library: () => renderLibrary(data),
        detail: () => renderDetail(data),
        search: () => renderSearch(data),
        person: () => renderPerson(data),
        settings: renderSettings,
        admin: renderAdmin
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
function itemCard(item, options = {}) {
    const name = item.Name || item.name || 'Unbekannt';
    const year = item.ProductionYear || item.production_year || '';
    const type = item.Type || item.type || '';
    const id = item.Id || item.id;
    const played = item.UserData?.Played || item.user_data?.played || false;
    const fav = item.UserData?.IsFavorite || item.user_data?.is_favorite || false;
    const progress = item.UserData?.PlayedPercentage || item.user_data?.played_percentage || 0;
    const rating = item.CommunityRating || item.community_rating;
    // Use explicit backdrop option if provided; otherwise derive from type
    const isBackdrop = options.backdrop !== undefined ? options.backdrop : type === 'Episode';

    // Progressive image loading: thumbnail for fast first paint,
    // full-res loaded on hover only to save bandwidth
    const thumbSrc = isBackdrop
        ? API.imageUrl(id, 'Backdrop', { maxWidth: 150, format: 'webp' })
        : API.imageUrl(id, 'Primary', { maxWidth: 150, format: 'webp' });
    const fullSrc = isBackdrop
        ? API.imageUrl(id, 'Backdrop', { maxWidth: 250 })
        : API.imageUrl(id, 'Primary', { maxWidth: 250 });

    const canPlay = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video'].includes(type);
    const streamType = type === 'Audio' ? 'Audio' : 'Videos';

    // Type icons
    const typeIcons = {
        Movie: '🎬', Series: '📺', Episode: '📺', Audio: '🎵', MusicAlbum: '💿',
        MusicArtist: '🎤', Folder: '📁', CollectionFolder: '🗂️', Photo: '🖼️'
    };

    return `
    <div class="jf-card group" onclick="navigate('detail', '${id}')">
        <div class="${isBackdrop ? 'jf-poster-backdrop' : 'jf-poster'}">
            <div class="jf-poster-skeleton"></div>
            <!-- Thumbnail loads first for instant visual feedback; full-res swaps on hover -->
            <img src="${thumbSrc}" alt="${escapeHtml(name)}" decoding="async" class="jf-thumb-img absolute inset-0 w-full h-full object-cover z-[1] opacity-0 blur-sm scale-105 transition-opacity duration-200"
                 data-full-src="${fullSrc}"
                 onload="this.style.opacity='0.8';this.classList.remove('blur-sm','scale-105');const sk=this.previousElementSibling;if(sk&&sk.classList.contains('jf-poster-skeleton'))sk.style.display='none';"
                 onerror="this.style.display='none';const sk=this.previousElementSibling;if(sk&&sk.classList.contains('jf-poster-skeleton'))sk.style.display='none';">
            <div class="hidden items-center justify-center h-full text-4xl text-white/20 absolute inset-0 bg-white/5 z-[3]" style="display:none">
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
            <div class="jf-play-overlay" onclick="event.stopPropagation();playItem('${id}','${streamType}','${escapeHtml(name)}')">
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
            <p class="text-xs sm:text-sm font-semibold truncate text-white group-hover:text-[#38bdf8] transition-colors" title="${escapeHtml(name)}">${escapeHtml(name)}</p>
            <div class="flex items-center gap-1.5 text-[11px] text-white/40 mt-0.5">
                ${year ? `<span>${year}</span>` : ''}
                ${year && type ? `<span>·</span>` : ''}
                <span class="truncate">${type}</span>
            </div>
        </div>
    </div>`;
}

// --- Create a single card DOM element (avoids innerHTML parsing overhead) ---
function createCardElement(item, wrapperClass = '', cardOptions = {}) {
    const wrapper = document.createElement('div');
    wrapper.className = wrapperClass;
    wrapper.innerHTML = itemCard(item, cardOptions);
    return wrapper.firstElementChild;
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
        ${moreAction ? `<button class="text-xs font-semibold text-[#00a4dc] hover:text-[#38bdf8] flex items-center gap-1 transition" onclick="${moreAction}">Alle ansehen →</button>` : ''}
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
    const page = $('page-home');
    if (!API.isLoggedIn()) {
        try {
            const info = await API.getPublicInfo();
            const sn = info.ServerName || info.server_name || 'Rustyfin';
            const ver = info.Version || info.version || serverVersion;
            page.innerHTML = `
            <div class="flex flex-col items-center justify-center min-h-[75vh] text-center jf-page px-4">
                <div class="relative mb-6">
                    <img src="/web/assets/logo.png" alt="Rustyfin" class="w-32 h-32 rounded-3xl shadow-2xl p-2 bg-gradient-to-tr from-white/10 to-transparent border border-white/10">
                    <span class="absolute -bottom-2 right-2 px-2.5 py-0.5 rounded-full text-[10px] font-bold bg-[#00a4dc] text-white shadow-lg">v${ver}</span>
                </div>
                <h1 class="text-4xl sm:text-5xl font-black mb-3 tracking-tight text-white">${escapeHtml(sn)}</h1>
                <p class="text-white/60 text-base max-w-lg mb-8 leading-relaxed">
                    Ultraschneller, ressourcenschonender Medienserver in Rust mit nativer Jellyfin-Kompatibilität.
                </p>
                <div class="grid grid-cols-3 gap-6 mb-10 w-full max-w-md">
                    <div class="p-4 rounded-xl bg-white/5 border border-white/10 text-center">
                        <p class="text-xl font-bold text-emerald-400">Online</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">Status</p>
                    </div>
                    <div class="p-4 rounded-xl bg-white/5 border border-white/10 text-center">
                        <p class="text-xl font-bold text-[#00a4dc]">&lt; 20 MB</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">RAM Idle</p>
                    </div>
                    <div class="p-4 rounded-xl bg-white/5 border border-white/10 text-center">
                        <p class="text-xl font-bold text-amber-400">v${ver}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">Release</p>
                    </div>
                </div>
                <button class="jf-btn-primary !px-8 !py-3.5 text-base shadow-xl" onclick="navigate('login')">
                    Jetzt Anmelden →
                </button>
            </div>`;
        } catch {
            page.innerHTML = '<div class="p-10 text-center text-red-400 jf-page">Server nicht erreichbar. Bitte Netzwerkverbindung prüfen.</div>';
        }
        return;
    }

    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        const [resumeData, nextUpData, latestData, libsData] = await Promise.all([
            (API.getResume ? API.getResume(12) : Promise.resolve({ Items: [] })).catch(() => ({ Items: [] })),
            (API.getNextUp ? API.getNextUp(12) : Promise.resolve({ Items: [] })).catch(() => ({ Items: [] })),
            (API.getLatest ? API.getLatest(20) : Promise.resolve([])).catch(() => []),
            (API.getLibraries ? API.getLibraries() : Promise.resolve([])).catch(() => [])
        ]);

        const resume = resumeData.Items || resumeData.items || [];
        const nextUp = nextUpData.Items || nextUpData.items || [];
        const latest = Array.isArray(latestData) ? latestData : (latestData.Items || latestData.items || []);
        libraries = libsData || [];

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
            const spBackdrop = API.imageUrl(spId, 'Backdrop', { maxWidth: 1920 });
            const canPlay = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video'].includes(spType);
            const streamType = spType === 'Audio' ? 'Audio' : 'Videos';

            c += `
            <div class="jf-spotlight mb-10">
                <img src="${spBackdrop}" class="jf-spotlight-backdrop" onerror="this.style.display='none'">
                <div class="jf-spotlight-gradient"></div>
                <div class="relative z-10 p-6 sm:p-10 lg:p-14 flex flex-col justify-end min-h-[420px] max-w-3xl">
                    <div class="flex items-center gap-2 mb-3">
                        <span class="jf-badge jf-badge-accent uppercase tracking-wider text-[11px]">Highlights</span>
                        <span class="jf-badge jf-badge-primary uppercase tracking-wider text-[11px]">${spType}</span>
                        ${spYear ? `<span class="jf-badge text-[11px]">${spYear}</span>` : ''}
                        ${spRating ? `<span class="jf-badge jf-badge-rating text-[11px]">★ ${spRating.toFixed(1)}</span>` : ''}
                    </div>
                    <h1 class="text-3xl sm:text-4xl lg:text-5xl font-black text-white tracking-tight mb-3 drop-shadow-md">${escapeHtml(spName)}</h1>
                    ${spOverview ? `<p class="text-white/80 text-xs sm:text-sm line-clamp-3 mb-6 max-w-2xl leading-relaxed">${escapeHtml(spOverview)}</p>` : ''}
                    <div class="flex flex-wrap items-center gap-3">
                        ${canPlay ? `
                        <button class="jf-btn-primary !py-2.5 !px-6 text-sm" onclick="playItem('${spId}','${streamType}','${escapeHtml(spName)}')">
                            <svg class="w-4 h-4" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
                            Abspielen
                        </button>` : ''}
                        <button class="jf-btn-ghost !py-2.5 !px-5 text-sm" onclick="navigate('detail','${spId}')">
                            Details & Info
                        </button>
                    </div>
                </div>
            </div>`;
        }

        // 2. "Meine Medien" Library Tiles
        if (libraries.length > 0) {
            c += `
            <div class="mb-10">
                <div class="flex items-center gap-2 mb-4">
                    <span class="w-2.5 h-2.5 rounded-full bg-[#00a4dc]"></span>
                    <h2 class="text-lg lg:text-xl font-bold tracking-tight text-white">Meine Bibliotheken</h2>
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
                        <div class="jf-card p-5 bg-gradient-to-br ${gradients[libType] || gradients.Mixed} flex flex-col justify-between min-h-[120px] relative" onclick="navigate('library','${libId}')">
                            <div class="flex items-center justify-between">
                                <span class="text-3xl">${icons[libType] || '📁'}</span>
                                <div class="flex items-center gap-1">
                                    ${scanStatuses[libId] ? `<span class="w-2 h-2 rounded-full bg-amber-400 animate-pulse" title="Scan läuft"></span>` : ''}
                                    <button type="button" class="w-6 h-6 rounded flex items-center justify-center text-white/40 hover:text-white hover:bg-white/10 transition" onclick="event.stopPropagation();API.scanLibrary('${libId}').then(()=>toast('Scan für ${escapeHtml(libName)} gestartet'))" title="Bibliothek scannen">
                                        <svg class="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/></svg>
                                    </button>
                                </div>
                            </div>
                            <div>
                                <p class="font-bold text-white text-base truncate">${escapeHtml(libName)}</p>
                            </div>
                        </div>`;
                    }).join('')}
                </div>
            </div>`;
        }

        // 3. Sections: Continue Watching & Next Up & Latest
        c += sectionRow('Weiterschauen', resume, null, { backdrop: true });
        c += sectionRow('Als Nächstes (Serien)', nextUp, null, { backdrop: true });
        c += sectionRow('Zuletzt hinzugefügt', latest);

        // Empty Library Welcome Banner
        if (!resume.length && !nextUp.length && !latest.length) {
            c += `
            <div class="text-center py-20 px-4 rounded-2xl bg-white/5 border border-white/10 my-8">
                <img src="/web/assets/logo.png" class="w-20 h-20 mx-auto rounded-2xl opacity-40 mb-4 p-1 bg-white/5">
                <h3 class="text-xl font-bold text-white mb-2">Deine Bibliothek ist noch leer</h3>
                <p class="text-white/50 text-sm max-w-md mx-auto mb-6">Füge deine ersten Medienordner hinzu, um Filme, Serien und Musik auf allen Geräten zu streamen.</p>
                ${API.isAdmin ? `
                <button class="jf-btn-primary" onclick="openAddLibrary()">
                    ＋ Erste Bibliothek anlegen
                </button>` : ''}
            </div>`;
        }

        c += '</div>';
        page.innerHTML = c;
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 border border-red-500/20 text-red-400 jf-page">Fehler beim Laden der Startseite: ${escapeHtml(e.message)}</div>`;
    }
}

// --- Favorites Page Renderer (with pagination) ---
async function renderFavorites() {
    favoritesPage = 0;
    destroyInfiniteScroll();
    const page = $('page-favorites');
    // Show skeleton grid immediately
    page.innerHTML = `
    <div class="jf-page">
        <div class="flex items-center gap-3 mb-6">
            <span class="w-3 h-3 rounded-full bg-rose-500 shadow-md"></span>
            <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">Meine Favoriten</h1>
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
        const items = data.Items || data.items || [];
        const total = data.TotalRecordCount || data.total_record_count || items.length;
        const hasMore = items.length >= LIBRARY_PAGE_SIZE;

        if (items.length > 0) {
            page.innerHTML = `
            <div class="jf-page">
                <div class="flex items-center gap-3 mb-6">
                    <span class="w-3 h-3 rounded-full bg-rose-500 shadow-md"></span>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">Meine Favoriten</h1>
                    <span class="text-xs text-white/40">(${total})</span>
                </div>
                <div id="fav-grid-container"></div>
                ${hasMore ? `
                <div id="fav-infinite-container" class="text-center mt-8">
                    <div id="infinite-scroll-spinner" class="jf-infinite-spinner" style="display:none"><div class="jf-spinner"></div></div>
                    <div id="infinite-scroll-sentinel" class="jf-infinite-sentinel"></div>
                </div>` : ''}
            </div>`;
            // Initialize virtual grid for favorites
            if (virtualGrid) { virtualGrid.destroy(); virtualGrid = null; }
            virtualGrid = new VirtualGrid('fav-grid-container');
            virtualGrid.init();
            virtualGrid.setItems(items);
        } else {
            page.innerHTML = `
            <div class="jf-page">
                <div class="flex items-center gap-3 mb-6">
                    <span class="w-3 h-3 rounded-full bg-rose-500 shadow-md"></span>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">Meine Favoriten</h1>
                    <span class="text-xs text-white/40">(${total})</span>
                </div>
                <div class="text-center py-24 text-white/40">
                    <p class="text-4xl mb-3">♡</p>
                    <p class="text-base font-semibold">Noch keine Favoriten markiert</p>
                    <p class="text-xs text-white/30 mt-1">Klicke auf das Herz-Symbol bei einem Titel, um ihn hier zu speichern.</p>
                </div>
            </div>`;
        }
        // Setup infinite scroll for favorites
        if (hasMore) {
            setupInfiniteScroll(() => loadMoreFavoritesInfinite(total));
        }
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

let favoritesPage = 0;

async function loadMoreFavoritesInfinite(initialTotal) {
    try {
        favoritesPage++;
        const startIndex = favoritesPage * LIBRARY_PAGE_SIZE;
        const data = await API.getUserItems({ IsFavorite: true, StartIndex: startIndex, Limit: LIBRARY_PAGE_SIZE });
        const items = data.Items || data.items || [];
        const total = data.TotalRecordCount || data.total_record_count || initialTotal;

        if (items.length > 0 && virtualGrid) {
            virtualGrid.appendItems(items);
        }

        // If no more items, destroy infinite scroll and remove sentinel
        const remaining = total - (favoritesPage + 1) * LIBRARY_PAGE_SIZE;
        if (remaining <= 0) {
            destroyInfiniteScroll();
            const sentinelContainer = $('fav-infinite-container');
            if (sentinelContainer) sentinelContainer.remove();
        }
    } catch (e) {
        toast('Fehler: ' + e.message, 'error');
    }
}

let currentLetterFilter = '';
let libraryPage = 0;
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

// --- Virtual Grid: only render visible cards + buffer ---
// Replaces the DOM-heavy approach of appending all cards to the grid.
// With 1500 films, this keeps ~30 DOM cards instead of 1500+.
class VirtualGrid {
    constructor(containerId, options = {}) {
        this.containerId = containerId;
        this.items = [];
        this.cardHeight = options.cardHeight || 280; // estimated card height in px
        this.buffer = options.buffer || 5; // extra cards above/below viewport
        this.columns = this._getColumns();
        this.scrollContainer = null;
        this.gridEl = null;
        this.spacerTop = null;
        this.spacerBottom = null;
        this.cardsContainer = null;
        this.renderedStart = -1;
        this.renderedEnd = -1;
        this.resizeObserver = null;
        this._onResize = this._onResize.bind(this);
        this._onScroll = this._onScroll.bind(this);
        this._rafId = null;
    }

    _getColumns() {
        const w = window.innerWidth;
        if (w >= 1280) return 7;
        if (w >= 1024) return 5;
        if (w >= 768) return 4;
        if (w >= 640) return 3;
        return 2;
    }

    init() {
        this.container = document.getElementById(this.containerId);
        if (!this.container) return;
        // Create virtual grid structure
        this.container.innerHTML = `
            <div class="jf-virtual-grid" style="position:relative;overflow:auto;max-height:calc(100vh - 280px);">
                <div class="jf-virtual-spacer" style="height:0"></div>
                <div class="jf-virtual-cards" style="position:absolute;top:0;left:0;right:0;"></div>
                <div class="jf-virtual-spacer" style="height:0"></div>
            </div>`;
        this.scrollContainer = this.container.querySelector('.jf-virtual-grid');
        this.spacerTop = this.scrollContainer.children[0];
        this.cardsContainer = this.scrollContainer.children[1];
        this.spacerBottom = this.scrollContainer.children[2];
        this.scrollContainer.addEventListener('scroll', this._onScroll, { passive: true });
        this.resizeObserver = new ResizeObserver(this._onResize);
        this.resizeObserver.observe(this.scrollContainer);
        this._updateTotalHeight();
    }

    destroy() {
        if (this.scrollContainer) {
            this.scrollContainer.removeEventListener('scroll', this._onScroll);
        }
        if (this.resizeObserver) {
            this.resizeObserver.disconnect();
        }
        if (this._rafId) {
            cancelAnimationFrame(this._rafId);
        }
    }

    _onResize() {
        this.columns = this._getColumns();
        this._renderVisible();
    }

    _onScroll() {
        if (this._rafId) return;
        this._rafId = requestAnimationFrame(() => {
            this._rafId = null;
            this._renderVisible();
        });
    }

    _updateTotalHeight() {
        const rows = Math.ceil(this.items.length / this.columns);
        const totalHeight = rows * this.cardHeight;
        this.spacerTop.style.height = '0px';
        this.spacerBottom.style.height = `${Math.max(0, totalHeight)}px`;
        this.scrollContainer.style.minHeight = `${totalHeight}px`;
    }

    setItems(newItems) {
        this.items = newItems;
        this._updateTotalHeight();
        this._renderVisible();
    }

    appendItems(newItems) {
        this.items.push(...newItems);
        this._updateTotalHeight();
        this._renderVisible();
    }

    _renderVisible() {
        if (!this.scrollContainer || this.items.length === 0) return;
        const scrollTop = this.scrollContainer.scrollTop;
        const viewportHeight = this.scrollContainer.clientHeight;
        const rowsPerCard = 1; // each card is one row in the grid
        const cardHeightWithGap = this.cardHeight + 16; // gap-4 = 16px

        const startRow = Math.max(0, Math.floor(scrollTop / cardHeightWithGap) - this.buffer);
        const endRow = Math.ceil((scrollTop + viewportHeight) / cardHeightWithGap) + this.buffer;

        const startIdx = Math.max(0, startRow * this.columns);
        const endIdx = Math.min(this.items.length, (endRow + 1) * this.columns);

        if (startIdx === this.renderedStart && endIdx === this.renderedEnd) return;
        this.renderedStart = startIdx;
        this.renderedEnd = endIdx;

        // Clear and re-render visible range
        this.cardsContainer.innerHTML = '';
        const fragment = document.createDocumentFragment();
        const grid = document.createElement('div');
        grid.className = 'grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4';
        grid.style.paddingTop = `${startIdx * cardHeightWithGap}px`;
        grid.style.paddingBottom = `${Math.max(0, (this.items.length - endIdx) * cardHeightWithGap)}px`;

        for (let i = startIdx; i < endIdx; i++) {
            const card = createCardElement(this.items[i]);
            if (card) grid.appendChild(card);
        }
        fragment.appendChild(grid);
        this.cardsContainer.appendChild(fragment);

        // Update spacers
        this.spacerTop.style.height = `${startIdx * cardHeightWithGap}px`;
        const remaining = this.items.length - endIdx;
        this.spacerBottom.style.height = `${remaining * cardHeightWithGap}px`;

        initLazyImages();
    }
}

// Global virtual grid instance
let virtualGrid = null;

// --- Library Explorer Page Renderer (with pagination) ---
async function renderLibrary(libId) {
    currentLibraryId = libId;
    currentLetterFilter = '';
    libraryPage = 0;
    const page = $('page-library');
    
    // Show skeleton grid immediately for instant visual feedback
    const lib = libraries.find(l => (l.id || l.Id) === libId);
    const libName = lib ? (lib.name || lib.Name) : 'Bibliothek';
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
                <p class="text-xs text-white/40 mt-1">Wird geladen...</p>
            </div>
        </div>
        <div class="jf-alpha-bar mb-6 opacity-50">
            ${'#ABCDEFGHIJKLMNOPQRSTUVWXYZ'.split('').map(l => `<span class="jf-alpha-btn">${l}</span>`).join('')}
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

    try {
        // Determine which item types to show based on library content type
        let includeItemTypes = undefined;
        if (contentType === 'TvShows') {
            includeItemTypes = 'Series';
        } else if (contentType === 'Music') {
            includeItemTypes = 'MusicAlbum';
        } else if (contentType === 'Movies') {
            includeItemTypes = 'Movie';
        }

        // Fetch first page with pagination + total count
        const params = { 
            LibraryId: libId, 
            SortBy: 'SortName', 
            SortOrder: 'Ascending', 
            Fields: 'PrimaryImageAspectRatio',
            StartIndex: 0,
            Limit: LIBRARY_PAGE_SIZE
        };
        if (includeItemTypes) params.IncludeItemTypes = includeItemTypes;

        const [libData, filterData] = await Promise.all([
            API.getItems(params),
            API.getFilters().catch(() => ({ Genres: [] }))
        ]);

        const items = libData.Items || libData.items || [];
        const total = libData.TotalRecordCount || libData.total_record_count || items.length;
        const genres = filterData.Genres || filterData.genres || [];
        const hasMore = items.length >= LIBRARY_PAGE_SIZE;

        page.innerHTML = `
        <div class="jf-page">
            <!-- Header bar with title, count, and actions -->
            <div class="flex flex-col md:flex-row md:items-center justify-between gap-4 mb-6 pb-4 border-b border-white/10">
                <div>
                    <div class="flex items-center gap-2">
                        <span class="text-2xl">${lib?.content_type === 'Movies' ? '🎬' : lib?.content_type === 'TvShows' ? '📺' : lib?.content_type === 'Music' ? '🎵' : '📁'}</span>
                        <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
                    </div>
                    <p id="lib-count-label" class="text-xs text-white/40 mt-1">${total} Medienobjekte erfasst</p>
                </div>

                <div class="flex flex-wrap items-center gap-2">
                    <!-- Sort selector -->
                    <select id="lib-sort" class="jf-input !py-1.5 text-xs" onchange="filterLibrary('${libId}')">
                        <option value="SortName:Ascending">Name (A-Z)</option>
                        <option value="SortName:Descending">Name (Z-A)</option>
                        <option value="PremiereDate:Descending">Erscheinungsdatum</option>
                        <option value="DateCreated:Descending">Zuletzt hinzugefügt</option>
                        <option value="CommunityRating:Descending">Bewertung (★)</option>
                    </select>

                    <!-- Genre filter -->
                    ${genres.length > 0 ? `
                    <select id="lib-genre" class="jf-input !py-1.5 text-xs max-w-[140px]" onchange="filterLibrary('${libId}')">
                        <option value="">Alle Genres</option>
                        ${genres.map(g => `<option value="${escapeHtml(g)}">${escapeHtml(g)}</option>`).join('')}
                    </select>` : ''}

                    <!-- Scan trigger -->
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs" onclick="API.scanLibrary('${libId}').then(()=>{toast('Medienscan für ${escapeHtml(libName)} gestartet');scanStatuses['${libId}']=true;})">
                        🔄 Scannen
                    </button>
                </div>
            </div>

            <!-- Alphabet Quick Jump Bar -->
            <div class="jf-alpha-bar mb-6">
                <button class="jf-alpha-btn active" data-letter="" onclick="jumpToLetter('${libId}', '')">Alle</button>
                ${'#ABCDEFGHIJKLMNOPQRSTUVWXYZ'.split('').map(l => `
                    <button class="jf-alpha-btn" data-letter="${l}" onclick="jumpToLetter('${libId}', '${l}')">${l}</button>
                `).join('')}
            </div>

            <!-- Media Grid Container (Virtual Scrolling) -->
            <div id="lib-grid-container"></div>

            <!-- Infinite Scroll Sentinel (replaces Load More button) -->
            ${hasMore ? `
            <div id="lib-infinite-container" class="text-center mt-8">
                <div id="infinite-scroll-spinner" class="jf-infinite-spinner" style="display:none"><div class="jf-spinner"></div></div>
                <div id="infinite-scroll-sentinel" class="jf-infinite-sentinel"></div>
            </div>` : ''}
        </div>`;
        // Initialize virtual grid for library
        if (virtualGrid) { virtualGrid.destroy(); virtualGrid = null; }
        virtualGrid = new VirtualGrid('lib-grid-container');
        virtualGrid.init();
        if (items.length > 0) {
            virtualGrid.setItems(items);
        }
        // Setup infinite scroll for library
        if (hasMore) {
            setupInfiniteScroll(() => loadMoreLibraryItemsInfinite(libId, total));
        }
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

// --- Load More Library Items (infinite scroll callback) ---
async function loadMoreLibraryItemsInfinite(libId, initialTotal) {
    try {
        const lib = libraries.find(l => (l.id || l.Id) === libId);
        const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';

        let includeItemTypes = undefined;
        if (contentType === 'TvShows') includeItemTypes = 'Series';
        else if (contentType === 'Music') includeItemTypes = 'MusicAlbum';
        else if (contentType === 'Movies') includeItemTypes = 'Movie';

        libraryPage++;
        const startIndex = libraryPage * LIBRARY_PAGE_SIZE;

        const params = { 
            LibraryId: libId, 
            SortBy: 'SortName', 
            SortOrder: 'Ascending', 
            Fields: 'PrimaryImageAspectRatio',
            StartIndex: startIndex,
            Limit: LIBRARY_PAGE_SIZE
        };
        if (includeItemTypes) params.IncludeItemTypes = includeItemTypes;

        const data = await API.getItems(params);
        const items = data.Items || data.items || [];
        const total = data.TotalRecordCount || data.total_record_count || initialTotal;

        if (items.length > 0 && virtualGrid) {
            virtualGrid.appendItems(items);
        }

        // If no more items, destroy infinite scroll and remove sentinel
        const remaining = total - (libraryPage + 1) * LIBRARY_PAGE_SIZE;
        if (remaining <= 0) {
            destroyInfiniteScroll();
            const sentinelContainer = $('lib-infinite-container');
            if (sentinelContainer) sentinelContainer.remove();
        }
    } catch (e) {
        toast('Fehler beim Laden: ' + e.message, 'error');
    }
}

async function filterLibrary(libId) {
    const sortVal = $('lib-sort')?.value || 'SortName:Ascending';
    const [sortBy, sortOrder] = sortVal.split(':');
    const genre = $('lib-genre')?.value || '';

    // Remove existing infinite scroll when filtering
    destroyInfiniteScroll();
    const existingInfinite = document.getElementById('lib-infinite-container');
    if (existingInfinite) existingInfinite.remove();

    // Show loading state in virtual grid container
    const container = $('lib-grid-container');
    if (container) {
        container.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    }

    try {
        const lib = libraries.find(l => (l.id || l.Id) === libId);
        const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';

        let includeItemTypes = undefined;
        if (contentType === 'TvShows') includeItemTypes = 'Series';
        else if (contentType === 'Music') includeItemTypes = 'MusicAlbum';
        else if (contentType === 'Movies') includeItemTypes = 'Movie';

        // Reset pagination on filter change
        libraryPage = 0;

        // Fetch first page with pagination
        const params = { 
            LibraryId: libId, 
            SortBy: sortBy, 
            SortOrder: sortOrder, 
            Fields: 'PrimaryImageAspectRatio',
            StartIndex: 0,
            Limit: LIBRARY_PAGE_SIZE
        };
        if (includeItemTypes) params.IncludeItemTypes = includeItemTypes;
        if (genre) params.Genres = genre;
        if (currentLetterFilter) params.NameStartsWith = currentLetterFilter;

        const data = await API.getItems(params);
        const items = data.Items || data.items || [];
        const total = data.TotalRecordCount || data.total_record_count || items.length;
        const hasMore = items.length >= LIBRARY_PAGE_SIZE;
        const countLabel = $('lib-count-label');
        if (countLabel) countLabel.textContent = `${total} Medienobjekte erfasst`;

        if (items.length > 0) {
            // Initialize virtual grid for filtered results
            if (virtualGrid) { virtualGrid.destroy(); virtualGrid = null; }
            virtualGrid = new VirtualGrid('lib-grid-container');
            virtualGrid.init();
            virtualGrid.setItems(items);
            // Setup infinite scroll for filtered library
            if (hasMore) {
                const infDiv = document.createElement('div');
                infDiv.id = 'lib-infinite-container';
                infDiv.className = 'text-center mt-8';
                infDiv.innerHTML = `
                    <div id="infinite-scroll-spinner" class="jf-infinite-spinner" style="display:none"><div class="jf-spinner"></div></div>
                    <div id="infinite-scroll-sentinel" class="jf-infinite-sentinel"></div>
                `;
                // Append sentinel after the virtual grid container
                const page = $('page-library');
                if (page) page.querySelector('.jf-page').appendChild(infDiv);
                setupInfiniteScroll(() => loadMoreFilteredLibraryItemsInfinite(libId, total));
            }
        } else {
            if (container) container.innerHTML = '<div class="text-center py-20 text-white/40">Keine Einträge für diese Filterkombination.</div>';
        }
    } catch (e) {
        toast('Filterfehler: ' + e.message, 'error');
    }
}

// --- Load More for Filtered Library (infinite scroll callback) ---
async function loadMoreFilteredLibraryItemsInfinite(libId, initialTotal) {
    try {
        const sortVal = $('lib-sort')?.value || 'SortName:Ascending';
        const [sortBy, sortOrder] = sortVal.split(':');
        const genre = $('lib-genre')?.value || '';

        const lib = libraries.find(l => (l.id || l.Id) === libId);
        const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';
        let includeItemTypes = undefined;
        if (contentType === 'TvShows') includeItemTypes = 'Series';
        else if (contentType === 'Music') includeItemTypes = 'MusicAlbum';
        else if (contentType === 'Movies') includeItemTypes = 'Movie';

        libraryPage++;
        const startIndex = libraryPage * LIBRARY_PAGE_SIZE;

        const params = { 
            LibraryId: libId, SortBy: sortBy, SortOrder: sortOrder, 
            Fields: 'PrimaryImageAspectRatio',
            StartIndex: startIndex, Limit: LIBRARY_PAGE_SIZE
        };
        if (includeItemTypes) params.IncludeItemTypes = includeItemTypes;
        if (genre) params.Genres = genre;
        if (currentLetterFilter) params.NameStartsWith = currentLetterFilter;

        const data = await API.getItems(params);
        const items = data.Items || data.items || [];
        const total = data.TotalRecordCount || data.total_record_count || initialTotal;

        if (items.length > 0 && virtualGrid) {
            virtualGrid.appendItems(items);
        }

        // If no more items, destroy infinite scroll and remove sentinel
        const remaining = total - (libraryPage + 1) * LIBRARY_PAGE_SIZE;
        if (remaining <= 0) {
            destroyInfiniteScroll();
            const sentinelContainer = $('lib-infinite-container');
            if (sentinelContainer) sentinelContainer.remove();
        }
    } catch (e) {
        toast('Fehler: ' + e.message, 'error');
    }
}

async function jumpToLetter(libId, letter) {
    currentLetterFilter = letter || '';
    document.querySelectorAll('.jf-alpha-btn').forEach(b => {
        const btnLetter = b.getAttribute('data-letter') ?? '';
        b.classList.toggle('active', btnLetter === currentLetterFilter);
    });
    await filterLibrary(libId);
}

// --- Detail View Renderer (Movies, TV Series, Music) ---
async function renderDetail(itemId) {
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

        const imgSrc = API.imageUrl(id, 'Primary', { maxWidth: 500 });
        const backdropSrc = API.imageUrl(id, 'Backdrop', { maxWidth: 1920 });
        const canPlay = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video'].includes(type);
        const streamType = type === 'Audio' ? 'Audio' : 'Videos';

        // Parallel fetch: Children/Seasons/Episodes AND Similar media
        // Previously these were sequential, causing a waterfall effect
        const isContainer = ['Series', 'Season', 'MusicAlbum', 'Folder'].includes(type);
        const [childrenData, similarData] = await Promise.all([
            isContainer ? API.getChildren(id).catch(() => ({ Items: [] })) : Promise.resolve({ Items: [] }),
            API.getSimilar(id).catch(() => ({ Items: [] }))
        ]);

        // Children / Seasons / Episodes
        let seasonsOrEpisodesHtml = '';
        const children = childrenData.Items || childrenData.items || [];
        if (children.length > 0) {
            if (type === 'Series') {
                // Build seasons grid using DocumentFragment
                const seasonsDiv = document.createElement('div');
                seasonsDiv.className = 'mb-10';
                seasonsDiv.innerHTML = `<h2 class="text-xl font-bold mb-4 text-white">Staffeln (${children.length})</h2>`;
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
                    <h2 class="text-xl font-bold mb-4 text-white">Episoden (${children.length})</h2>
                    <div class="space-y-3">
                        ${children.map(ep => {
                            const epId = ep.Id || ep.id;
                            const epName = ep.Name || ep.name || 'Episode';
                            const epIndex = ep.IndexNumber || ep.index_number || '';
                            const epDesc = ep.Overview || ep.overview || '';
                            const epDur = formatRuntime(ep.RunTimeTicks || ep.run_time_ticks);
                            const epPlayed = ep.UserData?.Played || ep.user_data?.played || false;
                            const epThumb = API.imageUrl(epId, 'Primary', { maxWidth: 400 });

                            return `
                            <div class="jf-episode-card group" onclick="playItem('${epId}','Videos','${escapeHtml(epName)}')">
                                <div class="relative w-full sm:w-48 h-28 rounded-lg overflow-hidden bg-white/5 flex-shrink-0">
                                    <img src="${epThumb}" alt="${escapeHtml(epName)}" class="w-full h-full object-cover">
                                    <div class="absolute inset-0 bg-black/40 flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity">
                                        <div class="w-10 h-10 rounded-full bg-[#00a4dc] text-white flex items-center justify-center shadow-lg">
                                            <svg class="w-5 h-5 ml-0.5" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
                                        </div>
                                    </div>
                                    ${epPlayed ? `<span class="absolute top-2 right-2 w-5 h-5 rounded-full bg-[#00a4dc] text-white flex items-center justify-center text-[10px]">✓</span>` : ''}
                                </div>
                                <div class="flex-1 min-w-0">
                                    <div class="flex items-center gap-2 mb-1">
                                        ${epIndex !== '' ? `<span class="text-xs font-bold text-[#00a4dc]">E${epIndex}</span>` : ''}
                                        <h3 class="text-sm font-bold text-white truncate group-hover:text-[#38bdf8] transition-colors">${escapeHtml(epName)}</h3>
                                        ${epDur ? `<span class="text-[11px] text-white/40 ml-auto flex-shrink-0">${epDur}</span>` : ''}
                                    </div>
                                    <p class="text-xs text-white/60 line-clamp-2 leading-relaxed">${escapeHtml(epDesc) || 'Keine Beschreibung vorhanden.'}</p>
                                </div>
                            </div>`;
                        }).join('')}
                    </div>
                </div>`;
            } else {
                seasonsOrEpisodesHtml = sectionRow('Titel', children);
            }
        }

        // Similar media
        let similarHtml = '';
        const similar = similarData.Items || similarData.items || [];
        if (similar.length > 0) {
            similarHtml = sectionRow('Ähnliche Empfehlungen', similar.slice(0, 7));
        }

        // Cast & Crew rail
        let castHtml = '';
        if (people.length > 0) {
            castHtml = `
            <div class="mb-10">
                <h2 class="text-xl font-bold mb-4 text-white">Besetzung & Mitwirkende</h2>
                <div class="jf-scroll-row pb-2">
                    ${people.slice(0, 15).map(p => {
                        const pId = p.Id || p.id;
                        const pName = p.Name || p.name || '';
                        const pRole = p.Role || p.role || p.Type || p.type || 'Schauspieler';
                        const pImg = pId ? API.imageUrl(pId, 'Primary', { maxWidth: 200 }) : '';
                        return `
                        <div class="w-28 text-center flex-shrink-0 cursor-pointer group" onclick="${pId ? `navigate('person','${pId}')` : ''}">
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

        page.innerHTML = `
        <div class="jf-page">
            <!-- Full Hero Backdrop Banner -->
            <div class="jf-detail-hero -mx-4 lg:-mx-8 -mt-6">
                <img src="${backdropSrc}" class="w-full h-[460px] object-cover opacity-35" onerror="this.style.display='none'">
                <div class="absolute bottom-0 left-0 right-0 p-6 sm:p-10 lg:p-12 z-10 flex flex-col md:flex-row gap-8 items-end">
                    <!-- Poster Box -->
                    <div class="w-44 sm:w-56 rounded-2xl overflow-hidden shadow-2xl border border-white/10 flex-shrink-0 hidden md:block bg-black/60">
                        <img src="${imgSrc}" alt="${escapeHtml(name)}" class="w-full h-auto object-cover" onerror="this.style.display='none'">
                    </div>

                    <!-- Meta & Actions -->
                    <div class="flex-1 min-w-0">
                        <div class="flex flex-wrap items-center gap-2 mb-3">
                            <span class="jf-badge jf-badge-primary uppercase tracking-wider text-[11px]">${type}</span>
                            ${year ? `<span class="jf-badge text-[11px] font-bold">${year}</span>` : ''}
                            ${officialRating ? `<span class="jf-badge text-[11px] font-bold bg-white/15">${officialRating}</span>` : ''}
                            ${runtime ? `<span class="jf-badge text-[11px]">${formatRuntime(runtime)}</span>` : ''}
                            ${rating ? `<span class="jf-badge jf-badge-rating text-[11px]">★ ${rating.toFixed(1)}</span>` : ''}
                        </div>

                        <h1 class="text-3xl sm:text-4xl lg:text-5xl font-black text-white tracking-tight mb-4 drop-shadow-lg">${escapeHtml(name)}</h1>

                        ${genres.length > 0 ? `
                        <div class="flex flex-wrap gap-1.5 mb-6">
                            ${genres.map(g => `<span class="text-[11px] font-medium px-2.5 py-0.5 rounded-full bg-white/10 text-white/80 border border-white/10">${escapeHtml(g)}</span>`).join('')}
                        </div>` : ''}

                        <!-- Action Bar -->
                        <div class="flex flex-wrap items-center gap-3">
                            ${canPlay ? `
                            <button class="jf-btn-primary !py-3 !px-7 text-sm font-bold shadow-xl" onclick="playItem('${id}','${streamType}','${escapeHtml(name)}')">
                                <svg class="w-5 h-5" fill="currentColor" viewBox="0 0 24 24"><path d="M8 5v14l11-7z"/></svg>
                                Abspielen
                            </button>` : ''}

                            <button class="jf-btn-ghost !py-3 !px-4 text-sm" title="Favorit umschalten" onclick="toggleFavorite('${id}', ${fav})">
                                <span class="${fav ? 'text-rose-500 font-bold' : 'text-white/60'} text-base">♥</span>
                                <span class="hidden sm:inline text-xs ml-1">${fav ? 'Favorit' : 'Favorisieren'}</span>
                            </button>

                            <button class="jf-btn-ghost !py-3 !px-4 text-xs font-semibold" onclick="togglePlayed('${id}', ${played})">
                                ${played ? '<span class="text-emerald-400">✓ Gesehen</span>' : '<span class="text-white/60">○ Ungesehen</span>'}
                            </button>

                            ${API.isAdmin ? `
                            <button class="jf-btn-ghost !py-3 !px-3 text-xs" title="Metadaten neu laden" onclick="API.refreshItem('${id}').then(()=>toast('Metadaten werden aktualisiert…'))">
                                🔄
                            </button>` : ''}
                        </div>
                    </div>
                </div>
            </div>

            <!-- Overview section -->
            ${overview ? `
            <div class="mb-10 max-w-4xl">
                <h2 class="text-lg font-bold mb-2 text-white">Handlung & Übersicht</h2>
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
        const pName = person.Name || person.name || 'Person';
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
                <h2 class="text-xl font-bold text-white mb-4">Bekannt für (${items.length})</h2>
                <div id="person-known-grid" class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4"></div>
            </div>` : '<p class="text-white/40">Keine verknüpften Medien gefunden.</p>'}
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
    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        const data = await API.search(term, 50);
        const hints = data.SearchHints || data.search_hints || [];
        if (!hints.length) {
            page.innerHTML = `<div class="text-center py-24 text-white/40 jf-page"><p class="text-lg">Keine Treffer für „${escapeHtml(term)}"</p></div>`;
            return;
        }
        const items = hints.map(h => ({
            Id: h.Id || h.id || h.ItemId,
            Name: h.Name || h.name,
            Type: h.Type || h.type,
            ProductionYear: h.ProductionYear || h.production_year
        }));

        page.innerHTML = `
        <div class="jf-page">
            <h1 class="text-2xl font-bold mb-6 text-white">Suchergebnisse für „${escapeHtml(term)}" (${items.length})</h1>
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

// --- Video Player (Cinema Mode) ---
async function playItem(itemId, streamType = 'Videos', title = '') {
    const video = $('video-player');
    const modal = $('player-modal');
    if (!video || !modal) return;

    // Destroy any existing HLS instance
    if (hlsInstance) {
        hlsInstance.destroy();
        hlsInstance = null;
    }

    $('player-title').textContent = title || 'Wiedergabe';
    $('player-subtitle').textContent = 'Lade...';

    // Show modal immediately while loading
    if (typeof modal.showModal === 'function') modal.showModal();
    else modal.setAttribute('open', '');

    let url;
    let isHls = false;
    if (streamType === 'Videos') {
        // Use Universal endpoint to get the optimal playback URL
        try {
            const data = await API.getUniversalVideoUrl(itemId);
            if (data && data.url) {
                // Add auth token to the stream URL so the video element can authenticate
                const sep = data.url.includes('?') ? '&' : '?';
                url = API.base + data.url + sep + 'api_key=' + encodeURIComponent(API.token);
                isHls = data.url.includes('.m3u8');
                // Determine playback method from the URL, not from SupportsDirectPlay flag
                // which may be stale from the database
                let method;
                if (isHls) {
                    method = 'HLS Transcode';
                } else if (data.url.includes('/stream')) {
                    // Check if it's a static direct play or direct stream
                    method = data.url.includes('Static=true') ? 'Direct Play' : 'Direct Stream';
                } else {
                    method = 'Stream';
                }
                $('player-subtitle').textContent = method;
            } else {
                // Fallback: direct stream
                url = API.streamUrl(itemId, streamType);
                $('player-subtitle').textContent = 'Video Stream (Fallback)';
            }
        } catch (e) {
            console.warn('Universal endpoint failed, using direct stream:', e);
            url = API.streamUrl(itemId, streamType);
            $('player-subtitle').textContent = 'Video Stream (Fallback)';
        }
    } else if (streamType === 'Audio') {
        url = API.streamUrl(itemId, streamType);
        $('player-subtitle').textContent = 'Audio Stream';
    } else {
        url = API.streamUrl(itemId, streamType);
        $('player-subtitle').textContent = 'Stream';
    }

    video.onerror = () => {
        console.error('Video playback error:', video.error);
        const errMsg = video.error ? video.error.message : 'Unbekannter Fehler';
        $('player-subtitle').textContent = 'Fehler: ' + errMsg;
    };

    // Handle HLS streams with hls.js
    if (isHls && typeof Hls !== 'undefined' && Hls.isSupported()) {
        hlsInstance = new Hls({
            enableWorker: true,
            lowLatencyMode: false,
        });
        hlsInstance.loadSource(url);
        hlsInstance.attachMedia(video);
        hlsInstance.on(Hls.Events.MANIFEST_PARSED, () => {
            video.play().catch(e => console.warn('Autoplay block:', e));
        });
        hlsInstance.on(Hls.Events.ERROR, (event, data) => {
            console.error('HLS error:', data);
            if (data.fatal) {
                $('player-subtitle').textContent = 'HLS Stream Fehler';
                if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
                    hlsInstance.startLoad();
                } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
                    hlsInstance.recoverMediaError();
                } else {
                    hlsInstance.destroy();
                }
            }
        });
    } else if (isHls && video.canPlayType('application/vnd.apple.mpegurl')) {
        // Safari native HLS support
        video.src = url;
        video.addEventListener('loadedmetadata', () => {
            video.play().catch(e => console.warn('Autoplay block:', e));
        }, { once: true });
    } else {
        // Direct play/stream
        video.src = url;
        video.currentTime = 0;
        video.play().catch(e => console.warn('Autoplay block:', e));
    }

    API.reportPlaying(itemId).catch(() => {});

    // Periodic progress reporting
    if (playerReportInterval) clearInterval(playerReportInterval);
    playerReportInterval = setInterval(() => {
        if (!video.paused && !video.ended) {
            const ticks = Math.round(video.currentTime * 10000000);
            API.reportProgress(itemId, ticks, false).catch(() => {});
        }
    }, 8000);

    // Video events for custom UI
    video.ontimeupdate = () => {
        const cur = video.currentTime;
        const dur = video.duration || 0;
        $('player-cur-time').textContent = formatDurationSeconds(cur);
        $('player-duration').textContent = formatDurationSeconds(dur);
        if (dur > 0) {
            $('player-seek').value = (cur / dur) * 100;
        }
    };

    video.onplay = () => updatePlayIcon(true);
    video.onpause = () => updatePlayIcon(false);
    video.onvolumechange = () => {
        updateVolumeIcon();
        const slider = $('player-vol');
        if (slider) slider.value = video.volume;
    };

    const cleanup = () => {
        if (playerReportInterval) {
            clearInterval(playerReportInterval);
            playerReportInterval = null;
        }
        const ticks = Math.round((video.currentTime || 0) * 10000000);
        API.reportStopped(itemId, ticks).catch(() => {});
    };

    video.onended = cleanup;
    modal.addEventListener('close', () => {
        video.pause();
        // Destroy HLS instance if playing HLS
        if (hlsInstance) {
            hlsInstance.destroy();
            hlsInstance = null;
        }
        video.src = '';
        cleanup();
    }, { once: true });
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

function seekVideo(percent) {
    const v = $('video-player');
    if (!v || !v.duration) return;
    v.currentTime = (percent / 100) * v.duration;
}

function skipTime(deltaSeconds) {
    const v = $('video-player');
    if (!v) return;
    v.currentTime = Math.max(0, Math.min(v.duration || 0, v.currentTime + deltaSeconds));
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
        toast(current ? 'Favorit entfernt' : 'Zu Favoriten hinzugefügt');
        if (currentDetailId === id) renderDetail(id);
    } catch (e) {
        toast(e.message, 'error');
    }
}

async function togglePlayed(id, current) {
    try {
        if (current) await API.markUnplayed(id);
        else await API.markPlayed(id);
        toast(current ? 'Als ungesehen markiert' : 'Als gesehen markiert');
        if (currentDetailId === id) renderDetail(id);
    } catch (e) {
        toast(e.message, 'error');
    }
}

// --- Login Page Renderer ---
function renderLogin() {
    html('page-login', `
    <div class="flex justify-center items-center min-h-[70vh] jf-page px-4">
        <div class="w-full max-w-sm p-8 rounded-2xl bg-[#131822] border border-white/10 shadow-2xl">
            <div class="text-center mb-8">
                <img src="/web/assets/logo.png" alt="Rustyfin" class="w-20 h-20 mx-auto rounded-2xl shadow-xl mb-3 p-1 bg-white/5 border border-white/10">
                <h2 class="text-2xl font-black text-white tracking-tight">Anmelden</h2>
                <p class="text-white/40 text-xs mt-1">Verbinde dich mit deinem Medienserver</p>
            </div>
            <form id="login-form" onsubmit="handleLogin(event)" class="space-y-4">
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">Benutzername</label>
                    <input type="text" id="login-user" class="jf-input w-full" placeholder="Benutzername" required autofocus>
                </div>
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">Passwort</label>
                    <input type="password" id="login-pass" class="jf-input w-full" placeholder="Passwort">
                </div>
                <button type="submit" class="jf-btn-primary w-full !py-3 font-bold text-sm shadow-lg mt-2">
                    Einloggen →
                </button>
                <div id="login-error" class="text-red-400 text-xs text-center hidden p-2 rounded bg-red-500/10 border border-red-500/20"></div>
            </form>
        </div>
    </div>`);
}

async function handleLogin(e) {
    e.preventDefault();
    try {
        await API.login($('login-user').value, $('login-pass').value);
        updateAuthUI();
        await loadSidebar();
        navigate('home');
        toast(`Willkommen zurück, ${API.userName}!`);
    } catch (err) {
        $('login-error').textContent = err.message;
        show('login-error');
    }
}

function doLogout() {
    API.logout();
    updateAuthUI();
    navigate('home');
    toast('Erfolgreich abgemeldet');
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
        toast('Bitte gültigen Code eingeben', 'error');
        return;
    }
    try {
        await API.post('/QuickConnect/Authorize', { Code: code });
        toast('Gerät erfolgreich autorisiert!');
        $('qc-modal')?.close();
    } catch (e) {
        toast('Autorisierung fehlgeschlagen: ' + e.message, 'error');
    }
}

// --- Settings Page Renderer ---
async function renderSettings() {
    const page = $('page-settings');
    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        const [me, config] = await Promise.all([
            API.getMe().catch(() => ({})),
            API.getConfig().catch(() => null)
        ]);

        const name = me.Name || me.name || API.userName || '';
        const id = me.Id || me.id || API.userId || '';

        let serverCfgHtml = '';
        if (API.isAdmin && config) {
            serverCfgHtml = `
            <div class="jf-settings-card">
                <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                    <span class="w-2 h-2 rounded-full bg-[#00a4dc]"></span>
                    Server-Konfiguration
                </h3>
                <form onsubmit="handleSaveConfig(event)" class="space-y-4">
                    <div class="grid grid-cols-1 sm:grid-cols-2 gap-4">
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">Servername</label>
                            <input id="cfg-server-name" class="jf-input w-full" value="${escapeHtml(config.ServerName || config.server_name || 'Rustyfin')}">
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">Metadaten-Sprache</label>
                            <select id="cfg-meta-lang" class="jf-input w-full">
                                ${['de', 'en', 'fr', 'es', 'it', 'ja', 'zh'].map(l => `<option value="${l}" ${(config.PreferredMetadataLanguage || config.preferred_metadata_language) === l ? 'selected' : ''}>${l.toUpperCase()}</option>`).join('')}
                            </select>
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">HTTP Port</label>
                            <input id="cfg-port" type="number" class="jf-input w-full" value="${config.PublicPort || config.public_port || 8096}">
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">TMDb API Key</label>
                            <input id="cfg-tmdb" type="text" class="jf-input w-full" value="${escapeHtml(config.TmdbApiKey || config.tmdb_api_key || '')}" placeholder="Optional">
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">OMDb API Key</label>
                            <input id="cfg-omdb" type="text" class="jf-input w-full" value="${escapeHtml(config.OmdbApiKey || config.omdb_api_key || '')}" placeholder="Optional">
                        </div>
                        <div>
                            <label class="block text-xs font-semibold text-white/70 mb-1">Fanart.tv API Key</label>
                            <input id="cfg-fanart" type="text" class="jf-input w-full" value="${escapeHtml(config.FanartApiKey || config.fanart_api_key || '')}" placeholder="Optional">
                        </div>
                    </div>
                    <div class="pt-2">
                        <button type="submit" class="jf-btn-primary text-xs">Konfiguration speichern</button>
                    </div>
                </form>
            </div>`;
        }

        page.innerHTML = `
        <div class="max-w-3xl mx-auto jf-page">
            <h1 class="text-2xl lg:text-3xl font-extrabold text-white mb-6">Benutzereinstellungen</h1>

            <!-- User Profile card -->
            <div class="jf-settings-card flex items-center gap-4">
                <div class="w-16 h-16 rounded-2xl bg-gradient-to-tr from-[#00a4dc] to-[#38bdf8] flex items-center justify-center text-2xl font-bold text-white shadow-xl">
                    ${escapeHtml(name[0]?.toUpperCase() || 'U')}
                </div>
                <div>
                    <h2 class="text-lg font-bold text-white">${escapeHtml(name)}</h2>
                    <p class="text-xs text-white/40">Benutzer-ID: ${id}</p>
                    <span class="inline-block mt-1 text-[10px] font-bold px-2 py-0.5 rounded bg-[#00a4dc]/20 text-[#38bdf8] uppercase">
                        ${API.isAdmin ? 'Administrator' : 'Standardbenutzer'}
                    </span>
                </div>
            </div>

            <!-- Change Password card -->
            <div class="jf-settings-card">
                <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                    <span class="w-2 h-2 rounded-full bg-amber-400"></span>
                    Passwort ändern
                </h3>
                <form onsubmit="handleChangePassword(event, '${id}')" class="space-y-3">
                    <input type="password" id="pw-current" class="jf-input w-full" placeholder="Aktuelles Passwort" required>
                    <input type="password" id="pw-new" class="jf-input w-full" placeholder="Neues Passwort" required>
                    <input type="password" id="pw-confirm" class="jf-input w-full" placeholder="Neues Passwort bestätigen" required>
                    <button type="submit" class="jf-btn-primary text-xs">Passwort speichern</button>
                    <span id="pw-msg" class="text-xs ml-3 hidden"></span>
                </form>
            </div>

            ${serverCfgHtml}
        </div>`;
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

async function handleChangePassword(e, userId) {
    e.preventDefault();
    const cur = $('pw-current').value;
    const n = $('pw-new').value;
    const c = $('pw-confirm').value;
    if (n !== c) {
        toast('Passwörter stimmen nicht überein', 'error');
        return;
    }
    try {
        await API.changePassword(userId, cur, n);
        toast('Passwort erfolgreich geändert');
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
        toast('Server-Konfiguration aktualisiert');
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- Admin Dashboard Renderer ---
async function renderAdmin() {
    const page = $('page-admin');
    if (!API.isAdmin) {
        page.innerHTML = '<div class="p-8 text-center text-yellow-400 jf-page">Zugriff verweigert (Administratorrechte erforderlich).</div>';
        return;
    }

    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        const [users, libs, tasks, plugins, info, sessions] = await Promise.all([
            API.getUsers().catch(() => []),
            API.getLibraries().catch(() => []),
            API.getTasks().catch(() => []),
            API.getPlugins().catch(() => []),
            API.getFullInfo().catch(() => ({})),
            API.getSessions().catch(() => [])
        ]);

        const tabs = ['Übersicht', 'Benutzer', 'Bibliotheken', 'Aufgaben', 'Sitzungen', 'Aktivitätslog', 'Live-Log'];
        const tabIds = ['admin-overview', 'admin-users', 'admin-libs', 'admin-tasks', 'admin-sessions', 'admin-activity', 'admin-livelog'];

        page.innerHTML = `
        <div class="max-w-6xl mx-auto jf-page">
            <div class="flex items-center justify-between mb-6">
                <div>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white">Server Dashboard</h1>
                    <p class="text-xs text-white/40 mt-1">Status & Verwaltung für ${escapeHtml(info.ServerName || 'Rustyfin')}</p>
                </div>
                <div class="flex gap-2">
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs" onclick="API.scanAllLibraries().then(()=>toast('Alle Bibliotheken werden gescannt…'))">
                        📚 Alle scannen
                    </button>
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs text-amber-400 hover:text-amber-300" onclick="if(confirm('Server neustarten?'))API.restart().then(()=>toast('Neustart…'))">
                        🔄 Neustart
                    </button>
                </div>
            </div>

            <!-- Tab pills -->
            <div class="flex gap-2 mb-6 overflow-x-auto pb-2">
                ${tabs.map((t, i) => `
                    <button class="px-4 py-2 rounded-xl text-xs font-semibold whitespace-nowrap transition ${i === 0 ? 'bg-[#00a4dc] text-white shadow-lg' : 'bg-white/5 text-white/60 hover:bg-white/10'}"
                            onclick="showAdminTab(this, '${tabIds[i]}')">${t}</button>
                `).join('')}
            </div>

            <!-- Tab 0: Overview -->
            <div id="admin-overview" class="space-y-6">
                <div class="grid grid-cols-2 md:grid-cols-4 gap-4">
                    <div class="jf-settings-card text-center">
                        <p class="text-3xl font-black text-[#00a4dc]">${escapeHtml(info.ServerName || 'Rustyfin')}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">Server</p>
                    </div>
                    <div class="jf-settings-card text-center">
                        <p class="text-3xl font-black text-white">v${escapeHtml(info.Version || serverVersion)}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">Release Version</p>
                    </div>
                    <div class="jf-settings-card text-center">
                        <p class="text-3xl font-black text-emerald-400">${users.length}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">Registrierte Benutzer</p>
                    </div>
                    <div class="jf-settings-card text-center">
                        <p class="text-3xl font-black text-amber-400">${libs.length}</p>
                        <p class="text-[11px] text-white/40 uppercase tracking-wider mt-1">Bibliotheken</p>
                    </div>
                </div>

                <div class="jf-settings-card">
                    <h3 class="font-bold text-white text-base mb-4">Systemdetails</h3>
                    <div class="grid grid-cols-1 md:grid-cols-2 gap-4 text-xs">
                        <div class="flex justify-between py-2 border-b border-white/5"><span class="text-white/50">Betriebssystem:</span> <span class="font-mono text-white">${escapeHtml(info.OperatingSystem || 'Linux')}</span></div>
                        <div class="flex justify-between py-2 border-b border-white/5"><span class="text-white/50">Architektur:</span> <span class="font-mono text-white">${escapeHtml(info.SystemArchitecture || 'x86_64')}</span></div>
                        <div class="flex justify-between py-2 border-b border-white/5"><span class="text-white/50">Programmdaten:</span> <span class="font-mono text-white truncate max-w-xs">${escapeHtml(info.ProgramDataPath || '/data')}</span></div>
                        <div class="flex justify-between py-2 border-b border-white/5"><span class="text-white/50">Log-Pfad:</span> <span class="font-mono text-white truncate max-w-xs">${escapeHtml(info.LogPath || '/data/logs')}</span></div>
                    </div>
                </div>
            </div>

            <!-- Tab 1: Users -->
            <div id="admin-users" class="hidden jf-settings-card">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="font-bold text-white text-base">Benutzerkonten</h3>
                    <button class="jf-btn-primary text-xs" onclick="openAddUser()">＋ Neuer Benutzer</button>
                </div>
                <div class="overflow-x-auto">
                    <table class="w-full text-left text-xs">
                        <thead>
                            <tr class="text-white/40 border-b border-white/10 pb-2">
                                <th class="pb-3 font-semibold">Name</th>
                                <th class="pb-3 font-semibold">Berechtigung</th>
                                <th class="pb-3 font-semibold">Letzter Login</th>
                                <th class="pb-3 font-semibold text-right">Aktion</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-white/5">
                            ${users.map(u => `
                            <tr>
                                <td class="py-3 font-bold text-white">${escapeHtml(u.Name || u.name)}</td>
                                <td class="py-3">${(u.Policy?.IsAdministrator || u.policy?.is_administrator) ? '<span class="px-2 py-0.5 rounded text-[10px] font-bold bg-[#00a4dc]/20 text-[#38bdf8]">Admin</span>' : '<span class="text-white/40">Benutzer</span>'}</td>
                                <td class="py-3 text-white/40">${escapeHtml(u.LastLoginDate || u.last_login_date || '–')}</td>
                                <td class="py-3 text-right">
                                    <button class="text-red-400 hover:text-red-300 font-semibold" onclick="if(confirm('Benutzer löschen?'))API.deleteUser('${u.Id || u.id}').then(()=>renderAdmin())">Löschen</button>
                                </td>
                            </tr>`).join('')}
                        </tbody>
                    </table>
                </div>
            </div>

            <!-- Tab 2: Libraries -->
            <div id="admin-libs" class="hidden jf-settings-card">
                <div class="flex justify-between items-center mb-4">
                    <h3 class="font-bold text-white text-base">Bibliotheken (${libs.length})</h3>
                    <button class="jf-btn-primary text-xs" onclick="openAddLibrary()">＋ Neue Bibliothek</button>
                </div>
                <div class="overflow-x-auto">
                    <table class="w-full text-left text-xs">
                        <thead>
                            <tr class="text-white/40 border-b border-white/10">
                                <th class="pb-3 font-semibold">Name</th>
                                <th class="pb-3 font-semibold">Typ</th>
                                <th class="pb-3 font-semibold">Pfade</th>
                                <th class="pb-3 font-semibold text-right">Aktionen</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-white/5">
                            ${libs.map(l => `
                            <tr>
                                <td class="py-3 font-bold text-white">${escapeHtml(l.name || l.Name)}</td>
                                <td class="py-3"><span class="px-2 py-0.5 rounded bg-white/10 text-white/70">${escapeHtml(l.content_type || l.ContentType || 'Mixed')}</span></td>
                                <td class="py-3 text-white/40 font-mono">${escapeHtml((l.paths || l.Paths || []).join(', '))}</td>
                                <td class="py-3 text-right space-x-2">
                                    <button class="text-[#00a4dc] hover:text-[#38bdf8]" onclick="API.scanLibrary('${l.id || l.Id}').then(()=>toast('Scan gestartet'))">Scannen</button>
                                    <button class="text-red-400 hover:text-red-300" onclick="if(confirm('Bibliothek wirklich entfernen?'))API.deleteLibrary('${l.id || l.Id}').then(()=>renderAdmin())">Löschen</button>
                                </td>
                            </tr>`).join('')}
                        </tbody>
                    </table>
                </div>
            </div>

            <!-- Tab 3: Tasks -->
            <div id="admin-tasks" class="hidden">
                <div class="flex items-center justify-between mb-4">
                    <h3 class="font-bold text-white text-base">Hintergrundaufgaben</h3>
                    <button class="jf-btn-ghost !py-1.5 !px-3 text-xs" onclick="refreshTasks()">🔄 Aktualisieren</button>
                </div>
                <div id="admin-tasks-list" class="grid gap-3">
                    ${tasks.map(t => renderTaskCard(t)).join('')}
                </div>
            </div>

            <!-- Tab 4: Sessions -->
            <div id="admin-sessions" class="hidden jf-settings-card">
                <h3 class="font-bold text-white text-base mb-4">Aktive Sitzungen</h3>
                <div class="overflow-x-auto">
                    <table class="w-full text-left text-xs">
                        <thead>
                            <tr class="text-white/40 border-b border-white/10">
                                <th class="pb-3 font-semibold">Benutzer</th>
                                <th class="pb-3 font-semibold">Gerät</th>
                                <th class="pb-3 font-semibold">Client</th>
                                <th class="pb-3 font-semibold">Aktivität</th>
                            </tr>
                        </thead>
                        <tbody class="divide-y divide-white/5">
                            ${sessions.map(s => `
                            <tr>
                                <td class="py-3 font-bold text-white">${escapeHtml(s.UserName || s.user_name || '–')}</td>
                                <td class="py-3 text-white/70">${escapeHtml(s.DeviceName || s.device_name || 'Browser')}</td>
                                <td class="py-3 text-white/50">${escapeHtml(s.Client || s.client || 'Rustyfin Web')}</td>
                                <td class="py-3 text-white/40">${escapeHtml(s.LastActivityDate || s.last_activity_date || '–')}</td>
                            </tr>`).join('')}
                        </tbody>
                    </table>
                </div>
            </div>

            <!-- Tab 5: Activity Log -->
            <div id="admin-activity" class="hidden jf-settings-card">
                <h3 class="font-bold text-white text-base mb-4">Aktivitätsprotokoll</h3>
                <div id="activity-content"><p class="text-white/40 text-xs">Lade Aktivitäten…</p></div>
            </div>

            <!-- Tab 6: Live Log -->
            <div id="admin-livelog" class="hidden">
                <div class="jf-settings-card">
                    <div class="flex items-center justify-between mb-4">
                        <h3 class="font-bold text-white text-base">Live-Log</h3>
                        <div class="flex items-center gap-3">
                            <span class="text-xs text-white/40">Verbosity:</span>
                            <select id="livelog-level" class="bg-white/10 text-xs text-white rounded-lg px-3 py-1.5 outline-none border border-white/10 cursor-pointer" onchange="setLogLevel(this.value)">
                                <option value="error">Error</option>
                                <option value="warn">Warn</option>
                                <option value="info" selected>Info</option>
                                <option value="debug">Debug</option>
                                <option value="trace">Trace</option>
                            </select>
                            <button onclick="clearLiveLog()" class="text-xs text-white/40 hover:text-white transition px-2 py-1 rounded hover:bg-white/10">Leeren</button>
                            <div id="livelog-status" class="flex items-center gap-1.5 text-xs">
                                <span class="w-2 h-2 rounded-full bg-red-400"></span>
                                <span class="text-white/40">Getrennt</span>
                            </div>
                        </div>
                    </div>
                    <div id="livelog-container" class="bg-black/50 rounded-xl border border-white/5 p-4 h-96 overflow-y-auto font-mono text-xs space-y-0.5">
                        <p class="text-white/30 italic">Verbinde mit Server…</p>
                    </div>
                    <p class="text-[10px] text-white/20 mt-2">Zeigt die letzten 500 Log-Einträge in Echtzeit. Änderungen der Verbosity werden sofort wirksam.</p>
                </div>
            </div>
        </div>`;
    } catch (e) {
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

function showAdminTab(btn, tabId) {
    btn.parentElement.querySelectorAll('button').forEach(b => {
        b.className = 'px-4 py-2 rounded-xl text-xs font-semibold whitespace-nowrap transition bg-white/5 text-white/60 hover:bg-white/10';
    });
    btn.className = 'px-4 py-2 rounded-xl text-xs font-semibold whitespace-nowrap transition bg-[#00a4dc] text-white shadow-lg';

    ['admin-overview', 'admin-users', 'admin-libs', 'admin-tasks', 'admin-sessions', 'admin-activity', 'admin-livelog'].forEach(id => {
        const el = $(id);
        if (el) el.classList.toggle('hidden', id !== tabId);
    });

    if (tabId === 'admin-activity') loadActivityLog();
    if (tabId === 'admin-livelog') initLiveLog();
}

// --- Scheduled Tasks UI ---

function getTaskStateLabel(state) {
    if (!state) return { text: 'Bereit', cls: 'bg-emerald-500/20 text-emerald-300' };
    const s = state.toLowerCase();
    if (s === 'running') return { text: 'Läuft', cls: 'bg-amber-500/20 text-amber-300 animate-pulse' };
    if (s === 'cancelling') return { text: 'Wird gestoppt', cls: 'bg-red-500/20 text-red-300' };
    return { text: 'Bereit', cls: 'bg-emerald-500/20 text-emerald-300' };
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

function renderTaskCard(t) {
    const id = t.Id || t.id;
    const name = t.Name || t.name || id;
    const description = t.Description || t.description || '';
    const state = t.State || t.state || 'Idle';
    const progress = t.CurrentProgressPercentage || t.current_progress_percentage;
    const category = t.Category || t.category || '';
    const lastResult = t.LastExecutionResult || t.last_execution_result;
    const triggers = t.Triggers || t.triggers || [];
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
        const timeStr = endTime ? new Date(endTime).toLocaleString('de-DE', { day: '2-digit', month: '2-digit', year: '2-digit', hour: '2-digit', minute: '2-digit' }) : '–';
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
                    return hours === 1 ? 'Stündlich' : `Alle ${hours}h`;
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
                <span>Fortschritt</span>
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
                    <span class="text-[10px] text-white/30">Letzte: ${lastExecHtml}</span>
                </div>
                ${progressHtml}
                <div class="flex items-center gap-2 mt-3">
                    ${isRunning || isCancelling ? `
                        <button class="jf-btn-danger text-xs !py-1.5 !px-3" onclick="stopTaskAction('${escapeHtml(id)}', '${escapeHtml(name)}')">
                            ⏹ Stoppen
                        </button>
                    ` : `
                        <button class="jf-btn-primary text-xs !py-1.5 !px-3" onclick="startTaskAction('${escapeHtml(id)}', '${escapeHtml(name)}')">
                            ▶ Starten
                        </button>
                    `}
                    <button class="jf-btn-ghost text-xs !py-1.5 !px-3 text-white/40" onclick="refreshTasks()" title="Status aktualisieren">
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
        toast(`„${name}" wird gestartet…`);
        await refreshTasks();
    } catch (e) {
        toast(`Fehler: ${e.message}`, 'error');
    }
}

async function stopTaskAction(id, name) {
    try {
        await API.stopTask(id);
        toast(`„${name}" wird gestoppt…`);
        await refreshTasks();
    } catch (e) {
        toast(`Fehler: ${e.message}`, 'error');
    }
}

async function refreshTasks() {
    try {
        const tasks = await API.getTasks();
        const list = $('admin-tasks-list');
        if (list) {
            list.innerHTML = tasks.map(t => renderTaskCard(t)).join('');
        }
    } catch (e) {
        toast(`Tasks konnten nicht geladen werden: ${e.message}`, 'error');
    }
}

async function loadActivityLog() {
    try {
        const data = await API.getActivityLog(0, 50);
        const items = data.Items || data.items || [];
        if (!items.length) {
            html('activity-content', '<p class="text-white/40 text-xs">Keine Aktivitäten protokolliert.</p>');
            return;
        }
        html('activity-content', `
        <div class="overflow-x-auto">
            <table class="w-full text-left text-xs">
                <thead>
                    <tr class="text-white/40 border-b border-white/10">
                        <th class="pb-2 font-semibold">Zeit</th>
                        <th class="pb-2 font-semibold">Typ</th>
                        <th class="pb-2 font-semibold">Details</th>
                    </tr>
                </thead>
                <tbody class="divide-y divide-white/5">
                    ${items.map(a => `
                    <tr>
                        <td class="py-2.5 text-white/40 whitespace-nowrap font-mono">${escapeHtml(a.Date || a.date || '–')}</td>
                        <td class="py-2.5"><span class="px-2 py-0.5 rounded bg-white/10 text-white/70">${escapeHtml(a.Type || a.type || 'Info')}</span></td>
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
    try {
        libraries = await API.getLibraries();
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
            const isScanning = scanStatuses[id];
            list.innerHTML += `
            <div class="flex items-center gap-1 group">
                <a onclick="navigate('library','${id}')" class="jf-drawer-item ${currentLibraryId === id && currentPage === 'library' ? 'active' : ''} flex-1">
                    <span class="text-base">${icons[type] || '📁'}</span>
                    <span class="truncate">${escapeHtml(name)}</span>
                    ${isScanning ? '<span class="w-1.5 h-1.5 rounded-full bg-amber-400 animate-pulse ml-1 flex-shrink-0"></span>' : ''}
                </a>
                <button type="button" class="jf-icon-btn !w-6 !h-6 text-white/20 hover:text-white opacity-0 group-hover:opacity-100 transition flex-shrink-0" onclick="API.scanLibrary('${id}').then(()=>toast('Scan für ${escapeHtml(name)} gestartet'))" title="Bibliothek scannen">
                    <svg class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24"><path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"/></svg>
                </button>
            </div>`;
        });

        if (API.isAdmin) {
            show('sidebar-add-lib');
            show('sidebar-admin-section');
        } else {
            hide('sidebar-add-lib');
            hide('sidebar-admin-section');
        }
    } catch {}
}

// --- Add Library Modal Handlers ---
function openAddLibrary() {
    if (!API.isLoggedIn()) {
        toast('Bitte zuerst anmelden', 'error');
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

// Directory picker state
let directoryPickerPath = '/';
let directoryPickerHistory = ['/'];
let selectedLibraryPaths = [];

async function submitAddLibrary(e) {
    e.preventDefault();
    const f = e.target;
    const name = f.name.value.trim();
    const type = f.type.value;
    // Combine selected paths from picker with manual input
    const manualPaths = f.paths.value.split(',').map(p => p.trim()).filter(Boolean);
    const paths = [...new Set([...selectedLibraryPaths, ...manualPaths])];

    if (!name || !paths.length) {
        toast('Name und mindestens ein Pfad erforderlich', 'error');
        return;
    }
    try {
        const lib = await API.createLibrary(name, type, paths);
        toast(`Bibliothek „${name}" angelegt – Scan wird gestartet…`);
        closeAddLibrary();
        // Reset selected paths
        selectedLibraryPaths = [];
        // Auto-start scan after creating library
        API.scanLibrary(lib.id || lib.Id).catch(() => {});
        await loadSidebar();
        if (currentPage === 'admin') renderAdmin();
        else if (currentPage === 'home') renderHome();
    } catch (err) {
        toast(err.message, 'error');
    }
}

// --- Directory Picker ---
async function openDirectoryPicker() {
    directoryPickerPath = '/';
    directoryPickerHistory = ['/'];
    const picker = $('directory-picker');
    const input = $('add-lib-path-input');
    if (picker) picker.classList.remove('hidden');
    if (input) input.classList.add('hidden');
    await loadDirectoryList('/');
}

function closeDirectoryPicker() {
    const picker = $('directory-picker');
    const input = $('add-lib-path-input');
    if (picker) picker.classList.add('hidden');
    if (input) input.classList.remove('hidden');
}

function directoryPickerBack() {
    if (directoryPickerHistory.length > 1) {
        directoryPickerHistory.pop();
        directoryPickerPath = directoryPickerHistory[directoryPickerHistory.length - 1];
        loadDirectoryList(directoryPickerPath);
    }
}

async function loadDirectoryList(path) {
    const list = $('directory-picker-list');
    const pathLabel = $('directory-picker-path');
    if (pathLabel) pathLabel.textContent = path;
    if (!list) return;

    list.innerHTML = '<div class="p-3 text-center text-white/40 text-xs"><div class="w-4 h-4 border-2 border-[#00a4dc] border-t-transparent rounded-full animate-spin mx-auto"></div></div>';

    try {
        const dirs = await API.browseDirectories(path);
        if (!dirs.length) {
            list.innerHTML = '<div class="p-3 text-center text-white/30 text-xs">Keine Unterverzeichnisse</div>';
            return;
        }

        let html = '';
        // Parent navigation (unless root)
        if (path !== '/') {
            const parent = path.split('/').slice(0, -1).join('/') || '/';
            html += `<button type="button" class="w-full text-left px-3 py-2 text-xs text-white/60 hover:bg-white/5 flex items-center gap-2 rounded" onclick="navigateDirectoryPicker('${parent}')">
                <span>📁</span> <span>…</span> <span class="text-white/30 text-[10px] ml-auto">Elternordner</span>
            </button>`;
        }

        for (const dir of dirs) {
            if (!dir.can_read) {
                html += `<div class="w-full text-left px-3 py-2 text-xs text-white/40 flex items-center gap-2 rounded">
                    <span>📁</span> <span class="truncate">${escapeHtml(dir.name)}</span>
                    <span class="text-white/30 text-[10px] ml-auto">Kein Zugriff</span>
                </div>`;
            } else {
                // Click navigates into the directory, double-click or button selects it
                const escapedPath = dir.path.replace(/'/g, "\\'");
                html += `<div class="flex items-center gap-1">
                    <button type="button" class="flex-1 text-left px-3 py-2 text-xs text-white hover:bg-white/5 flex items-center gap-2 rounded" onclick="navigateDirectoryPicker('${escapedPath}')">
                        <span>📁</span> <span class="truncate">${escapeHtml(dir.name)}</span>
                    </button>
                    <button type="button" class="px-2 py-2 text-[10px] text-[#00a4dc] hover:text-[#38bdf8] rounded" onclick="selectDirectoryPath('${escapedPath}')" title="Diesen Ordner auswählen">✓</button>
                </div>`;
            }
        }
        list.innerHTML = html;
    } catch (e) {
        list.innerHTML = `<div class="p-3 text-center text-red-400 text-xs">${escapeHtml(e.message)}</div>`;
    }
}

function navigateDirectoryPicker(path) {
    directoryPickerHistory.push(path);
    directoryPickerPath = path;
    loadDirectoryList(path);
}

function selectDirectoryPath(path) {
    if (!selectedLibraryPaths.includes(path)) {
        selectedLibraryPaths.push(path);
    }
    // Update the input field with all selected paths
    const input = $('add-lib-path-input');
    if (input) input.value = selectedLibraryPaths.join(', ');
    // Show selected paths
    renderSelectedPaths();
    // Close picker
    closeDirectoryPicker();
    toast(`Pfad hinzugefügt: ${path}`);
}

function renderSelectedPaths() {
    const container = $('add-lib-selected-paths');
    if (!container) return;
    if (!selectedLibraryPaths.length) {
        container.innerHTML = '';
        return;
    }
    container.innerHTML = selectedLibraryPaths.map((p, i) => `
        <div class="flex items-center gap-2 px-2 py-1 rounded bg-[#00a4dc]/10 border border-[#00a4dc]/20">
            <span class="text-xs text-white/70 font-mono truncate flex-1">${escapeHtml(p)}</span>
            <button type="button" class="text-white/40 hover:text-red-400 text-xs" onclick="removeSelectedPath(${i})">×</button>
        </div>
    `).join('');
}

function removeSelectedPath(index) {
    selectedLibraryPaths.splice(index, 1);
    const input = $('add-lib-path-input');
    if (input) input.value = selectedLibraryPaths.join(', ');
    renderSelectedPaths();
}

// Override openAddLibrary to reset state
const _origOpenAddLibrary = openAddLibrary;
openAddLibrary = function() {
    selectedLibraryPaths = [];
    const picker = $('directory-picker');
    if (picker) picker.classList.add('hidden');
    _origOpenAddLibrary();
};

// --- Scan Progress ---
let scanProgressTimeout = null;

function showScanProgress(message) {
    const el = $('scan-progress');
    const textEl = $('scan-progress-text');
    if (el) el.classList.remove('hidden');
    if (textEl) textEl.textContent = message;
    
    // Clear any existing timeout
    if (scanProgressTimeout) {
        clearTimeout(scanProgressTimeout);
        scanProgressTimeout = null;
    }
}

function hideScanProgress() {
    const el = $('scan-progress');
    if (el) el.classList.add('hidden');
    
    // Auto-hide after 3 seconds
    scanProgressTimeout = setTimeout(() => {
        if (el) el.classList.add('hidden');
        scanProgressTimeout = null;
    }, 3000);
}

// --- WebSocket for scan progress ---
function initScanProgressWebSocket() {
    if (typeof WebSocket === 'undefined') return;
    
    const protocol = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
    const wsUrl = `${protocol}//${window.location.host}/websocket?api_key=${API.accessToken}`;
    
    let ws;
    let reconnectDelay = 1000;
    
    function connect() {
        try {
            ws = new WebSocket(wsUrl);
            
            ws.onmessage = (event) => {
                try {
                    const data = JSON.parse(event.data);
                    if (data.MessageType === 'ScheduledTaskStarted') {
                        showScanProgress(data.data.Name || 'Wird verarbeitet…');
                        // Track per-library scan status
                        const libId = data.data.ItemId;
                        if (libId) scanStatuses[libId] = true;
                        // Refresh tasks tab if visible
                        if (currentPage === 'admin') refreshTasks();
                    } else if (data.MessageType === 'ScheduledTaskCompleted') {
                        // Clear per-library scan status
                        const libId = data.data.ItemId;
                        if (libId) delete scanStatuses[libId];
                        if (data.data.State === 'Completed') {
                            hideScanProgress();
                        } else if (data.data.State === 'Failed') {
                            hideScanProgress();
                            toast(`Fehler: ${data.data.Name || 'Aufgabe'} - ${data.data.Error || 'Unbekannter Fehler'}`, 'error');
                        }
                        // Refresh tasks tab if visible
                        if (currentPage === 'admin') refreshTasks();
                    } else if (data.MessageType === 'LibraryChanged') {
                        // Library changed - refresh current view if on library page
                        if (currentPage === 'library' && currentLibraryId) {
                            setTimeout(() => renderLibrary(currentLibraryId), 500);
                        } else if (currentPage === 'home') {
                            setTimeout(() => renderHome(), 500);
                        }
                    }
                } catch (e) {
                    // Ignore parse errors
                }
            };
            
            ws.onclose = () => {
                // Reconnect with exponential backoff
                setTimeout(connect, reconnectDelay);
                reconnectDelay = Math.min(reconnectDelay * 2, 30000);
            };
            
            ws.onerror = () => {
                ws.close();
            };
            
            reconnectDelay = 1000;
        } catch (e) {
            // WebSocket not available, silently fail
        }
    }
    
    if (API.isLoggedIn()) {
        connect();
    }
}

// --- Add User Modal Handlers ---
function openAddUser() {
    if (!API.isLoggedIn() || !API.isAdmin) {
        toast('Nur Administratoren können Benutzer erstellen', 'error');
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
        toast('Name und Passwort sind erforderlich', 'error');
        return;
    }
    try {
        await API.createUser(name, password, isAdmin);
        toast(`Benutzer „${name}" erstellt`);
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
                statusEl.innerHTML = '<span class="w-2 h-2 rounded-full bg-emerald-400"></span><span class="text-emerald-400">Verbunden</span>';
            } else {
                statusEl.innerHTML = '<span class="w-2 h-2 rounded-full bg-amber-400"></span><span class="text-amber-400">Verbinde…</span>';
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
            timeStr = d.toLocaleTimeString('de-DE', { hour: '2-digit', minute: '2-digit', second: '2-digit', fractionalSecondDigits: 3 });
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
        container.innerHTML = '<p class="text-white/30 italic">Log geleert. Warte auf neue Einträge…</p>';
    }
    if (typeof LiveLog !== 'undefined') {
        LiveLog.messages = [];
    }
}

async function setLogLevel(level) {
    try {
        await API.setLogLevel(level);
        toast(`Log-Level auf "${level}" gesetzt`);
    } catch (e) {
        toast('Fehler beim Setzen des Log-Levels', 'error');
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
