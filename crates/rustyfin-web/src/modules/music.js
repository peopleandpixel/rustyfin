// Music: the album page's track list, and a music library's tabs (albums, album artists, artists,
// songs and genres)
import { $, icon, toast, escapeHtml } from './util.js';
import { createCardElement } from './cards.js';
import { setupInfiniteScroll, destroyInfiniteScroll, addInfiniteScrollSentinel } from './library.js';

// --- Album track list ---

// A track's length as Jellyfin Web shows it: m:ss, or h:mm:ss from an hour on
function trackDuration(ticks) {
    if (!ticks) return '';
    const total = Math.round(ticks / 10000000);
    const h = Math.floor(total / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = String(total % 60).padStart(2, '0');
    return h ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

// An album's tracks by disc, each disc in track order (the server sends them unsorted). Tracks
// without a disc number go with disc 1, those without a track number after the numbered ones.
function trackDiscGroups(tracks) {
    const num = (v, missing) => (Number.isFinite(v) ? v : missing);
    const sorted = [...tracks].sort((a, b) =>
        num(a.ParentIndexNumber, 1) - num(b.ParentIndexNumber, 1)
        || num(a.IndexNumber, Infinity) - num(b.IndexNumber, Infinity)
        || String(a.SortName || a.Name || '').localeCompare(String(b.SortName || b.Name || '')));
    const groups = [];
    for (const track of sorted) {
        const disc = num(track.ParentIndexNumber, 1);
        if (groups.at(-1)?.disc !== disc) groups.push({ disc, tracks: [] });
        groups.at(-1).tracks.push(track);
    }
    return groups;
}

// One track row; a click plays it. The artists show only where they differ from the album artist
// (a compilation's tracks), as on Jellyfin Web's album page.
function trackRow(track, { albumArtist = '', showAlbum = false } = {}) {
    const id = track.Id || track.id;
    const name = track.Name || track.name || '';
    const artists = (track.Artists || []).join(', ');
    const sub = [
        artists && artists !== albumArtist ? artists : '',
        showAlbum ? track.Album || '' : '',
    ].filter(Boolean).join(' · ');
    const played = track.UserData?.Played;
    return `
    <div class="jf-track-row group" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="Audio" data-title="${escapeHtml(name)}">
        <span class="jf-track-number">
            <span class="group-hover:hidden">${escapeHtml(track.IndexNumber ?? '')}</span>
            <span class="hidden group-hover:inline-flex">${icon('Play', { size: 14, class: 'fill-current' })}</span>
        </span>
        <span class="min-w-0 flex-1">
            <span class="block truncate text-sm text-white">${escapeHtml(name)}</span>
            ${sub ? `<span class="block truncate text-xs text-white/50">${escapeHtml(sub)}</span>` : ''}
        </span>
        ${played ? `<span class="text-[#00a4dc]" title="${escapeHtml(t('detail.played'))}">${icon('Check', { size: 14 })}</span>` : ''}
        <span class="jf-track-duration">${trackDuration(track.RunTimeTicks)}</span>
    </div>`;
}

// The album page's track list: disc headings only when the album has more than one disc
function albumTrackList(album, tracks) {
    const groups = trackDiscGroups(tracks);
    const albumArtist = album.AlbumArtist || '';
    const first = groups[0]?.tracks[0];
    // TODO(play queue): Play should hand every track id, in this order, to the player's queue once
    // modules/player.js has one; until then it plays the first track.
    const playAll = first ? `
        <button class="jf-btn-primary !py-1.5 !px-4 text-xs" data-action="play" data-id="${escapeHtml(first.Id)}" data-stream-type="Audio" data-title="${escapeHtml(first.Name || '')}">
            ${icon('Play', { size: 14, class: 'fill-current' })} ${t('common.play')}
        </button>` : '';
    return `
    <div class="mb-10" id="album-tracks">
        <div class="flex items-center gap-3 mb-4">
            <h2 class="text-xl font-bold text-white">${t('music.tracks_count', { count: tracks.length })}</h2>
            ${playAll}
        </div>
        ${groups.map(g => `
        <div class="jf-track-disc">
            ${groups.length > 1 ? `<h3 class="text-sm font-semibold text-white/60 mt-4 mb-2">${t('music.disc', { number: g.disc })}</h3>` : ''}
            ${g.tracks.map(track => trackRow(track, { albumArtist })).join('')}
        </div>`).join('')}
    </div>`;
}

// --- Music library tabs ---

// Jellyfin Web's music library tabs, in its order. Albums is the library's own grid (library.js);
// the others are drawn here. A genre opens its albums ('genre', at library/<id>/genres/<genre id>).
const MUSIC_TABS = [
    ['albums', 'music.tab_albums'],
    ['albumartists', 'music.tab_album_artists'],
    ['artists', 'music.tab_artists'],
    ['songs', 'music.tab_songs'],
    ['genres', 'music.tab_genres'],
];
const MUSIC_PAGE_SIZE = 100;
// The tab last shown per library, which opening the library again shows, as in Jellyfin Web
const musicTabMemory = new Map();
// Drops the pages of a tab that was left meanwhile
let musicGen = 0;

// The tab a library route shows: the one it names, else the one last shown, else Albums
function musicTab(libId, tab, sub) {
    if (tab === 'genres' && sub) return 'genre';
    if (MUSIC_TABS.some(([key]) => key === tab)) return tab;
    return musicTabMemory.get(libId) || 'albums';
}

function rememberMusicTab(libId, tab) {
    musicTabMemory.set(libId, tab === 'genre' ? 'genres' : tab);
}

function musicTabBar(libId, active) {
    if (active === 'genre') active = 'genres';
    return `
    <nav class="jf-tabs" role="tablist" aria-label="${escapeHtml(t('music.tabs'))}">
        ${MUSIC_TABS.map(([key, label]) => `
        <button type="button" role="tab" class="jf-tab" aria-selected="${key === active}" data-action="navigate" data-page="library" data-id="${escapeHtml(`${libId}/${key}`)}">${t(label)}</button>`).join('')}
    </nav>`;
}

// The request for one page of a tab: the artist lists and genres have their own endpoints, songs
// and a genre's albums come from /Items
function musicTabRequest(libId, tab, start, genreId) {
    const paging = { StartIndex: start, Limit: MUSIC_PAGE_SIZE };
    switch (tab) {
    case 'albumartists':
        return { path: '/Artists/AlbumArtists', query: { ParentId: libId, ...paging } };
    case 'artists':
        return { path: '/Artists', query: { ParentId: libId, ...paging } };
    case 'genres':
        return { path: '/MusicGenres', query: { ParentId: libId, SortBy: 'SortName', SortOrder: 'Ascending', ...paging } };
    case 'songs':
        return {
            path: '/Items',
            query: { ParentId: libId, IncludeItemTypes: 'Audio', Recursive: true, SortBy: 'SortName', SortOrder: 'Ascending', ...paging },
        };
    case 'genre':
        return {
            path: '/Items',
            query: {
                ParentId: libId, IncludeItemTypes: 'MusicAlbum', Recursive: true, GenreIds: genreId,
                SortBy: 'SortName', SortOrder: 'Ascending', Fields: 'PrimaryImageAspectRatio', ...paging,
            },
        };
    default:
        return null;
    }
}

// Adds one page of a tab: songs as track rows, genres as tiles that open their albums, artists and
// albums as cards
function appendMusicItems(container, libId, tab, items) {
    if (tab === 'songs') {
        container.insertAdjacentHTML('beforeend', items.map(it => trackRow(it, { showAlbum: true })).join(''));
    } else if (tab === 'genres') {
        container.insertAdjacentHTML('beforeend', items.map(g => `
        <div class="jf-card p-5 min-h-[96px] flex items-end bg-gradient-to-br from-amber-600/30 to-orange-500/5" data-action="navigate" data-page="library" data-id="${escapeHtml(`${libId}/genres/${g.Id}`)}">
            <p class="font-bold text-white truncate">${escapeHtml(g.Name || '')}</p>
        </div>`).join(''));
    } else {
        const frag = document.createDocumentFragment();
        for (const it of items) {
            const card = createCardElement(it);
            if (card) frag.appendChild(card);
        }
        container.appendChild(frag);
    }
}

// Draws a music library tab other than Albums, a page at a time as the list scrolls
async function renderMusicTab(libId, libName, tab, genreId) {
    const session = API.token;
    const gen = ++musicGen;
    rememberMusicTab(libId, tab);
    destroyInfiniteScroll();
    const page = $('page-library');
    page.classList.remove('jf-with-rail');
    const listClass = tab === 'songs'
        ? 'flex flex-col'
        : 'grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4';
    page.innerHTML = `
    <div class="jf-page">
        <div class="mb-4 pb-4 border-b border-white/10">
            <div class="flex items-center gap-2">
                <span class="inline-flex">${icon('Music', { size: 28 })}</span>
                <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
            </div>
            <p id="music-count" class="text-xs text-white/40 mt-1">${t('common.loading')}</p>
        </div>
        ${musicTabBar(libId, tab)}
        ${tab === 'genre' ? '<h2 id="music-genre" class="text-xl font-bold text-white mb-4"></h2>' : ''}
        <div id="music-items" class="${listClass}"></div>
        <div id="music-more"></div>
    </div>`;
    if (tab === 'genre') {
        API.cachedGet(`/MusicGenres?ParentId=${encodeURIComponent(libId)}`).then(data => {
            const genre = (data.Items || []).find(g => g.Id === genreId);
            if (genre && gen === musicGen && $('music-genre')) $('music-genre').textContent = genre.Name;
        }).catch(() => {});
    }

    let start = 0;
    // Loads the next page; answers the total, or null once the tab was left
    const loadMore = async () => {
        const { path, query } = musicTabRequest(libId, tab, start, genreId);
        const data = await API.cachedGet(`${path}?${new URLSearchParams(query)}`);
        if (gen !== musicGen || API.token !== session || !$('music-items')) return null;
        const items = data.Items || [];
        appendMusicItems($('music-items'), libId, tab, items);
        start += items.length;
        const total = data.TotalRecordCount ?? start;
        if (start >= total || !items.length) {
            destroyInfiniteScroll();
            $('music-more')?.replaceChildren();
        }
        return total;
    };
    try {
        const total = await loadMore();
        if (total == null) return;
        $('music-count').textContent = t('library.item_count', { count: total });
        if (!total) {
            $('music-items').outerHTML = `<div class="text-center py-20 text-white/40">${t('library.empty')}</div>`;
        } else if (start < total) {
            addInfiniteScrollSentinel($('music-more'));
            setupInfiniteScroll(() => loadMore().catch(e => toast(t('common.error_message', { message: e.message }), 'error')));
        }
    } catch (e) {
        if (gen !== musicGen || API.token !== session) return;
        $('music-items').outerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

export {
    trackDuration, trackDiscGroups, trackRow, albumTrackList, MUSIC_TABS, musicTab, rememberMusicTab, musicTabBar,
    musicTabRequest, renderMusicTab,
};
