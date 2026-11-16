// A movie or TV library's Genres tab: a row per genre with some of its titles, as Jellyfin Web's
// genres view. A row's header opens the library's list of that genre (library.js).
import { $, icon, escapeHtml } from './util.js';
import { sectionRow } from './cards.js';

// Jellyfin Web's genre rows: up to 25 titles each, in a random order, a few rows asked at a time,
// each when it comes near the screen
const GENRE_ROW_LIMIT = 25;
const GENRE_ROWS_AT_ONCE = 4;
const GENRE_ROW_MARGIN = '600px';
// Drops the rows of a tab that was left meanwhile
let genreGen = 0;
// Watches the open tab's rows that are not asked for yet
let genreObserver = null;

function genreItemType(contentType) {
    return contentType === 'TvShows' ? 'Series' : 'Movie';
}

// The library's genres, as Jellyfin Web asks for them
function genresUrl(libId, contentType) {
    const q = new URLSearchParams({
        ParentId: libId, IncludeItemTypes: genreItemType(contentType), Recursive: 'true', SortBy: 'SortName', SortOrder: 'Ascending',
    });
    return `/Genres?${q}`;
}

// One genre's row: its titles in the library, in a random order
function genreRowUrl(libId, contentType, genreId) {
    const q = new URLSearchParams({
        ParentId: libId, IncludeItemTypes: genreItemType(contentType), Recursive: 'true', GenreIds: genreId,
        SortBy: 'Random', SortOrder: 'Ascending', Fields: 'PrimaryImageAspectRatio', ImageTypeLimit: '1',
        EnableImageTypes: 'Primary', Limit: String(GENRE_ROW_LIMIT), EnableTotalRecordCount: 'false',
    });
    return `/Items?${q}`;
}

// A genre's name from its id, for the title of its list; '' when not found
async function genreNameOf(libId, contentType, genreId) {
    try {
        const data = await API.cachedGet(genresUrl(libId, contentType));
        return (data.Items || []).find(g => g.Id === genreId)?.Name || '';
    } catch {
        return '';
    }
}

async function renderGenreRows(libId, libName, contentType, tabBar) {
    const session = API.token;
    const gen = ++genreGen;
    genreObserver?.disconnect();
    genreObserver = null;
    const page = $('page-library');
    const headerIcon = contentType === 'TvShows' ? 'Tv' : 'Film';
    page.classList.remove('jf-with-rail');
    page.innerHTML = `
    <div class="jf-page">
        <div class="mb-4 pb-4 border-b border-white/10">
            <div class="flex items-center gap-2">
                <span class="inline-flex">${icon(headerIcon, { size: 28 })}</span>
                <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
            </div>
            <p id="genre-count" class="text-xs text-white/40 mt-1">${t('common.loading')}</p>
        </div>
        ${tabBar}
        <div id="genre-rows"></div>
    </div>`;
    const stale = () => gen !== genreGen || API.token !== session || !$('genre-rows');
    let genres;
    try {
        genres = (await API.cachedGet(genresUrl(libId, contentType))).Items || [];
    } catch (e) {
        if (stale()) return;
        $('genre-count').textContent = '';
        $('genre-rows').innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
        return;
    }
    if (stale()) return;
    $('genre-count').textContent = t('library.genre_count', { count: genres.length });
    if (!genres.length) {
        $('genre-rows').innerHTML = `<div class="text-center py-20 text-white/40">${t('library.empty')}</div>`;
        return;
    }
    // A slot per genre in order, with a row's height reserved, filled when it nears the screen; a
    // genre with no titles ends up empty. The random rows are asked fresh on every visit.
    $('genre-rows').innerHTML = genres.map((_, i) => `<div data-genre-row="${i}" class="jf-genre-slot"></div>`).join('');
    const queue = [];
    let running = 0;
    const loadRow = async i => {
        const genre = genres[i];
        let items = [];
        try {
            items = (await API.get(genreRowUrl(libId, contentType, genre.Id))).Items || [];
        } catch (e) {
            console.warn('Genre row failed', genre.Name, e);
        }
        if (stale()) return;
        const slot = document.querySelector(`#genre-rows [data-genre-row="${i}"]`);
        if (slot) {
            slot.classList.remove('jf-genre-slot');
            slot.innerHTML = sectionRow(genre.Name || '', items, { page: 'library', data: `${libId}/genres/${genre.Id}` });
        }
    };
    const pump = async () => {
        while (queue.length && running < GENRE_ROWS_AT_ONCE && !stale()) {
            running++;
            loadRow(queue.shift()).finally(() => {
                running--;
                pump();
            });
        }
    };
    const slots = [...document.querySelectorAll('#genre-rows [data-genre-row]')];
    genreObserver = new IntersectionObserver(entries => {
        if (stale()) {
            genreObserver?.disconnect();
            return;
        }
        for (const entry of entries) {
            if (!entry.isIntersecting) continue;
            genreObserver.unobserve(entry.target);
            queue.push(Number(entry.target.dataset.genreRow));
        }
        queue.sort((a, b) => a - b);
        pump();
    }, { rootMargin: `${GENRE_ROW_MARGIN} 0px` });
    for (const slot of slots) genreObserver.observe(slot);
}

export { genresUrl, genreRowUrl, genreNameOf, renderGenreRows };
