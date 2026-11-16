// Item and person detail pages, favourite and played toggles
import { $, icon, toast, escapeHtml, formatRuntime } from './util.js';
import { backdropUrl, createCardElement, sectionRow } from './cards.js';
import { resumePositionTicks } from './player.js';
import { navigate } from './router.js';
import { albumTrackList } from './music.js';
import { containerSectionHtml, renderContainerItems } from './collections.js';
import { playThemeSong } from './theme-media.js';
let currentDetailId = null;
let activeItem = null;
// The detail page's version, audio and subtitle choice: { itemId, sources, mediaSourceId, audioIndex,
// subtitleIndex }. A null index is untouched, so the player picks its own default for it.
let detailChoice = null;

// --- Detail page parts, in the order and with the rules of Jellyfin Web's item details ---

// Jellyfin's "Ends at": only for a video with a runtime that is not a scheduled recording, counting from
// the resume point
function endsAt(item, now = Date.now()) {
    if (item.MediaType !== 'Video' || !item.RunTimeTicks || item.StartDate) return null;
    const left = item.RunTimeTicks - (item.UserData?.PlaybackPositionTicks || 0);
    return new Date(now + left / 1e4);
}

// The critic rating badge; 60 and up counts as fresh, as in Jellyfin Web
function criticRatingHtml(item) {
    const rating = item.CriticRating;
    if (rating == null) return '';
    const fresh = rating >= 60;
    return `<span class="jf-badge jf-critic-rating text-[11px] font-bold ${fresh ? 'text-emerald-300' : 'text-rose-300'}" data-fresh="${fresh}" title="${escapeHtml(t(fresh ? 'detail.critic_fresh' : 'detail.critic_rotten'))}">${icon(fresh ? 'ThumbsUp' : 'ThumbsDown', { size: 10 })} ${Math.round(rating)}%</span>`;
}

// Director, Writer and Studio lines; Jellyfin Web uses the plural label for more than one name
function creditLinesHtml(item) {
    const people = item.People || [];
    const personLink = p => p.Id ? `<a href="#person/${escapeHtml(p.Id)}" class="text-white hover:text-[#38bdf8] hover:underline">${escapeHtml(p.Name || '')}</a>` : escapeHtml(p.Name || '');
    const lines = [
        ['detail.director', 'detail.directors', people.filter(p => p.Type === 'Director').map(personLink)],
        ['detail.writer', 'detail.writers', people.filter(p => p.Type === 'Writer').map(personLink)],
        ['detail.studio', 'detail.studios', (item.Studios || []).map(s => escapeHtml(s.Name || s))],
    ].filter(([, , names]) => names.length > 0);
    if (lines.length === 0) return '';
    return `<dl class="jf-credit-lines grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 mb-8 max-w-4xl text-sm">${lines.map(([one, many, names]) =>
        `<dt class="text-white/50">${t(names.length > 1 ? many : one)}</dt><dd class="text-white/85">${names.join(', ')}</dd>`).join('')}</dl>`;
}

// Where a provider id leads when the server sent no ExternalUrls for it. TMDb and TheTVDB ids name a page
// only for movies and series, as Jellyfin's external-id providers have it; null is no link.
const PROVIDER_URLS = {
    Imdb: (id) => `https://www.imdb.com/title/${id}`,
    Tmdb: (id, type) => ({ Movie: `https://www.themoviedb.org/movie/${id}`, Series: `https://www.themoviedb.org/tv/${id}` })[type] || null,
    Tvdb: (id, type) => ({ Movie: `https://thetvdb.com/dereferrer/movie/${id}`, Series: `https://thetvdb.com/?tab=series&id=${id}` })[type] || null,
};
const PROVIDER_NAMES = { Imdb: 'IMDb', Tmdb: 'TMDb', Tvdb: 'TheTVDB' };

