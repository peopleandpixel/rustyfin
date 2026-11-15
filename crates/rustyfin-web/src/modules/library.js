// The library grid: paging, infinite scroll, the letter rail and the poster loader
import { $, initLazyImages, icon, toast, escapeHtml, scanRing } from './util.js';
import { createCardElement } from './cards.js';
let currentLibraryId = null;

// Library list state: each load gets a new generation, and responses from older generations are dropped
let libraryGen = 0;
let libraryQuery = null;
let libraryTotal = null;
let libraryObserver = null;
let libraryRequested = new Set();
let letterClicks = 0;
// The letter rail: the anchor of the last jump ({ k, first }) and the pending frame of an update
let railAnchor = null;
let railFrame = 0;
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

// --- Library Explorer Page Renderer (with pagination) ---
async function renderLibrary(libId) {
    const session = API.token;
    currentLibraryId = libId;
    const page = $('page-library');
    
    // Show skeleton grid immediately for instant visual feedback
    const lib = libraries.find(l => (l.id || l.Id) === libId);
    const libName = lib ? (lib.name || lib.Name) : t('common.library');
    const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';
    const libIcon = contentType === 'Movies' ? 'Film' : contentType === 'TvShows' ? 'Tv' : contentType === 'Music' ? 'Music' : 'Folder';
    const headerIcon = { Movies: 'Film', TvShows: 'Tv', Music: 'Music' }[lib?.content_type] || 'Folder';
    
    page.innerHTML = `
    <div class="jf-page">
        <div class="flex flex-col md:flex-row md:items-center justify-between gap-4 mb-6 pb-4 border-b border-white/10">
            <div>
                <div class="flex items-center gap-2">
                    <span class="inline-flex">${icon(libIcon, { size: 28 })}</span>
                    <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
                </div>
                <p class="text-xs text-white/40 mt-1">${t('common.loading')}</p>
            </div>
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
    renderLetterRail(libId);

    const gen = startLibraryLoad(libId);
    try {
        // Fetch first page with pagination + total count, and the genres for the filter
        const [{ items, total }, filterData] = await Promise.all([
            fetchLibraryPage(0),
            API.getFilters().catch(() => ({ Genres: [] }))
        ]);
        if (gen !== libraryGen || API.token !== session) return;
        const genres = filterData.Genres || filterData.genres || [];

        page.innerHTML = `
        <div class="jf-page">
            <!-- Header bar with title, count, and actions -->
            <div class="flex flex-col md:flex-row md:items-center justify-between gap-4 mb-6 pb-4 border-b border-white/10">
                <div>
                    <div class="flex items-center gap-2">
                        <span class="inline-flex">${icon(headerIcon, { size: 28 })}</span>
                        <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
                        ${scanRing(libId, libName, 'w-5 h-5')}
                    </div>
                    <p id="lib-count-label" class="text-xs text-white/40 mt-1">${t('library.item_count', { count: total })}</p>
                </div>

                <div class="flex flex-wrap items-center gap-2">
                    <!-- Sort selector, with a toggle that reverses its order -->
                    <div class="flex items-center gap-1">
                        <select id="lib-sort" class="jf-input !py-1.5 text-xs">
                            <option value="SortName,ProductionYear:Ascending">${t('library.sort_name')}</option>
                            <option value="PremiereDate,SortName,ProductionYear:Descending">${t('library.sort_release_date')}</option>
                            <option value="DateCreated,SortName,ProductionYear:Descending">${t('library.sort_date_added')}</option>
                            <option value="CommunityRating,SortName,ProductionYear:Descending">${t('library.sort_rating')}</option>
                        </select>
                        <button id="lib-sort-reverse" type="button" class="jf-btn-ghost self-stretch !py-1.5 !px-2 text-xs" aria-pressed="false" title="${escapeHtml(t('library.sort_reverse'))}" aria-label="${escapeHtml(t('library.sort_reverse'))}">
                            ${icon('ArrowDownUp', { size: 16 })}
                        </button>
                    </div>

                    <!-- Genre filter -->
                    ${genres.length > 0 ? `
                    <select id="lib-genre" class="jf-input !py-1.5 text-xs max-w-[140px]">
                        <option value="">${t('library.all_genres')}</option>
                        ${genres.map(g => `<option value="${escapeHtml(g)}">${escapeHtml(g)}</option>`).join('')}
                    </select>` : ''}

                    <!-- Scan trigger -->
                    <button class="jf-btn-ghost inline-flex items-center gap-1.5 !py-1.5 !px-3 text-xs" data-action="scan-library-page" data-id="${escapeHtml(libId)}" data-name="${escapeHtml(libName)}">
                        ${icon('RefreshCw', { size: 14 })}
                        ${t('library.scan_button')}
                    </button>
                </div>
            </div>

            <!-- Media Grid Container -->
            <div id="lib-grid-container"></div>
        </div>`;
        $('lib-sort')?.addEventListener('change', () => reloadLibrary(libId));
        $('lib-sort-reverse')?.addEventListener('click', e => {
            const btn = e.currentTarget;
            btn.setAttribute('aria-pressed', btn.getAttribute('aria-pressed') === 'true' ? 'false' : 'true');
            reloadLibrary(libId);
        });
        $('lib-genre')?.addEventListener('change', () => reloadLibrary(libId));
        fillLibraryGrid(items, total);
    } catch (e) {
        if (gen !== libraryGen || API.token !== session) return;
        page.classList.remove('jf-with-rail');
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

// The library list's sort: the select's value, with the order flipped while the reverse toggle is pressed
function librarySort() {
    const [sortBy, sortOrder] = ($('lib-sort')?.value || 'SortName,ProductionYear:Ascending').split(':');
    const reversed = $('lib-sort-reverse')?.getAttribute('aria-pressed') === 'true';
    if (!reversed) return { sortBy, sortOrder };
    return { sortBy, sortOrder: sortOrder === 'Ascending' ? 'Descending' : 'Ascending' };
}

// The /Items query for the library list; paging is added per request
function libraryListQuery(libId) {
    const lib = libraries.find(l => (l.id || l.Id) === libId);
    const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';

    let includeItemTypes = undefined;
    if (contentType === 'TvShows') includeItemTypes = 'Series';
    else if (contentType === 'Music') includeItemTypes = 'MusicAlbum';
    else if (contentType === 'Movies') includeItemTypes = 'Movie';

    const { sortBy, sortOrder } = librarySort();
    return {
        LibraryId: libId,
        IncludeItemTypes: includeItemTypes,
        SortBy: sortBy,
        SortOrder: sortOrder,
        Fields: 'PrimaryImageAspectRatio',
        Genres: $('lib-genre')?.value || undefined
    };
}

// Starts a load: a new generation, and the observer of the previous load is dropped
function startLibraryLoad(libId) {
    libraryObserver?.disconnect();
    libraryObserver = null;
    libraryQuery = libraryListQuery(libId);
    libraryTotal = null;
    libraryRequested = new Set([0]); // the first page is requested by the load itself
    railAnchor = null;
    return ++libraryGen;
}

// One page of the current load, with the total record count
async function fetchLibraryPage(startIndex) {
    const data = await API.getItems({ ...libraryQuery, StartIndex: startIndex, Limit: LIBRARY_PAGE_SIZE });
    const items = data.Items || data.items || [];
    return { items, total: data.TotalRecordCount || data.total_record_count || items.length };
}

// Fills #lib-grid-container with one child per item: cards for the loaded items, placeholders for the rest
function fillLibraryGrid(items, total) {
    const container = $('lib-grid-container');
    if (!container) return;
    libraryTotal = total;
    if (total === 0) {
        container.innerHTML = `<div class="text-center py-20 text-white/40">${t('library.no_filter_results')}</div>`;
        return;
    }
    container.innerHTML = '<div id="lib-grid" class="jf-lib-grid"></div>';
    const grid = container.firstElementChild;

    const template = document.createElement('div');
    template.innerHTML = '<div class="jf-card"><div class="jf-poster"></div><div class="p-3"><p class="text-xs sm:text-sm">&nbsp;</p><div class="text-[11px] mt-0.5">&nbsp;</div></div></div>';
    const placeholder = template.firstElementChild;
    const frag = document.createDocumentFragment();
    for (let i = 0; i < total; i++) {
        if (i < items.length) {
            const card = createCardElement(items[i]);
            card.dataset.letter = railLetter(items[i]);
            frag.appendChild(card);
        } else {
            const ph = placeholder.cloneNode(true);
            ph.dataset.i = i;
            frag.appendChild(ph);
        }
    }
    grid.appendChild(frag);
    initLazyImages(grid);

    // A placeholder that comes near the viewport requests its page, once per load
    const gen = libraryGen;
    libraryObserver = new IntersectionObserver(entries => {
        if (gen !== libraryGen) return; // a newer load has replaced this observer
        for (const entry of entries) {
            if (!entry.isIntersecting) continue;
            const page = Math.floor(Number(entry.target.dataset.i) / LIBRARY_PAGE_SIZE);
            if (libraryRequested.has(page)) continue;
            libraryRequested.add(page);
            loadLibraryPage(gen, page);
        }
    }, { rootMargin: '200% 0px' });
    grid.querySelectorAll('[data-i]').forEach(ph => libraryObserver.observe(ph));
    scheduleRailUpdate();
}

// Puts the cards of one page in place of its placeholders
async function loadLibraryPage(gen, page) {
    try {
        const { items } = await fetchLibraryPage(page * LIBRARY_PAGE_SIZE);
        if (gen !== libraryGen) return;
        const grid = $('lib-grid');
        items.forEach((item, j) => {
            const placeholder = grid?.children[page * LIBRARY_PAGE_SIZE + j];
            if (!placeholder) return;
            const card = createCardElement(item);
            card.dataset.letter = railLetter(item);
            placeholder.replaceWith(card);
            libraryObserver.unobserve(placeholder);
            initLazyImages(card);
        });
        scheduleRailUpdate();
    } catch (e) {
        if (gen !== libraryGen) return;
        libraryRequested.delete(page);
        toast(t('library.load_failed', { message: e.message }), 'error');
    }
}

// A sort or genre change: a new load for the new query, shown from the top of the list
async function reloadLibrary(libId) {
    renderLetterRail(libId);

    window.scrollTo({ top: 0, behavior: 'instant' });
    const gen = startLibraryLoad(libId);
    const container = $('lib-grid-container');
    if (container) {
        container.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    }

    try {
        const { items, total } = await fetchLibraryPage(0);
        if (gen !== libraryGen) return;
        fillLibraryGrid(items, total);
        const countLabel = $('lib-count-label');
        if (countLabel) countLabel.textContent = t('library.item_count', { count: total });
    } catch (e) {
        if (gen !== libraryGen) return;
        toast(t('library.filter_failed', { message: e.message }), 'error');
    }
}

// Scrolls the list to the first item of a letter. Nothing is filtered.
async function jumpToLetter(libId, letter) {
    const click = ++letterClicks;
    const gen = libraryGen;
    if (libraryTotal === null || !$('lib-grid')) return;
    const query = libraryQuery;
    const n = libraryTotal;
    const desc = query.SortOrder === 'Descending';
    // The rail's first entry goes back to the top of the page, header and controls included; no request
    if (letter === (desc ? 'Z' : '#')) {
        railAnchor = null;
        window.scrollTo({ top: 0, behavior: 'instant' });
        scheduleRailUpdate();
        return;
    }
    const stale = () => click !== letterClicks || gen !== libraryGen;
    // Count of items whose name sorts at or after the bound; Limit 0 fetches no items
    const countFrom = async (bound) => {
        const data = await API.getItems({ ...query, NameStartsWithOrGreater: bound, Limit: 0 });
        return data.TotalRecordCount || data.total_record_count || 0;
    };
    try {
        let k = 0;
        if (letter !== '#') {
            const b = letter.toLowerCase();
            k = desc
                ? await countFrom(String.fromCharCode(b.charCodeAt(0) + 1))
                : n - await countFrom(b);
        } else if (desc) {
            k = await countFrom('a');
        }
        if (stale()) return;
        const grid = $('lib-grid');
        if (!grid) return;
        if (k < n) {
            grid.children[k].scrollIntoView({ block: 'start', behavior: 'instant' });
            // The clicked section stays lit while its row is the top row (that row often starts with the previous letter)
            railAnchor = { k, first: firstVisibleIndex(grid) };
            setRailLetter(letter);
            updateLetterRail();
        } else {
            railAnchor = null;
            window.scrollTo({ top: document.scrollingElement.scrollHeight, behavior: 'instant' });
            scheduleRailUpdate();
        }
    } catch (e) {
        if (stale()) return;
        toast(t('library.load_failed', { message: e.message }), 'error');
    }
}

// The letter rail (#lib-letter-rail in index.html) shows for the name sorts, its letters reversed for a descending order
function renderLetterRail(libId) {
    const { sortBy, sortOrder } = librarySort();
    $('page-library').classList.toggle('jf-with-rail', sortBy.startsWith('SortName'));
    const letters = ['#', ...'ABCDEFGHIJKLMNOPQRSTUVWXYZ'];
    if (sortOrder === 'Descending') letters.reverse();
    $('lib-letter-rail').innerHTML = letters.map(l =>
        `<button type="button" data-action="jump-letter" data-letter="${l}" data-id="${escapeHtml(libId)}">${l}</button>`).join('');
    railAnchor = null;
}

// An item's rail section: the server sorts by the lowercased name, so its first character decides
function railLetter(item) {
    const c = (item.SortName || item.Name || '').toLowerCase().charAt(0);
    if (c < 'a') return '#';
    if (c > 'z') return 'Z';
    return c.toUpperCase();
}

// Index of the first card whose bottom edge is below the header; cards are in row order, so a binary search works
function firstVisibleIndex(grid) {
    const top = $('main-navbar').getBoundingClientRect().bottom;
    const kids = grid.children;
    let lo = 0, hi = kids.length;
    while (lo < hi) {
        const mid = (lo + hi) >> 1;
        if (kids[mid].getBoundingClientRect().bottom > top) hi = mid;
        else lo = mid + 1;
    }
    return lo;
}

function setRailLetter(letter) {
    for (const b of $('lib-letter-rail').children) {
        if (b.dataset.letter === letter) b.setAttribute('aria-current', 'true');
        else b.removeAttribute('aria-current');
    }
}

// Lights the section of the first item at the top of the view, or after a jump the clicked one (see jumpToLetter)
function updateLetterRail() {
    const page = $('page-library');
    const grid = $('lib-grid');
    if (!grid || page.classList.contains('hidden') || !page.classList.contains('jf-with-rail')) return;
    let i = firstVisibleIndex(grid);
    if (railAnchor && i === railAnchor.first) i = railAnchor.k;
    else railAnchor = null;
    const letter = grid.children[i]?.dataset.letter;
    if (letter) setRailLetter(letter); // a placeholder keeps the highlight until its page is in
}

// At most one update per frame
function scheduleRailUpdate() {
    if (railFrame) return;
    railFrame = requestAnimationFrame(() => {
        railFrame = 0;
        updateLetterRail();
    });
}
window.addEventListener('scroll', scheduleRailUpdate, { passive: true });
window.addEventListener('resize', scheduleRailUpdate);

export { currentLibraryId, LIBRARY_PAGE_SIZE, setupInfiniteScroll, destroyInfiniteScroll, renderLibrary, jumpToLetter };
