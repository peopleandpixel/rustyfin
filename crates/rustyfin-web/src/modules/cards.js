// Card and row builders shared by the home, library, detail and search pages
import { icon, escapeHtml } from './util.js';
import { blurhashTint } from './library.js';

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

// A banner card's picture: the Banner, else the landscape picks (Jellyfin Web shows the title instead)
function bannerImage(item) {
    const id = item.Id || item.id;
    const tag = item.ImageTags?.Banner;
    return tag ? { id, type: 'Banner', tag, contain: false } : cardImage(item, true);
}

// What a card's badges and buttons show of the user's data, as data-* values on the card, so a mark
// made from any card can redraw them on every card of the item (applyCardUserData)
const isFolderItem = item => !!item.IsFolder || ['Series', 'Season', 'BoxSet', 'MusicAlbum', 'Playlist'].includes(item.Type || item.type);

function cardState(item) {
    const data = item.UserData || {};
    const folder = isFolderItem(item);
    return {
        played: !!(data.Played || item.user_data?.played),
        favorite: !!(data.IsFavorite || item.user_data?.is_favorite),
        progress: data.PlayedPercentage || item.user_data?.played_percentage || item.CompletionPercentage || 0,
        // Jellyfin Web counts a folder's unplayed items in the corner; a played folder shows the check
        unplayed: folder ? data.UnplayedItemCount || 0 : 0,
        likes: data.Likes ?? null,
    };
}

const STATE_ATTRS = ['played', 'favorite', 'progress', 'unplayed', 'likes'];
function stateAttrs(state) {
    return STATE_ATTRS.map(key => ` data-${key}="${escapeHtml(String(state[key]))}"`).join('');
}

function stateFromDataset(ds) {
    return {
        played: ds.played === 'true',
        favorite: ds.favorite === 'true',
        progress: Number(ds.progress) || 0,
        unplayed: Number(ds.unplayed) || 0,
        likes: ds.likes === 'true' ? true : ds.likes === 'false' ? false : null,
    };
}

// The marks over the picture: favourite and rating on the left, the played check or the unplayed
// count on the right (Jellyfin Web's indicators), and the progress bar of a started item
function cardMarksHtml(state, rating) {
    const right = state.played
        ? `<span class="jf-card-check" title="${escapeHtml(t('detail.played'))}">${icon('Check', { size: 14 })}</span>`
        : state.unplayed > 0
            ? `<span class="jf-card-count" title="${escapeHtml(t('cards.unplayed_count', { count: state.unplayed }))}">${state.unplayed > 99 ? '99+' : state.unplayed}</span>`
            : '';
    return `<div class="jf-card-marks">
                <div class="absolute top-2 left-2 right-2 flex items-start justify-between gap-1">
                    <span class="flex items-center gap-1">
                        ${state.favorite ? `<span class="inline-flex text-rose-500 bg-black/60 p-1 rounded-full text-xs leading-none">${icon('Heart', { size: 12, class: 'fill-current' })}</span>` : ''}
                        ${rating ? `<span class="jf-badge jf-badge-rating inline-flex items-center gap-0.5 text-[10px] !py-0.5 !px-1.5 shadow-md">${icon('Star', { size: 10, class: 'fill-current' })} ${rating.toFixed(1)}</span>` : ''}
                    </span>
                    ${right}
                </div>
                ${state.progress > 0 && state.progress < 100 && !state.played ? `
                <div class="jf-progress">
                    <div class="jf-progress-bar" style="width:${state.progress}%"></div>
                </div>` : ''}
            </div>`;
}