// The item's external links: the server's ExternalUrls, then IMDb, TMDb and TheTVDB from ProviderIds where
// those are missing. Only http(s) links are kept.
function externalLinks(item) {
    const links = (item.ExternalUrls || []).filter(l => l?.Name && /^https?:\/\//i.test(l.Url || ''));
    for (const [key, url] of Object.entries(PROVIDER_URLS)) {
        const id = item.ProviderIds?.[key];
        const name = PROVIDER_NAMES[key];
        if (!id || links.some(l => l.Name === name || l.Name === key)) continue;
        const built = url(encodeURIComponent(id), item.Type);
        if (built) links.push({ Name: name, Url: built });
    }
    if (item.HomePageUrl && /^https?:\/\//i.test(item.HomePageUrl)) links.unshift({ Name: t('detail.website'), Url: item.HomePageUrl });
    return links;
}

function externalLinksHtml(item) {
    const links = externalLinks(item);
    if (links.length === 0) return '';
    return `<p class="jf-external-links mb-8 text-sm"><span class="text-white/50 mr-2">${t('detail.links')}</span>${links.map(l =>
        `<a href="${escapeHtml(l.Url)}" target="_blank" rel="noopener noreferrer" class="text-[#00a4dc] hover:text-[#38bdf8] hover:underline">${escapeHtml(l.Name)}</a>`).join(', ')}</p>`;
}

// A language code as a name in the page's language ("eng" -> "English"); the code when there is none
function languageName(code) {
    if (!code || code === 'und') return '';
    try {
        const name = new Intl.DisplayNames([document.documentElement.lang || 'en'], { type: 'language' }).of(code);
        return name && name !== 'root' ? name : code;
    } catch {
        return code;
    }
}

// A stream's label: the server's DisplayTitle, else its language, codec and layout, as Jellyfin builds it
function streamLabel(stream) {
    if (stream.DisplayTitle) return stream.DisplayTitle;
    const parts = [stream.Title, languageName(stream.Language), (stream.Codec || '').toUpperCase()];
    if (stream.Type === 'Audio') parts.push(stream.ChannelLayout);
    if (stream.IsForced) parts.push(t('detail.forced'));
    if (stream.IsDefault) parts.push(t('detail.default'));
    if (stream.IsExternal) parts.push(t('detail.external'));
    return parts.filter(Boolean).join(' - ') || t('detail.track_number', { number: stream.Index });
}

// The versions' labels: each one's name, else the part of its file name the versions do not share (as
// Jellyfin names "Film - 1080p.mkv" "1080p"), else its resolution
function versionLabels(sources) {
    const files = sources.map(s => (s.Path || '').split(/[\\/]/).pop().replace(/\.[^.]+$/, ''));
    let shared = files.every(Boolean) && files.length > 1 ? files[0].length : 0;
    for (const file of files) while (shared > 0 && file.slice(0, shared) !== files[0].slice(0, shared)) shared--;
    // Cut only at a separator, so "Film 1080p" and "Film 10bit" keep "1080p" and "10bit"
    if (files.some(file => file.length > shared)) shared = files[0].slice(0, shared).search(/[\s\-_.][^\s\-_.]*$/) + 1;
    return sources.map((source, index) => {
        if (source.Name) return source.Name;
        const rest = shared ? files[index].slice(shared).replace(/^[\s\-_.[(]+|[\s)\]]+$/g, '') : '';
        if (rest) return rest;
        if (files[index]) return files[index];
        const video = (source.MediaStreams || []).find(s => s.Type === 'Video');
        return video?.Height ? `${video.Height}p` : t('detail.version_number', { number: index + 1 });
    });
}

// The source a choice plays, with its audio and subtitle streams and the index each select shows: the
// choice's, else the source's default, else (audio) the first track or (subtitles) off
function chosenTracks(choice) {
    const source = choice.sources.find(s => s.Id === choice.mediaSourceId) || choice.sources[0];
    const streams = source?.MediaStreams || [];
    const audio = streams.filter(s => s.Type === 'Audio');
    const subs = streams.filter(s => s.Type === 'Subtitle');
    const pick = (list, wanted, fallback) => (list.some(s => s.Index === wanted) ? wanted : fallback);
    const audioDefault = pick(audio, source?.DefaultAudioStreamIndex, (audio.find(s => s.IsDefault) || audio[0])?.Index ?? null);
    const subDefault = pick(subs, source?.DefaultSubtitleStreamIndex, (subs.find(s => s.IsForced) || subs.find(s => s.IsDefault))?.Index ?? -1);
    return {
        source,
        audio,
        subs,
        audioIndex: choice.audioIndex ?? audioDefault,
        subtitleIndex: choice.subtitleIndex ?? subDefault,
    };
}

// The version, audio and subtitle selects. Versions show for more than one source, subtitles when there
// are any; a single audio track shows disabled, as in Jellyfin Web.
function trackSelectsHtml(choice) {
    if (!choice || choice.sources.length === 0) return '';
    const { source, audio, subs, audioIndex, subtitleIndex } = chosenTracks(choice);
    const select = (name, label, options, disabled = false) => `
        <label class="flex flex-col gap-1 text-xs text-white/60 min-w-0">${escapeHtml(label)}
            <select class="jf-input h-9 !py-0 text-sm max-w-xs" data-track-select="${name}"${disabled ? ' disabled' : ''}>${options}</select>
        </label>`;
    const option = (value, label, selected) => `<option value="${escapeHtml(String(value))}"${selected ? ' selected' : ''}>${escapeHtml(label)}</option>`;
    const parts = [];
    if (choice.sources.length > 1) {
        const labels = versionLabels(choice.sources);
        parts.push(select('version', t('detail.version'), choice.sources.map((s, i) => option(s.Id, labels[i], s === source)).join('')));
    }
    if (audio.length > 0) {
        parts.push(select('audio', t('detail.audio'), audio.map(s => option(s.Index, streamLabel(s), s.Index === audioIndex)).join(''), audio.length === 1));
    }
    if (subs.length > 0) {
        parts.push(select('subtitle', t('detail.subtitles'),
            option(-1, t('detail.subtitles_off'), subtitleIndex === -1) + subs.map(s => option(s.Index, streamLabel(s), s.Index === subtitleIndex)).join('')));
    }
    return parts.join('');
}

// A change in one of the selects. A new version starts from its own defaults.
function changeTrackSelect(choice, name, value) {
    if (name === 'version') return { ...choice, mediaSourceId: value, audioIndex: null, subtitleIndex: null };
    if (name === 'audio') return { ...choice, audioIndex: Number(value) };
    if (name === 'subtitle') return { ...choice, subtitleIndex: Number(value) };
    return choice;
}

// What the player gets for item `itemId`: the version when there is a choice of them, and each track the
// user picked. Untouched tracks are left out, so the player's own defaults (remembered tracks) apply.
function detailSelection(itemId) {
    if (!detailChoice || detailChoice.itemId !== itemId) return null;
    const selection = {};
    if (detailChoice.sources.length > 1) selection.mediaSourceId = chosenTracks(detailChoice).source.Id;
    if (detailChoice.audioIndex != null) selection.audioIndex = detailChoice.audioIndex;
    if (detailChoice.subtitleIndex != null) selection.subtitleIndex = detailChoice.subtitleIndex;
    return selection;
}

// Media info of one version: its container, size and bitrate, then one entry per stream
function mediaInfo(source) {
    if (!source) return null;
    const kbps = bps => (bps ? t('detail.kbps', { value: Math.round(bps / 1000) }) : '');
    const mb = bytes => (bytes ? t('detail.megabytes', { value: Math.round(bytes / 1048576) }) : '');
    const fps = rate => {
        const [n, d] = String(rate || '').split('/').map(Number);
        const value = d ? n / d : Number(rate);
        return value > 0 ? t('detail.fps', { value: Math.round(value * 100) / 100 }) : '';
    };
    const yes = flag => (flag ? t('detail.yes') : '');
    const facts = list => list.filter(([, value]) => value !== '' && value != null);
    const streams = (source.MediaStreams || []).filter(s => ['Video', 'Audio', 'Subtitle'].includes(s.Type)).map(s => {
        if (s.Type === 'Video') {
            return { type: s.Type, title: streamLabel(s), facts: facts([
                ['detail.codec', (s.Codec || '').toUpperCase()],
                ['detail.profile', s.Profile],
                ['detail.resolution', s.Width && s.Height ? `${s.Width}×${s.Height}` : ''],
                ['detail.framerate', fps(s.RealFrameRate || s.AverageFrameRate)],
                ['detail.bitrate', kbps(s.BitRate)],
                ['detail.video_range', s.VideoRange],
            ]) };
        }
        if (s.Type === 'Audio') {
            return { type: s.Type, title: streamLabel(s), facts: facts([
                ['detail.language', languageName(s.Language)],
                ['detail.codec', (s.Codec || '').toUpperCase()],
                ['detail.layout', s.ChannelLayout || (s.Channels ? String(s.Channels) : '')],
                ['detail.bitrate', kbps(s.BitRate)],
                ['detail.default', yes(s.IsDefault)],
            ]) };
        }
        return { type: s.Type, title: streamLabel(s), facts: facts([
            ['detail.language', languageName(s.Language)],
            ['detail.codec', (s.Codec || '').toUpperCase()],
            ['detail.default', yes(s.IsDefault)],
            ['detail.forced', yes(s.IsForced)],
            ['detail.external', yes(s.IsExternal)],
        ]) };
    });
    return {
        facts: facts([
            ['detail.container', (source.Container || '').toUpperCase()],
            ['detail.size', mb(source.Size)],
            ['detail.bitrate', kbps(source.Bitrate)],
        ]),
        streams,
    };
}

function mediaInfoHtml(source) {
    const info = mediaInfo(source);
    if (!info || (info.facts.length === 0 && info.streams.length === 0)) return '';
    const factList = facts => `<dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5">${facts.map(([key, value]) =>
        `<dt class="text-white/50">${t(key)}</dt><dd class="text-white/85">${escapeHtml(String(value))}</dd>`).join('')}</dl>`;
    const typeKey = { Video: 'detail.video', Audio: 'detail.audio', Subtitle: 'detail.subtitle' };
    return `
        <details class="jf-media-info mb-10 max-w-4xl" data-media-info>
            <summary class="cursor-pointer text-lg font-bold text-white mb-2">${t('detail.media_info')}</summary>
            <div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3 text-xs">
                ${info.facts.length ? `<div class="rounded-lg bg-white/5 p-3"><h3 class="font-bold text-white mb-1">${t('detail.media_file')}</h3>${factList(info.facts)}</div>` : ''}
                ${info.streams.map(s => `<div class="rounded-lg bg-white/5 p-3"><h3 class="font-bold text-white mb-1">${t(typeKey[s.type])}: ${escapeHtml(s.title)}</h3>${factList(s.facts)}</div>`).join('')}
            </div>
        </details>`;
}

// A position as h:mm:ss, or m:ss under an hour
function clockTime(ticks) {
    const total = Math.floor((ticks || 0) / 1e7);
    const pad = n => String(n).padStart(2, '0');
    const h = Math.floor(total / 3600);
    const m = Math.floor(total / 60) % 60;
    return h > 0 ? `${h}:${pad(m)}:${pad(total % 60)}` : `${m}:${pad(total % 60)}`;
}

// The chapters row ("Scenes" in Jellyfin Web): an image where the server has one, the name and start time;
// a click plays the item from that chapter with the detail page's track choice
function chaptersHtml(item) {
    const chapters = item.Chapters || [];
    if (chapters.length === 0) return '';
    const id = item.Id;
    return `
        <div class="mb-10 jf-chapters">
            <h2 class="text-xl font-bold mb-4 text-white">${t('detail.chapters')}</h2>
            <div class="jf-scroll-row pb-2">
                ${chapters.map((c, index) => {
                    const name = c.Name || t('detail.chapter_number', { number: index + 1 });
                    const start = clockTime(c.StartPositionTicks);
                    const img = c.ImageTag ? API.imageUrl(id, `Chapter/${index}`, { maxWidth: 400, tag: c.ImageTag }) : '';
                    return `
                    <button type="button" class="w-56 flex-shrink-0 text-left group" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="Videos" data-title="${escapeHtml(item.Name || '')}" data-start-ticks="${Number(c.StartPositionTicks) || 0}" data-detail-tracks="true">
                        <div class="relative aspect-video rounded-lg overflow-hidden bg-white/5 mb-1.5 border-2 border-transparent group-hover:border-[#00a4dc] transition-all">
                            ${img ? `<img src="${img}" alt="" loading="lazy" class="w-full h-full object-cover" onerror="this.style.display='none'">` : `<div class="w-full h-full flex items-center justify-center text-white/30">${icon('Film', { size: 28 })}</div>`}
                        </div>
                        <p class="text-xs font-semibold text-white truncate group-hover:text-[#38bdf8] transition-colors">${escapeHtml(name)}</p>
                        <p class="text-[10px] text-white/50">${start}</p>
                    </button>`;
                }).join('')}
            </div>
        </div>`;
}

// Jellyfin's ExtraType of a special feature, as its label's i18n key
const EXTRA_TYPE_KEYS = {
    BehindTheScenes: 'detail.extra_behind_the_scenes',
    DeletedScene: 'detail.extra_deleted_scene',
    Interview: 'detail.extra_interview',
    Scene: 'detail.extra_scene',
    Sample: 'detail.extra_sample',
    Short: 'detail.extra_short',
    Featurette: 'detail.extra_featurette',
    Clip: 'detail.extra_clip',
};

// The Trailer button, as Jellyfin Web's: a local trailer plays in the player; without one, the first remote
// trailer (YouTube, from the metadata) opens in a new tab
function trailerButtonHtml(item, localTrailers) {
    const label = `${icon('Film', { size: 18 })} ${t('detail.trailer')}`;
    const cls = 'jf-btn-ghost h-11 !py-0 !px-4 text-sm';
    const local = localTrailers[0];
    if (local) {
        return `<button class="${cls}" data-action="play" data-id="${escapeHtml(local.Id)}" data-stream-type="Videos" data-title="${escapeHtml(local.Name || item.Name || '')}" data-from-start="true" data-trailer="true">${label}</button>`;
    }
    const remote = (item.RemoteTrailers || []).find(r => /^https?:\/\//i.test(r.Url || ''));
    if (!remote) return '';
    return `<a class="${cls}" href="${escapeHtml(remote.Url)}" target="_blank" rel="noopener noreferrer" data-trailer="true" title="${escapeHtml(remote.Name || t('detail.trailer'))}">${label}</a>`;
}

// The special features row: each one plays in the player
function specialFeaturesHtml(features) {
    if (features.length === 0) return '';
    return `
        <div class="mb-10 jf-special-features">
            <h2 class="text-xl font-bold mb-4 text-white">${t('detail.special_features')}</h2>
            <div class="jf-scroll-row pb-2">
                ${features.map(f => {
                    const img = f.ImageTags?.Primary ? API.imageUrl(f.Id, 'Primary', { maxWidth: 400, tag: f.ImageTags.Primary }) : '';
                    const kind = EXTRA_TYPE_KEYS[f.ExtraType] ? t(EXTRA_TYPE_KEYS[f.ExtraType]) : '';
                    const runtime = formatRuntime(f.RunTimeTicks);
                    return `
                    <button type="button" class="w-56 flex-shrink-0 text-left group" data-action="play" data-id="${escapeHtml(f.Id)}" data-stream-type="Videos" data-title="${escapeHtml(f.Name || '')}">
                        <div class="relative aspect-video rounded-lg overflow-hidden bg-white/5 mb-1.5 border-2 border-transparent group-hover:border-[#00a4dc] transition-all">
                            ${img ? `<img src="${img}" alt="" loading="lazy" class="w-full h-full object-cover" onerror="this.style.display='none'">` : `<div class="w-full h-full flex items-center justify-center text-white/30">${icon('Film', { size: 28 })}</div>`}
                        </div>
                        <p class="text-xs font-semibold text-white truncate group-hover:text-[#38bdf8] transition-colors">${escapeHtml(f.Name || '')}</p>
                        <p class="text-[10px] text-white/50">${[kind, runtime].filter(Boolean).map(escapeHtml).join(' · ')}</p>
                    </button>`;
                }).join('')}
            </div>
        </div>`;
}

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
        // A photo's Play opens the photo viewer (photos.js)
        const canPlay = ['Movie', 'Episode', 'Audio', 'MusicVideo', 'Video', 'Photo'].includes(type);
        // Containers play their items as a queue: Play all from the start, Shuffle in a random order
        const canPlayAll = ['Series', 'Season', 'MusicAlbum', 'MusicArtist', 'Playlist', 'BoxSet', 'Folder'].includes(type);
        const streamType = type === 'Audio' ? 'Audio' : type === 'Photo' ? 'Photo' : 'Videos';
        const resumeTicks = streamType === 'Videos' ? resumePositionTicks(item) : 0;
        detailChoice = canPlay && streamType === 'Videos'
            ? { itemId: id, sources: item.MediaSources || [], mediaSourceId: null, audioIndex: null, subtitleIndex: null }
            : null;
        const isContainerOfEpisodes = type === 'Series' || type === 'Season';
        const tagline = (item.Taglines || [])[0] || '';
        const ends = endsAt(item);

        // Parallel fetch: Children/Seasons/Episodes AND Similar media
        // Previously these were sequential, causing a waterfall effect
        const isContainer = ['Series', 'Season', 'MusicAlbum', 'Folder'].includes(type);
        // The extras: trailer files for the Trailer button, and special features on movie, series and season pages
        const showsSpecials = ['Movie', 'Series', 'Season'].includes(type);
        const [childrenData, similarData, localTrailers, specialFeatures] = await Promise.all([
            isContainer ? API.getChildren(id).catch(() => ({ Items: [] })) : Promise.resolve({ Items: [] }),
            API.getSimilar(id).catch(() => ({ Items: [] })),
            item.LocalTrailerCount > 0 ? API.getLocalTrailers(id).catch(() => []) : Promise.resolve([]),
            showsSpecials && item.SpecialFeatureCount > 0 ? API.getSpecialFeatures(id).catch(() => []) : Promise.resolve([]),
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
            } else if (type === 'MusicAlbum') {
                seasonsOrEpisodesHtml = albumTrackList(item, children);
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
                ${backdropSrc ? `<img src="${backdropSrc}" class="jf-item-backdrop w-full h-[460px] object-cover opacity-35" onerror="this.style.display='none'">` : ''}
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
                            ${criticRatingHtml(item)}
                            ${ends ? `<span class="jf-badge jf-ends-at text-[11px]">${t('detail.ends_at', { time: ends.toLocaleTimeString(document.documentElement.lang || [], { hour: 'numeric', minute: '2-digit' }) })}</span>` : ''}
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
                            <button class="jf-btn-primary h-11 !py-0 !px-7 text-sm font-bold shadow-xl" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(name)}"${resumeTicks > 0 ? ' data-resume="true"' : ''} data-detail-tracks="true">
                                ${icon('Play', { size: 20, class: 'fill-current' })}
                                ${resumeTicks > 0 ? t('detail.resume') : t('common.play')}
                            </button>
                            ${resumeTicks > 0 ? `
                            <button class="jf-btn-ghost h-11 !py-0 !px-4 text-sm" data-action="play" data-id="${escapeHtml(id)}" data-stream-type="${escapeHtml(streamType)}" data-title="${escapeHtml(name)}" data-from-start="true" data-detail-tracks="true">
                                ${t('detail.play_from_beginning')}
                            </button>` : ''}` : ''}
                            ${canPlayAll ? `
                            <button class="jf-btn-primary h-11 !py-0 !px-7 text-sm font-bold shadow-xl" data-action="play-all" data-id="${escapeHtml(id)}">
                                ${icon('Play', { size: 20, class: 'fill-current' })}
                                ${t('common.play')}
                            </button>
                            <button class="jf-btn-ghost h-11 !py-0 !px-4 text-sm" data-action="shuffle" data-id="${escapeHtml(id)}">
                                ${icon('Shuffle', { size: 18 })}
                                ${t('player.shuffle')}
                            </button>` : ''}
                            ${trailerButtonHtml(item, Array.isArray(localTrailers) ? localTrailers : [])}

                            <button class="jf-btn-ghost h-11 !py-0 w-11 !px-0 sm:w-auto sm:!px-4 text-sm" title="${escapeHtml(t('detail.toggle_favorite'))}" aria-label="${escapeHtml(t('detail.toggle_favorite'))}" data-action="toggle-favorite" data-id="${escapeHtml(id)}" data-favorite="${fav}">
                                ${icon('Heart', { size: 18, class: fav ? 'text-rose-500 fill-current' : 'text-white/60' })}
                                <span class="hidden sm:inline text-xs ml-1">${fav ? t('detail.favorite') : t('detail.add_favorite')}</span>
                            </button>

                            <button class="jf-btn-ghost h-11 !py-0 !px-4 text-xs font-semibold" data-action="toggle-played" data-id="${escapeHtml(id)}" data-played="${played}" data-all="${isContainerOfEpisodes}" title="${escapeHtml(playedButtonTitle(isContainerOfEpisodes, played))}">
                                ${played ? `<span class="inline-flex items-center gap-1 text-emerald-400">${icon('Check', { size: 14 })} ${isContainerOfEpisodes ? t('detail.all_played') : t('detail.played')}</span>` : `<span class="inline-flex items-center gap-1 text-white/60">${icon('Circle', { size: 14 })} ${isContainerOfEpisodes ? t('detail.mark_all_played') : t('detail.unplayed')}</span>`}
                            </button>

                            ${API.isAdmin ? `
                            <button class="jf-btn-ghost h-11 !py-0 w-11 !px-0" title="${escapeHtml(t('detail.refresh_metadata'))}" aria-label="${escapeHtml(t('detail.refresh_metadata'))}" data-action="refresh-item" data-id="${escapeHtml(id)}">
                                ${icon('RefreshCw', { size: 18, class: 'jf-busy-spin' })}
                            </button>
                            ${metadataToolButtons(item)}
                            <button class="jf-btn-ghost h-11 !py-0 w-11 !px-0 hover:!text-rose-400" title="${escapeHtml(t('detail.delete_item'))}" aria-label="${escapeHtml(t('detail.delete_item'))}" data-action="delete-item" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}" data-type="${escapeHtml(type)}" data-parent-id="${escapeHtml(deleteParentId(item))}">
                                ${icon('Trash2', { size: 18 })}
                            </button>` : ''}
                            ${!API.isAdmin && type === 'Playlist' && item.CanDelete ? `
                            <button class="jf-btn-ghost h-11 !py-0 w-11 !px-0 hover:!text-rose-400" title="${escapeHtml(t('detail.delete_item'))}" aria-label="${escapeHtml(t('detail.delete_item'))}" data-action="delete-item" data-id="${escapeHtml(id)}" data-name="${escapeHtml(name)}" data-type="${escapeHtml(type)}" data-parent-id="">
                                ${icon('Trash2', { size: 18 })}
                            </button>` : ''}
                            <button class="jf-btn-ghost h-11 !py-0 w-11 !px-0" title="${escapeHtml(t('cards.more'))}" aria-label="${escapeHtml(t('cards.more'))}" aria-haspopup="menu" data-action="card-menu" data-id="${escapeHtml(id)}">
                                ${icon('EllipsisVertical', { size: 18 })}
                            </button>
                        </div>
                        ${detailChoice ? `<div id="detail-track-selects" class="flex flex-wrap gap-3 mt-4">${trackSelectsHtml(detailChoice)}</div>` : ''}
                    </div>
                </div>
            </div>

            <!-- Overview section -->
            ${overview || tagline ? `
            <div class="mb-8 max-w-4xl">
                ${tagline ? `<h3 class="jf-tagline text-base sm:text-lg italic text-white/85 mb-2">${escapeHtml(tagline)}</h3>` : ''}
                ${overview ? `
                <h2 class="text-lg font-bold mb-2 text-white">${t('detail.overview')}</h2>
                <p class="text-white/75 text-sm sm:text-base leading-relaxed">${escapeHtml(overview)}</p>` : ''}
            </div>` : ''}

            ${externalLinksHtml(item)}
            ${creditLinesHtml(item)}

            <!-- Media info of the chosen version -->
            <div id="detail-media-info">${detailChoice ? mediaInfoHtml(chosenTracks(detailChoice).source) : ''}</div>

            <!-- Cast & Crew -->
            ${castHtml}

            <!-- Seasons / Episodes -->
            ${seasonsOrEpisodesHtml}

            <!-- Special features -->
            ${specialFeaturesHtml(Array.isArray(specialFeatures) ? specialFeatures : [])}

            <!-- A collection's or playlist's items (collections.js) -->
            ${containerSectionHtml(item)}

            <!-- Chapters -->
            ${canPlay && streamType === 'Videos' ? chaptersHtml(item) : ''}

            <!-- Similar Titles -->
            ${similarHtml}
        </div>`;
        if (type === 'BoxSet' || type === 'Playlist') renderContainerItems(item);
        playThemeSong(id, () => currentDetailId === itemId && API.token === session);
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
                <div id="person-known-grid" class="jf-card-grid grid grid-cols-2 sm:grid-cols-3 md:grid-cols-4 lg:grid-cols-5 xl:grid-cols-7 gap-4"></div>
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

// A series or season marks all its episodes; the server applies the mark to each of them
function playedButtonTitle(all, played) {
    if (all) return played ? t('detail.mark_all_unplayed') : t('detail.mark_all_played');
    return played ? t('detail.mark_unplayed') : t('detail.mark_played');
}

async function togglePlayed(id, current, all = false) {
    try {
        if (current) await API.markUnplayed(id);
        else await API.markPlayed(id);
        // Marking changes the item's (and a show's episodes') watch data, so drop the cached lists before re-rendering
        API.invalidateCache('/Items');
        API.invalidateCache('/Shows');
        if (all) toast(current ? t('detail.all_marked_unplayed') : t('detail.all_marked_played'));
        else toast(current ? t('detail.marked_unplayed') : t('detail.marked_played'));
        if (currentDetailId === id) renderDetail(id);
    } catch (e) {
        toast(e.message, 'error');
    }
}

// Admin: the metadata editor, Identify (for the types the provider knows) and the image editor
function metadataToolButtons(item) {
    const tools = [['edit-metadata', 'Pencil', 'metadata.edit_metadata']];
    if (item.Type === 'Movie' || item.Type === 'Series') tools.push(['identify-item', 'ScanSearch', 'metadata.identify']);
    tools.push(['edit-images', 'Images', 'metadata.edit_images']);
    return tools.map(([action, name, key]) => `<button class="jf-btn-ghost h-11 !py-0 w-11 !px-0" title="${escapeHtml(t(key))}" aria-label="${escapeHtml(t(key))}" data-action="${action}" data-id="${escapeHtml(item.Id)}">
                                ${icon(name, { size: 18 })}
                            </button>`).join('');
}

// The page an item's deletion leaves for: its season, series or album, else home
function deleteParentId(item) {
    const parent = item.SeasonId || item.SeriesId || item.AlbumId || '';
    return parent === item.Id ? '' : parent;
}

// Admin: removes the item (and what is under it) after a confirmation. The button carries the item's own
// name, type and parent. A collection or playlist is gone for good; anything else only leaves the library,
// and its files stay.
async function deleteItemFromDetail({ id, name = '', type = '', parentId = '' }) {
    const permanent = type === 'BoxSet' || type === 'Playlist';
    if (!confirm(t(permanent ? 'detail.delete_confirm_permanent' : 'detail.delete_confirm', { name }))) return;
    try {
        await API.deleteItem(id);
        API.invalidateCache('/Items');
        API.invalidateCache('/Shows');
        toast(t('detail.deleted', { name }));
        navigate(...(parentId ? ['detail', parentId] : ['home']));
    } catch (e) {
        toast(t('detail.delete_failed', { message: e.message }), 'error');
    }
}

// The version, audio and subtitle selects: a new version redraws the selects and the media info for it
document.addEventListener('change', e => {
    const name = e.target.dataset?.trackSelect;
    if (!name || !detailChoice) return;
    detailChoice = changeTrackSelect(detailChoice, name, e.target.value);
    if (name !== 'version') return;
    const selects = $('detail-track-selects');
    if (selects) selects.innerHTML = trackSelectsHtml(detailChoice);
    const info = $('detail-media-info');
    if (info) {
        const open = info.querySelector('details')?.open;
        info.innerHTML = mediaInfoHtml(chosenTracks(detailChoice).source);
        if (open) info.querySelector('details').open = true;
    }
});

export {
    currentDetailId, renderDetail, renderPerson, toggleFavorite, togglePlayed, deleteItemFromDetail, detailSelection,
    versionLabels,
};
