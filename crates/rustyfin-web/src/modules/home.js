// The home page and favourites
import { $, icon, toast, escapeHtml, scanRing } from './util.js';
import { backdropUrl, createCardElement, sectionRow } from './cards.js';
import { serverVersion } from './wizard.js';
import { LIBRARY_PAGE_SIZE, setupInfiniteScroll, destroyInfiniteScroll } from './library.js';

// --- Home Page Renderer ---

// Jellyfin Web's home rows: a resume row per media type, and a "Recently Added in" row per library
// except the kinds it leaves out; music shows more items, as its cards are smaller
const LATEST_EXCLUDED_TYPES = ['playlists', 'livetv', 'boxsets', 'channels', 'folders'];

function resumeUrl(mediaType) {
    return `/Items/Resume?Limit=12&MediaTypes=${mediaType}`;
}

function homeLatestPlan(libs) {
    return (libs || [])
        .map(lib => ({
            id: lib.id || lib.Id,
            name: lib.name || lib.Name || '',
            type: lib.content_type || lib.ContentType || lib.CollectionType || '',
        }))
        .filter(lib => lib.id && !LATEST_EXCLUDED_TYPES.includes(String(lib.type).toLowerCase()))
        .map(lib => ({
            id: lib.id,
            name: lib.name,
            url: `/Items/Latest?ParentId=${encodeURIComponent(lib.id)}&Limit=${String(lib.type).toLowerCase() === 'music' ? 30 : 16}&Fields=PrimaryImageAspectRatio`,
        }));
}

// Each library's latest items, in library order; a library whose request fails shows no row
function fetchLatestRows(libs) {
    return Promise.all(homeLatestPlan(libs).map(row => API.cachedGet(row.url)
        .then(data => ({ ...row, items: Array.isArray(data) ? data : (data.Items || []) }))
        .catch(() => ({ ...row, items: [] }))));
}

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
        const libsRequest = API.getLibraries().catch(err => { libsError = err; return null; });
        const [resumeData, resumeAudioData, nextUpData, latestRows, libsData] = await Promise.all([
            API.cachedGet(resumeUrl('Video')).catch(() => ({ Items: [] })),
            API.cachedGet(resumeUrl('Audio')).catch(() => ({ Items: [] })),
            (API.getNextUp ? API.getNextUp(12) : Promise.resolve({ Items: [] })).catch(() => ({ Items: [] })),
            libsRequest.then(fetchLatestRows),
            libsRequest,
        ]);
        if (API.token !== session) return;

        const resume = resumeData.Items || resumeData.items || [];
        const resumeAudio = resumeAudioData.Items || resumeAudioData.items || [];
        const nextUp = nextUpData.Items || nextUpData.items || [];
        const latest = latestRows.find(row => row.items.length)?.items || [];
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
                        ${spRating ? `<span class="jf-badge jf-badge-rating inline-flex items-center gap-0.5 text-[11px]">${icon('Star', { size: 10, class: 'fill-current' })} ${spRating.toFixed(1)}</span>` : ''}
                    </div>
                    <h1 class="text-3xl sm:text-4xl lg:text-5xl font-black text-white tracking-tight mb-3 drop-shadow-md">${escapeHtml(spName)}</h1>
                    ${spOverview ? `<p class="text-white/80 text-xs sm:text-sm line-clamp-3 mb-6 max-w-2xl leading-relaxed">${escapeHtml(spOverview)}</p>` : ''}
                    <div class="flex flex-wrap items-center gap-3">
                        ${canPlay ? `
                        <button class="jf-btn-primary !py-2.5 !px-6 text-sm" data-action="play" data-id="${escapeHtml(spId)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(spName)}">
                            ${icon('Play', { size: 16, class: 'fill-current' })}
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
                        const icons = { Movies: 'Film', TvShows: 'Tv', Music: 'Music', Photos: 'Image', Mixed: 'Folder' };
                        return `
                        <div class="jf-card p-5 bg-gradient-to-br ${gradients[libType] || gradients.Mixed} flex flex-col justify-between min-h-[120px] relative" data-action="navigate" data-page="library" data-id="${escapeHtml(libId)}">
                            <div class="flex items-center justify-between">
                                <span class="inline-flex">${icon(icons[libType] || 'Folder', { size: 28 })}</span>
                                <div class="flex items-center gap-1">
                                    <button type="button" class="w-6 h-6 rounded flex items-center justify-center text-white/40 hover:text-white hover:bg-white/10 transition" data-action="scan-library" data-id="${escapeHtml(libId)}" data-name="${escapeHtml(libName)}" title="${escapeHtml(t('library.scan_library'))}" aria-label="${escapeHtml(t('library.scan_library'))}">
                                        ${icon('RefreshCw', { size: 14 })}
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

        // 3. Sections: Continue Watching, Continue Listening, Next Up, then each library's latest
        c += sectionRow(t('home.continue_watching'), resume, null, { backdrop: true });
        c += sectionRow(t('home.continue_listening'), resumeAudio);
        c += sectionRow(t('home.next_up'), nextUp, null, { backdrop: true });
        for (const row of latestRows) {
            c += sectionRow(t('home.latest_in', { name: row.name }), row.items, { page: 'library', data: row.id });
        }

        // Empty Library Welcome Banner
        if (!resume.length && !resumeAudio.length && !nextUp.length && !latest.length) {
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
                <div id="fav-grid" class="jf-card-grid grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4"></div>
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
                    <div class="flex justify-center mb-3 text-white/30">${icon('Heart', { size: 40 })}</div>
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

export { renderHome, renderFavorites };
