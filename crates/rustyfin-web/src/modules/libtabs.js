// A movie or TV library's Suggestions, Upcoming and Networks tabs, as Jellyfin Web's
// (moviesrecommended.js, tvrecommended.js, tvupcoming.js, tvstudios.js). The list tabs (Movies,
// Shows, Favorites, Episodes, a network's shows) are library.js's grid.
import { $, icon, escapeHtml } from './util.js';
import { sectionRow } from './cards.js';

// Drops the answers of a tab that was left meanwhile
let tabGen = 0;

// Jellyfin Web's recommendation rows: how many, and titles per row
const RECOMMENDATION_ROWS = 6;
const RECOMMENDATION_ITEMS = 8;
const RECOMMENDATION_TITLES = {
    SimilarToRecentlyPlayed: 'library.because_you_watched',
    SimilarToLikedItem: 'library.because_you_like',
    HasDirectorFromRecentlyPlayed: 'library.directed_by',
    HasLikedDirector: 'library.directed_by',
    HasActorFromRecentlyPlayed: 'library.starring',
    HasLikedActor: 'library.starring',
};

// The Suggestions tab's rows in Jellyfin Web's order: [title key, url, card options]. A movie
// library adds its recommendation rows after these (recommendationsUrl).
function suggestionRequests(libId, contentType) {
    const tv = contentType === 'TvShows';
    const type = tv ? 'Episode' : 'Movie';
    const q = extra => new URLSearchParams({ ParentId: libId, ...extra });
    const fields = { Fields: 'PrimaryImageAspectRatio', EnableTotalRecordCount: 'false' };
    return [
        ['home.continue_watching', `/UserItems/Resume?${q({ IncludeItemTypes: type, Recursive: 'true', Limit: '12', ...fields })}`, { backdrop: true }],
        [tv ? 'library.latest_episodes' : 'library.latest_movies', `/Items/Latest?${q({ IncludeItemTypes: type, Limit: '18', Fields: 'PrimaryImageAspectRatio' })}`, { backdrop: tv }],
        ...(tv ? [['home.next_up', `/Shows/NextUp?${q({ Limit: '24', ...fields })}`, { backdrop: true }]] : []),
    ];
}

function recommendationsUrl(libId) {
    const q = new URLSearchParams({
        ParentId: libId, CategoryLimit: String(RECOMMENDATION_ROWS), ItemLimit: String(RECOMMENDATION_ITEMS), Fields: 'PrimaryImageAspectRatio',
    });
    return `/Movies/Recommendations?${q}`;
}

// A list answer's items: a QueryResult's Items, or the array itself (/Items/Latest)
function itemsOf(data) {
    return Array.isArray(data) ? data : data?.Items || [];
}

// A tab's page: the library's title, a status line and the tab bar over the tab's own content
function tabPage(libName, contentType, tabBar, contentId) {
    const page = $('page-library');
    page.classList.remove('jf-with-rail');
    page.innerHTML = `
    <div class="jf-page">
        <div class="mb-4 pb-4 border-b border-white/10">
            <div class="flex items-center gap-2">
                <span class="inline-flex">${icon(contentType === 'TvShows' ? 'Tv' : 'Film', { size: 28 })}</span>
                <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(libName)}</h1>
            </div>
            <p id="libtab-status" class="text-xs text-white/40 mt-1">${t('common.loading')}</p>
        </div>
        ${tabBar}
        <div id="${contentId}"></div>
    </div>`;
    const gen = ++tabGen;
    const session = API.token;
    return () => gen !== tabGen || API.token !== session || !$(contentId);
}

// Fills a tab's rows, or says it has nothing (emptyKey) or why it failed
function showRows(contentId, rows, emptyKey) {
    $('libtab-status').textContent = '';
    $(contentId).innerHTML = rows.join('') || `<div class="text-center py-20 text-white/40">${t(emptyKey)}</div>`;
}

function showError(contentId, e) {
    $('libtab-status').textContent = '';
    $(contentId).innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
}

// Suggestions: Continue Watching, the latest titles, Next Up for TV, and for movies a row per
// recommendation ("Because you watched …"). A row that fails or is empty is left out.
async function renderSuggestions(libId, libName, contentType, tabBar) {
    const stale = tabPage(libName, contentType, tabBar, 'suggestion-rows');
    const requests = suggestionRequests(libId, contentType);
    const [settled, recommendations] = await Promise.all([
        Promise.allSettled(requests.map(([, url]) => API.get(url))),
        contentType === 'Movies' ? API.get(recommendationsUrl(libId)).catch(() => []) : [],
    ]);
    if (stale()) return;
    if (settled.every(r => r.status === 'rejected')) {
        showError('suggestion-rows', settled[0].reason);
        return;
    }
    const rows = requests.map(([title, , options], i) => settled[i].status === 'fulfilled'
        ? sectionRow(t(title), itemsOf(settled[i].value), null, options) : '');
    for (const row of Array.isArray(recommendations) ? recommendations : []) {
        const key = RECOMMENDATION_TITLES[row.RecommendationType];
        if (key) rows.push(sectionRow(t(key, { name: row.BaselineItemName || '' }), row.Items || []));
    }
    showRows('suggestion-rows', rows, 'library.suggestions_empty');
}