// Jellyfin Web's card buttons: played, favourite and the context menu. Shown on hover (the menu
// button always on touch screens); the router dispatches them, so a click doesn't open the item.
function cardActionsHtml(id, state) {
    const button = (action, label, iconName, pressed, extra = '') => `<button type="button" class="jf-card-action${pressed ? ' is-on' : ''}" data-action="${action}" data-id="${escapeHtml(id)}" aria-label="${escapeHtml(label)}" title="${escapeHtml(label)}"${pressed === null ? '' : ` aria-pressed="${pressed}"`}${extra}>${icon(iconName, { size: 16 })}</button>`;
    return `<div class="jf-card-actions">
                ${button('card-played', state.played ? t('detail.mark_unplayed') : t('detail.mark_played'), 'Check', state.played)}
                ${button('card-favorite', state.favorite ? t('cards.remove_favorite') : t('detail.add_favorite'), 'Heart', state.favorite)}
                ${button('card-menu', t('cards.more'), 'EllipsisVertical', null, ' aria-haspopup="menu"')}
            </div>`;
}

// Draws again the marks and buttons of every card of item `id` after a change of its user data
// (a partial UserData: Played, IsFavorite, PlayedPercentage, UnplayedItemCount, Likes)
function applyCardUserData(id, data, root = document) {
    for (const card of root.querySelectorAll(`.jf-card[data-id="${CSS.escape(String(id))}"]`)) {
        const state = stateFromDataset(card.dataset);
        if (data.Played !== undefined) {
            state.played = !!data.Played;
            if (state.played) {
                state.progress = 0;
                state.unplayed = 0;
            }
        }
        if (data.IsFavorite !== undefined) state.favorite = !!data.IsFavorite;
        if (data.PlayedPercentage !== undefined) state.progress = data.PlayedPercentage || 0;
        if (data.UnplayedItemCount !== undefined && card.dataset.folder === 'true') state.unplayed = data.UnplayedItemCount || 0;
        if (data.Likes !== undefined) state.likes = data.Likes;
        for (const key of STATE_ATTRS) card.dataset[key] = String(state[key]);
        const rating = Number(card.dataset.rating) || null;
        const marks = card.querySelector('.jf-card-marks');
        if (marks) marks.outerHTML = cardMarksHtml(state, rating);
        const actions = card.querySelector('.jf-card-actions');
        if (actions) actions.outerHTML = cardActionsHtml(id, state);
    }
}

// Jellyfin Web's list view row: a small picture, the title with its year and type, and the buttons
function itemListRow(item) {
    const rawName = item.Name || item.name || '';
    const type = item.Type || item.type || '';
    const id = item.Id || item.id;
    const year = item.ProductionYear || item.production_year || '';
    const state = cardState(item);
    const picture = cardImage(item, false);
    const isEpisode = type === 'Episode' && item.SeriesName;
    const title = isEpisode ? item.SeriesName : rawName || t('common.unknown');
    const sub = isEpisode && item.ParentIndexNumber != null && item.IndexNumber != null
        ? t('home.episode_line', { season: item.ParentIndexNumber, episode: item.IndexNumber, name: rawName })
        : [year, type].filter(Boolean).join(' · ');
    const minutes = item.RunTimeTicks ? Math.round(item.RunTimeTicks / 600000000) : 0;
    return `
    <div class="jf-card jf-list-row group" data-action="navigate" data-page="detail" data-id="${escapeHtml(id)}" data-type="${escapeHtml(type)}" data-folder="${isFolderItem(item)}" data-rating=""${stateAttrs(state)}>
        <div class="jf-list-thumb">
            ${picture ? `<img alt="" decoding="async" class="jf-thumb-img absolute inset-0 w-full h-full object-cover z-[1] opacity-0" data-img-id="${escapeHtml(picture.id)}" data-img-type="${escapeHtml(picture.type)}" data-img-tag="${escapeHtml(picture.tag)}" onload="posterLoaded(this)" onerror="posterFailed(this)">` : ''}
            ${cardMarksHtml({ ...state, favorite: false }, null)}
        </div>
        <div class="min-w-0 flex-1">
            <p class="text-sm font-semibold truncate text-white group-hover:text-[#38bdf8]" title="${escapeHtml(title)}">${escapeHtml(title)}</p>
            <p class="text-xs text-white/40 truncate">${escapeHtml(sub)}${minutes ? ` · ${escapeHtml(t('cards.minutes', { count: minutes }))}` : ''}</p>
        </div>
        ${cardActionsHtml(id, state)}
    </div>`;
}

