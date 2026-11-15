// The play queue: Play all and Shuffle on a series, season, album, artist, playlist, collection or library,
// and the player's previous and next through the items it holds
import { toast } from './util.js';
import { playItem, playWithResumeChoice } from './player.js';

// The queue in play, or null: { items, index }. A play that doesn't come from the queue clears it.
let playQueue = null;
// Jellyfin Web plays at most this many items of a folder, and shuffles 100 episodes of a series.
// Play all on a series lists every episode, so its start (Next Up) is always in the list.
const QUEUE_LIMIT = 300;
const SERIES_LIMIT = 100;

function queueStreamType(item) {
    return item.MediaType === 'Audio' || item.Type === 'Audio' ? 'Audio' : 'Videos';
}

// The request that lists a container's playable items in play order, as Jellyfin Web's playback manager builds
// it: a series or season in episode order, an album by disc and track, a playlist and a collection as they are
// saved, anything else by name; Shuffle asks the server for a random order instead.
function queueQuery(item, shuffle) {
    const q = new URLSearchParams();
    const type = item.Type;
    if ((type === 'Series' || type === 'Season') && !shuffle) {
        const seriesId = type === 'Season' ? item.SeriesId || item.ParentId : item.Id;
        if (type === 'Season') q.set('SeasonId', item.Id);
        return `/Shows/${seriesId}/Episodes${type === 'Season' ? `?${q}` : ''}`;
    }
    if (type === 'MusicArtist') q.set('ArtistIds', item.Id);
    else q.set('ParentId', item.Id);
    q.set('Recursive', 'true');
    if (type === 'Series' || type === 'Season') q.set('IncludeItemTypes', 'Episode');
    else if (type === 'MusicAlbum' || type === 'MusicArtist' || item.CollectionType === 'music') q.set('MediaTypes', 'Audio');
    else q.set('MediaTypes', 'Audio,Video');
    if (shuffle) q.set('SortBy', 'Random');
    else if (type === 'MusicAlbum') q.set('SortBy', 'ParentIndexNumber,IndexNumber,SortName');
    else if (type === 'MusicArtist' || item.CollectionType === 'music') q.set('SortBy', 'Album,ParentIndexNumber,IndexNumber,SortName');
    else if (type !== 'Playlist' && type !== 'BoxSet') q.set('SortBy', 'SortName');
    q.set('Limit', String(type === 'Series' ? SERIES_LIMIT : QUEUE_LIMIT));
    return `/Items?${q}`;
}

// Where Play all starts, as Jellyfin does: a series at its Next Up episode (nextUpId) and else its first
// unplayed one, a season at its first unplayed episode; Shuffle and other containers at the top
function queueStartIndex(container, items, shuffle, nextUpId = null) {
    if (shuffle || (container.Type !== 'Series' && container.Type !== 'Season')) return 0;
    const nextUp = nextUpId ? items.findIndex(i => i.Id === nextUpId) : -1;
    if (nextUp >= 0) return nextUp;
    const unplayed = items.findIndex(i => !i.UserData?.Played);
    return unplayed >= 0 ? unplayed : 0;
}

// Plays a container's items from the start (Play all), or in a random order (Shuffle)
async function playContainer(id, { shuffle = false } = {}) {
    let items = [];
    let container = null;
    let nextUpId = null;
    try {
        container = await API.get('/Items/' + id);
        const series = container.Type === 'Series' && !shuffle;
        const [result, nextUp] = await Promise.all([
            API.get(queueQuery(container, shuffle)),
            series ? API.get(`/Shows/NextUp?SeriesId=${encodeURIComponent(id)}&Limit=1`).catch(() => null) : null,
        ]);
        items = (result?.Items || []).filter(i => !i.IsFolder && i.LocationType !== 'Virtual');
        nextUpId = nextUp?.Items?.[0]?.Id || null;
    } catch (e) {
        console.warn('Could not load the items to play:', e);
    }
    if (!items.length) {
        toast(t('player.queue_empty'), 'error');
        return;
    }
    playQueueAt(items, queueStartIndex(container, items, shuffle, nextUpId));
}

// Starts the queue at index. Its first item resumes or starts over as the user picks, like any Play; the
// items after it start from the beginning (playQueueStep).
function playQueueAt(items, index) {
    const item = items[index];
    if (!item) return;
    playQueue = { items, index };
    playWithResumeChoice(item.Id, queueStreamType(item), item.Name || '', { queued: true });
}

// The queue's items before and after the playing one, or null without a queue
function queueNeighbours() {
    if (!playQueue) return null;
    const { items, index } = playQueue;
    return { prev: items[index - 1] || null, next: items[index + 1] || null };
}

// Plays the queue's item step places away (-1 previous, 1 next); carry keeps the track languages
function playQueueStep(step, carry = null) {
    if (!playQueue) return false;
    const index = playQueue.index + step;
    const item = playQueue.items[index];
    if (!item) return false;
    playQueue.index = index;
    playItem(item.Id, queueStreamType(item), item.Name || '', { fromStart: true, queued: true, carry });
    return true;
}

function clearQueue() {
    playQueue = null;
}

export { queueQuery, queueStartIndex, queueStreamType, playContainer, playQueueAt, queueNeighbours, playQueueStep, clearQueue };