// The day an episode airs, as its row's title: Today, Tomorrow, or the date
function airDayTitle(date, now = new Date()) {
    const day = d => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
    const days = Math.round((day(date) - day(now)) / 86400000);
    if (days === 0) return t('library.today');
    if (days === 1) return t('library.tomorrow');
    return date.toLocaleDateString(undefined, { weekday: 'long', month: 'long', day: 'numeric' });
}

// The upcoming episodes in a row per day they air, in order; one with no date goes last
function upcomingGroups(items, now = new Date()) {
    const groups = new Map();
    for (const item of items) {
        const date = item.PremiereDate ? new Date(item.PremiereDate) : null;
        const title = date && !Number.isNaN(date.getTime()) ? airDayTitle(date, now) : t('library.date_unknown');
        if (!groups.has(title)) groups.set(title, []);
        groups.get(title).push(item);
    }
    return [...groups];
}

// Upcoming: the library's episodes still to air, a row per day
async function renderUpcoming(libId, libName, contentType, tabBar) {
    const stale = tabPage(libName, contentType, tabBar, 'upcoming-rows');
    const q = new URLSearchParams({ ParentId: libId, Limit: '48', Fields: 'AirTime,PrimaryImageAspectRatio', EnableTotalRecordCount: 'false' });
    let items;
    try {
        items = itemsOf(await API.get(`/Shows/Upcoming?${q}`));
    } catch (e) {
        if (!stale()) showError('upcoming-rows', e);
        return;
    }
    if (stale()) return;
    showRows('upcoming-rows', upcomingGroups(items).map(([title, list]) => sectionRow(title, list, null, { backdrop: true })),
        'library.upcoming_empty');
}

// The library's networks (studios of its shows), as Jellyfin Web asks for them
function studiosUrl(libId) {
    const q = new URLSearchParams({
        ParentId: libId, IncludeItemTypes: 'Series', Recursive: 'true', SortBy: 'SortName', SortOrder: 'Ascending',
        Fields: 'PrimaryImageAspectRatio', EnableTotalRecordCount: 'false',
    });
    return `/Studios?${q}`;
}

// A network's name from its id, for the title of its list; '' when not found
async function studioNameOf(libId, studioId) {
    try {
        return itemsOf(await API.cachedGet(studiosUrl(libId))).find(s => s.Id === studioId)?.Name || '';
    } catch {
        return '';
    }
}

// A network's card: its thumb or logo when it has one, else its name; it opens the list of the
// library's shows from that network (library/<id>/studios/<studio id>)
function studioCard(libId, studio) {
    const tags = studio.ImageTags || {};
    const type = tags.Thumb ? 'Thumb' : tags.Primary ? 'Primary' : '';
    const picture = type
        ? `<img class="w-full h-full object-cover" loading="lazy" alt="" src="${escapeHtml(`/Items/${studio.Id}/Images/${type}?fillWidth=480&quality=90&tag=${tags[type]}`)}">`
        : `<span class="flex flex-col items-center justify-center gap-2 h-full text-white/50">${icon('Tv', { size: 32 })}</span>`;
    return `
    <button type="button" class="jf-card group text-left" data-action="navigate" data-page="library" data-id="${escapeHtml(`${libId}/studios/${studio.Id}`)}">
        <div class="jf-poster-backdrop">${picture}</div>
        <div class="p-2 text-sm font-semibold text-white truncate">${escapeHtml(studio.Name || '')}</div>
    </button>`;
}

// Networks: a card per network of the library's shows
async function renderStudios(libId, libName, contentType, tabBar) {
    const stale = tabPage(libName, contentType, tabBar, 'studio-grid');
    let studios;
    try {
        studios = itemsOf(await API.cachedGet(studiosUrl(libId)));
    } catch (e) {
        if (!stale()) showError('studio-grid', e);
        return;
    }
    if (stale()) return;
    $('libtab-status').textContent = t('library.studio_count', { count: studios.length });
    $('studio-grid').innerHTML = studios.length
        ? `<div class="grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 xl:grid-cols-5 gap-4">${studios.map(s => studioCard(libId, s)).join('')}</div>`
        : `<div class="text-center py-20 text-white/40">${t('library.empty')}</div>`;
}

export {
    suggestionRequests, recommendationsUrl, renderSuggestions, airDayTitle, upcomingGroups, renderUpcoming, studiosUrl,
    studioNameOf, renderStudios,
};
