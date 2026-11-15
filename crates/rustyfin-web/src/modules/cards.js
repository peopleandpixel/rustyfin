// Card and row builders shared by the home, library, detail and search pages
import { icon, escapeHtml } from './util.js';

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
        Movie: 'Film', Series: 'Tv', Episode: 'Tv', Audio: 'Music', MusicAlbum: 'Disc3',
        MusicArtist: 'MicVocal', Folder: 'Folder', CollectionFolder: 'Library', Photo: 'Image'
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
            <div class="${picture ? 'hidden' : 'flex'} items-center justify-center h-full text-white/20 absolute inset-0 bg-white/5"${picture ? ' style="display:none"' : ''}>
                ${icon(typeIcons[type] || 'Folder', { size: 40 })}
            </div>

            <!-- Top Badges -->
            <div class="absolute top-2 left-2 right-2 flex items-center justify-between pointer-events-none z-10">
                ${fav ? `<span class="inline-flex text-rose-500 bg-black/60 backdrop-blur-md p-1 rounded-full text-xs leading-none">${icon('Heart', { size: 12, class: 'fill-current' })}</span>` : '<span></span>'}
                ${rating ? `<span class="jf-badge jf-badge-rating inline-flex items-center gap-0.5 text-[10px] !py-0.5 !px-1.5 shadow-md">${icon('Star', { size: 10, class: 'fill-current' })} ${rating.toFixed(1)}</span>` : ''}
            </div>

            <!-- Watched checkmark badge -->
            ${played ? `
            <div class="absolute top-2 right-2 w-6 h-6 rounded-full bg-[#00a4dc] text-white flex items-center justify-center shadow-lg border border-white/20 z-10">
                ${icon('Check', { size: 14 })}
            </div>` : ''}

            <!-- Play Button Hover Overlay -->
            ${canPlay ? `
            <div class="jf-play-overlay" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(rawName || t('common.unknown'))}">
                <div class="jf-play-btn-circle">
                    ${icon('Play', { size: 28, class: 'text-white ml-1 fill-current' })}
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

export { backdropUrl, createCardElement, sectionRow };
