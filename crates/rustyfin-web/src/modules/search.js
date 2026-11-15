// The search page: results as you type, grouped by type as Jellyfin Web groups them
import { $, escapeHtml } from './util.js';
import { createCardElement } from './cards.js';
import { navigate, currentPage } from './router.js';

// Jellyfin Web's search sections, in its order, with the item types each one asks for
const SEARCH_SECTIONS = [
    ['movies', 'Movie'],
    ['shows', 'Series'],
    ['episodes', 'Episode'],
    ['people', 'Person'],
    ['playlists', 'Playlist'],
    ['artists', 'MusicArtist'],
    ['albums', 'MusicAlbum'],
    ['songs', 'Audio'],
    ['videos', 'Video,MusicVideo'],
    ['photos', 'Photo'],
    ['audiobooks', 'AudioBook'],
    ['books', 'Book'],
    ['collections', 'BoxSet'],
];
const SEARCH_SECTION_LIMIT = 24;
// Jellyfin Web waits this long after the last key before it searches
const SEARCH_DEBOUNCE_MS = 500;

function searchSectionUrl(term, types) {
    return `/Search/Hints?SearchTerm=${encodeURIComponent(term)}&IncludeItemTypes=${types}&Limit=${SEARCH_SECTION_LIMIT}`;
}

// A search hint as the item the cards draw
function hintItem(h) {
    return {
        Id: h.Id || h.ItemId,
        Name: h.Name,
        Type: h.Type,
        ProductionYear: h.ProductionYear,
        ...(h.PrimaryImageTag ? { ImageTags: { Primary: h.PrimaryImageTag } } : {}),
    };
}

// One section's row of cards; a person's card opens the person page
function searchSection(key, items) {
    const section = document.createElement('section');
    section.className = 'mb-10';
    section.dataset.searchSection = key;
    section.innerHTML = `
        <div class="flex items-center gap-2 mb-4">
            <span class="w-2.5 h-2.5 rounded-full bg-[#00a4dc]"></span>
            <h2 class="text-lg lg:text-xl font-bold tracking-tight text-white">${escapeHtml(t(`search.section_${key}`))}</h2>
            <span class="text-xs text-white/30 font-medium ml-1">(${items.length})</span>
        </div>
        <div class="jf-scroll-row pb-2"></div>`;
    const row = section.lastElementChild;
    for (const item of items) {
        const card = createCardElement(item, 'w-36 sm:w-44 md:w-48');
        if (!card) continue;
        if (item.Type === 'Person') card.dataset.page = 'person';
        row.appendChild(card);
    }
    return section;
}

// Answers of a search that was overtaken meanwhile are dropped
let searchSeq = 0;

// --- Search Page Renderer ---
async function renderSearch(term) {
    const seq = ++searchSeq;
    term = decodeURIComponent(term || '').trim();
    if ($('search-input') && document.activeElement !== $('search-input')) $('search-input').value = term;
    const page = $('page-search');
    const showNoResults = () => {
        page.innerHTML = `<div class="text-center py-24 text-white/40 jf-page"><p class="text-lg">${t('search.no_results', { term: escapeHtml(term) })}</p></div>`;
    };
    // A blank term never reaches the server: /Search/Hints answers 400 to it.
    if (!term) {
        showNoResults();
        return;
    }
    if (!page.querySelector('[data-search-section]')) {
        page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    }

    try {
        // One request per section, as Jellyfin Web makes, so no type crowds out the others
        const sections = await Promise.all(SEARCH_SECTIONS.map(([key, types]) => API.get(searchSectionUrl(term, types))
            .then(data => ({ key, items: (data.SearchHints || []).map(hintItem) }))));
        if (seq !== searchSeq) return;
        const found = sections.filter(s => s.items.length);
        const count = found.reduce((n, s) => n + s.items.length, 0);
        if (!count) {
            showNoResults();
            return;
        }
        page.innerHTML = `
        <div class="jf-page">
            <h1 class="text-2xl font-bold mb-6 text-white">${t('search.results_for', { term: escapeHtml(term), count })}</h1>
            <div id="search-sections"></div>
        </div>`;
        $('search-sections').append(...found.map(s => searchSection(s.key, s.items)));
    } catch (e) {
        if (seq !== searchSeq) return;
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

let searchTimeout = null;

// Shows the results for a term; on the search page already, in place, without a history entry per term
function showSearch(value) {
    const term = (value || '').trim();
    if (!term) return;
    const encoded = encodeURIComponent(term);
    if (currentPage === 'search') {
        history.replaceState(null, '', `#search/${encoded}`);
        renderSearch(encoded);
    } else {
        navigate('search', encoded);
    }
}

// Typing searches once the keys pause; Enter searches at once
function searchAsYouType(value) {
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(() => showSearch(value), SEARCH_DEBOUNCE_MS);
}

function doSearch(term) {
    clearTimeout(searchTimeout);
    showSearch(term);
}

$('search-input')?.addEventListener('input', e => searchAsYouType(e.target.value));

export { renderSearch, doSearch };
