// The search page
import { $, escapeHtml } from './util.js';
import { createCardElement } from './cards.js';
import { navigate } from './router.js';

// --- Search Page Renderer ---
async function renderSearch(term) {
    term = decodeURIComponent(term || '').trim();
    if ($('search-input')) $('search-input').value = term;
    const page = $('page-search');
    const showNoResults = () => {
        page.innerHTML = `<div class="text-center py-24 text-white/40 jf-page"><p class="text-lg">${t('search.no_results', { term: escapeHtml(term) })}</p></div>`;
    };
    // A blank term never reaches the server: /Search/Hints answers 400 to it.
    if (!term) {
        showNoResults();
        return;
    }
    page.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';

    try {
        const data = await API.search(term, 50);
        const hints = data.SearchHints || data.search_hints || [];
        if (!hints.length) {
            showNoResults();
            return;
        }
        const items = hints.map(h => ({
            Id: h.Id || h.id || h.ItemId,
            Name: h.Name || h.name,
            Type: h.Type || h.type,
            ProductionYear: h.ProductionYear || h.production_year,
            ...(h.PrimaryImageTag ? { ImageTags: { Primary: h.PrimaryImageTag } } : {})
        }));

        page.innerHTML = `
        <div class="jf-page">
            <h1 class="text-2xl font-bold mb-6 text-white">${t('search.results_for', { term: escapeHtml(term), count: items.length })}</h1>
            <div id="search-grid" class="jf-card-grid grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4"></div>
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

export { renderSearch, doSearch };
