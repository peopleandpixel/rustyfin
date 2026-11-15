// Item and person detail pages, favourite and played toggles
import { $, initLazyImages, icon, toast, escapeHtml, formatRuntime } from './util.js';
import { backdropUrl, createCardElement, sectionRow } from './cards.js';
import { resumePositionTicks } from './player.js';
let currentDetailId = null;
let activeItem = null;

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
                                            ${icon('Play', { size: 20, class: 'fill-current ml-0.5' })}
                                        </div>
                                    </div>
                                    ${epPlayed ? `<span class="absolute top-2 right-2 w-5 h-5 rounded-full bg-[#00a4dc] text-white flex items-center justify-center text-[10px]">${icon('Check', { size: 12 })}</span>` : ''}
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
                            ${rating ? `<span class="jf-badge jf-badge-rating inline-flex items-center gap-0.5 text-[11px]">${icon('Star', { size: 10, class: 'fill-current' })} ${rating.toFixed(1)}</span>` : ''}
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
                            <button class="jf-btn-primary h-11 !py-0 !px-7 text-sm font-bold shadow-xl" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(name)}"${resumeTicks > 0 ? ' data-resume="true"' : ''}>
                                ${icon('Play', { size: 20, class: 'fill-current' })}
                                ${resumeTicks > 0 ? t('detail.resume') : t('common.play')}
                            </button>
                            ${resumeTicks > 0 ? `
                            <button class="jf-btn-ghost h-11 !py-0 !px-4 text-sm" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(name)}" data-from-start="true">
                                ${t('detail.play_from_beginning')}
                            </button>` : ''}` : ''}

                            <button class="jf-btn-ghost h-11 !py-0 w-11 !px-0 sm:w-auto sm:!px-4 text-sm" title="${escapeHtml(t('detail.toggle_favorite'))}" aria-label="${escapeHtml(t('detail.toggle_favorite'))}" data-action="toggle-favorite" data-id="${escapeHtml(id)}" data-favorite="${fav}">
                                ${icon('Heart', { size: 18, class: fav ? 'text-rose-500 fill-current' : 'text-white/60' })}
                                <span class="hidden sm:inline text-xs ml-1">${fav ? t('detail.favorite') : t('detail.add_favorite')}</span>
                            </button>

                            <button class="jf-btn-ghost h-11 !py-0 !px-4 text-xs font-semibold" data-action="toggle-played" data-id="${escapeHtml(id)}" data-played="${played}">
                                ${played ? `<span class="inline-flex items-center gap-1 text-emerald-400">${icon('Check', { size: 14 })} ${t('detail.played')}</span>` : `<span class="inline-flex items-center gap-1 text-white/60">${icon('Circle', { size: 14 })} ${t('detail.unplayed')}</span>`}
                            </button>

                            ${API.isAdmin ? `
                            <button class="jf-btn-ghost h-11 !py-0 w-11 !px-0" title="${escapeHtml(t('detail.refresh_metadata'))}" aria-label="${escapeHtml(t('detail.refresh_metadata'))}" data-action="refresh-item" data-id="${escapeHtml(id)}">
                                ${icon('RefreshCw', { size: 18, class: 'jf-busy-spin' })}
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

export { currentDetailId, renderDetail, renderPerson, toggleFavorite, togglePlayed };