// options.backdrop: a landscape (thumb) card; options.banner: a banner card; options.list: a list row
function itemCard(item, options = {}) {
    if (options.list) return itemListRow(item);
    // rawName goes into the onclick code, which translates the fallback itself
    const rawName = item.Name || item.name || '';
    const name = rawName || t('common.unknown');
    const year = item.ProductionYear || item.production_year || '';
    const type = item.Type || item.type || '';
    const id = item.Id || item.id;
    const state = cardState(item);
    const rating = item.CommunityRating || item.community_rating;
    // Use explicit backdrop option if provided; otherwise derive from type
    const isBanner = !!options.banner;
    const isBackdrop = isBanner || (options.backdrop !== undefined ? options.backdrop : type === 'Episode');

    // One picture per card, sized by the poster loader for the card's width when it starts
    const picture = isBanner ? bannerImage(item) : cardImage(item, isBackdrop);
    // The picture's BlurHash, painted under it until it loads (Jellyfin's ImageBlurHashes shape)
    const blurhash = picture ? item.ImageBlurHashes?.[picture.type]?.[picture.tag] : null;
    // Under the BlurHash: its average colour (as the `background` shorthand, which drops the poster
    // box's dark gradient), so the box is never dark before the BlurHash is painted
    const tint = blurhash ? blurhashTint(blurhash) : null;
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
    <div class="jf-card group" data-action="navigate" data-page="detail" data-id="${escapeHtml(id)}" data-type="${escapeHtml(type)}" data-folder="${isFolderItem(item)}" data-rating="${rating || ''}"${stateAttrs(state)}>
        <div class="${isBanner ? 'jf-poster-banner' : isBackdrop ? 'jf-poster-backdrop' : 'jf-poster'}"${tint ? ` style="background:${tint}"` : ''}>
            ${picture ? `
            ${blurhash
                ? `<div class="jf-blurhash" data-w="${isBackdrop ? 32 : 20}" data-h="${isBackdrop ? 18 : 30}" data-blurhash="${escapeHtml(blurhash)}" aria-hidden="true"></div>`
                : '<div class="jf-poster-skeleton"></div>'}
            <img alt="${escapeHtml(cardTitle)}" decoding="async" class="jf-thumb-img absolute inset-0 w-full h-full object-cover z-[1] opacity-0 transition-opacity duration-200${picture.contain ? ' jf-img-contain' : ''}"
                 data-img-id="${escapeHtml(picture.id)}" data-img-type="${escapeHtml(picture.type)}" data-img-tag="${escapeHtml(picture.tag)}" onload="posterLoaded(this)" onerror="posterFailed(this)">` : ''}
            <div class="${picture ? 'hidden' : 'flex'} items-center justify-center h-full text-white/20 absolute inset-0 bg-white/5"${picture ? ' style="display:none"' : ''}>
                ${icon(typeIcons[type] || 'Folder', { size: 40 })}
            </div>

            <!-- Badges: favourite, rating, played or unplayed count, progress -->
            ${cardMarksHtml(state, rating)}

            <!-- Play Button Hover Overlay -->
            ${canPlay ? `
            <div class="jf-play-overlay" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(rawName || t('common.unknown'))}">
                <div class="jf-play-btn-circle">
                    ${icon('Play', { size: 28, class: 'text-white ml-1 fill-current' })}
                </div>
            </div>` : ''}

            ${cardActionsHtml(id, state)}
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

export {
    cardImage, bannerImage, backdropUrl, cardState, cardMarksHtml, cardActionsHtml, applyCardUserData, itemListRow,
    itemCard, createCardElement, sectionRow,
};
