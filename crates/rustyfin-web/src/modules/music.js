// Music: the album page's track list, and a music library's tabs (albums, album artists, artists,
// songs and genres)
import { icon, escapeHtml } from './util.js';

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

export { trackDuration, trackDiscGroups, trackRow, albumTrackList };
