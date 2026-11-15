// The video player: streams, seeking, audio and subtitle tracks, controls
import { $, icon, formatDurationSeconds } from './util.js';
import { cardImage } from './cards.js';
import { currentPage } from './router.js';
import { renderHome } from './home.js';
import { currentDetailId, renderDetail } from './item.js';
import { clearQueue, queueNeighbours, playQueueStep } from './queue.js';
let playerReportInterval = null;
let playerBars = null;

// HLS.js instance for adaptive streaming
let hlsInstance = null;
// The playback session in progress, or null when nothing is playing
let activePlayback = null;
// Promise of the last Stopped report; playItem waits for it before reading the resume point
let pendingStop = Promise.resolve();
// Incremented by each playItem call, so a call that finishes loading after a newer one gives up
let playRequest = 0;
// True while the seek bar is being dragged, so the timeline does not move the thumb
let seekDragging = false;

// Ticks to resume from. Items at or past 90 % of their runtime count as watched and start over.
function resumePositionTicks(item) {
    const pos = item?.UserData?.PlaybackPositionTicks || 0;
    if (!(pos > 0)) return 0;
    const runtime = item.RunTimeTicks || 0;
    if (runtime > 0 && pos >= runtime * 0.9) return 0;
    return pos;
}

// The resume point as m:ss under an hour and h:mm:ss from an hour on
function formatResumeTime(ticks) {
    const total = Math.floor(ticks / 1e7);
    const h = Math.floor(total / 3600);
    const m = Math.floor((total % 3600) / 60);
    const s = String(total % 60).padStart(2, '0');
    return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${s}` : `${m}:${s}`;
}

// Asks whether to resume at ticks or start over. Resolves 'resume' or 'start', or '' when the menu is cancelled
// (Escape, a click outside) or already open. Escape keeps the old returnValue, so it is cleared before opening.
function askResumeOrStart(title, ticks) {
    const menu = $('resume-menu');
    if (!menu || menu.open) return Promise.resolve('');
    $('resume-menu-title').textContent = title;
    $('resume-menu-resume').textContent = t('player.resume_from', { time: formatResumeTime(ticks) });
    menu.returnValue = '';
    return new Promise(resolve => {
        menu.addEventListener('close', () => resolve(menu.returnValue), { once: true });
        menu.showModal();
        // The buttons fade in with the dialog (input.css transitions all their properties, visibility too), so
        // autofocus finds Resume still hidden and focuses the backdrop; focus Resume once the fade is done
        setTimeout(() => { if (menu.open) $('resume-menu-resume').focus(); }, 250);
    });
}

// Starts an item from a card or a Play button. A video with a resume point asks first whether to resume.
async function playWithResumeChoice(itemId, streamType = 'Videos', title = '') {
    if (streamType !== 'Videos') return playItem(itemId, streamType, title);
    // Read the point after the last Stopped report and uncached, so it includes the position just saved
    await pendingStop;
    let item = null;
    try {
        item = await API.get('/Items/' + itemId);
    } catch (e) {
        console.warn('Could not load item for resume point:', e);
    }
    const ticks = resumePositionTicks(item);
    if (ticks <= 0) return playItem(itemId, streamType, title);
    const choice = await askResumeOrStart(title || item?.Name || '', ticks);
    if (choice === 'resume') playItem(itemId, streamType, title);
    else if (choice === 'start') playItem(itemId, streamType, title, { fromStart: true });
}

// Stops the session that is still playing, if any. Returns the promise of its Stopped report.
function stopPlayback() {
    if (activePlayback) activePlayback.stop();
    return pendingStop;
}

// Loads the stream of a session from startTicks (item ticks) and attaches it to the video element
async function openStream(session, startTicks) {
    const video = $('video-player');
    const { itemId, streamType } = session;
    const load = ++session.loads;
    // This load stays current until the session stops or a newer load starts
    const current = () => activePlayback === session && session.loads === load;
    const startSeconds = startTicks / 1e7;
    // Capture the subtitle before the request so a change made during it sees this load's burn-in
    const requestedSubtitle = session.subtitleIndex;
    session.burnedIn = streamType === 'Videos' && isImageSubtitle(findStream(session, 'subtitle', requestedSubtitle)?.Codec);

    let url;
    let isHls = false;
    let playSessionId = null;
    let mediaSourceId = null;
    if (streamType === 'Videos') {
        // Use Universal endpoint to get the optimal playback URL
        try {
            const data = await API.getUniversalVideoUrl(itemId, {
                startTimeTicks: startTicks,
                mediaSourceId: session.mediaSourceId,
                audioStreamIndex: session.audioIndex,
                subtitleStreamIndex: session.subtitleIndex,
            });
            if (!current()) return;
            if (data && data.url) {
                // Add auth token to the stream URL so the video element can authenticate
                const sep = data.url.includes('?') ? '&' : '?';
                url = API.base + data.url + sep + 'api_key=' + encodeURIComponent(API.token);
                isHls = data.url.includes('.m3u8');
                // Determine playback method from the URL, not from SupportsDirectPlay flag
                // which may be stale from the database
                let method;
                if (isHls) {
                    method = t('player.hls_transcode');
                } else if (data.url.includes('/stream')) {
                    // Check if it's a static direct play or direct stream
                    method = data.url.includes('Static=true') ? t('player.direct_play') : t('player.direct_stream');
                } else {
                    method = t('player.stream');
                }
                $('player-subtitle').textContent = method;
                // Every load gets its own id from the server; the reports carry it, and an HLS one stops the transcode
                playSessionId = data.PlaySessionId || null;
                mediaSourceId = data.MediaSources?.[0]?.Id || null;
            } else {
                // Fallback: direct stream
                url = API.streamUrl(itemId, streamType);
                $('player-subtitle').textContent = t('player.video_stream_fallback');
            }
        } catch (e) {
            if (!current()) return;
            console.warn('Universal endpoint failed, using direct stream:', e);
            url = API.streamUrl(itemId, streamType);
            $('player-subtitle').textContent = t('player.video_stream_fallback');
        }
    } else if (streamType === 'Audio') {
        url = API.streamUrl(itemId, streamType);
        $('player-subtitle').textContent = t('player.audio_stream');
    } else {
        url = API.streamUrl(itemId, streamType);
        $('player-subtitle').textContent = t('player.stream');
    }

    session.offset = isHls ? startSeconds : 0;
    session.startAt = startSeconds;
    session.isHls = isHls;
    session.playSessionId = playSessionId;
    session.mediaSourceId = mediaSourceId || session.mediaSourceId;
    session.burnedIn = isHls && isImageSubtitle(findStream(session, 'subtitle', requestedSubtitle)?.Codec);

    video.onerror = () => {
        if (!current()) return;
        console.error('Video playback error:', video.error);
        session.failed = true;
        const errMsg = video.error ? video.error.message : t('common.unknown_error');
        $('player-subtitle').textContent = t('common.error_message', { message: errMsg });
    };

    // Handle HLS streams with hls.js
    if (isHls && typeof Hls !== 'undefined' && Hls.isSupported()) {
        // The transcode already starts at startTicks, so its playlist starts at 0
        hlsInstance = new Hls({
            enableWorker: true,
            lowLatencyMode: false,
            // The subtitle menu doesn't list captions in the stream, so they couldn't be turned off
            enableCEA708Captions: false,
            startPosition: 0,
        });
        hlsInstance.loadSource(url);
        hlsInstance.attachMedia(video);
        hlsInstance.on(Hls.Events.MANIFEST_PARSED, () => {
            if (!current()) return;
            video.play().catch(e => console.warn('Autoplay block:', e));
        });
        // A transcode that starts after the last keyframe of copied video lists no time: nothing is left
        // to play, so end the playback as the stream's end does
        hlsInstance.on(Hls.Events.LEVEL_LOADED, (event, data) => {
            if (!current() || session.offset <= 0 || data.details.totalduration !== 0) return;
            session.stop();
            upNextAtEnd(session);
        });
        hlsInstance.on(Hls.Events.ERROR, (event, data) => {
            if (!current()) return;
            console.error('HLS error:', data);
            if (data.fatal) {
                $('player-subtitle').textContent = t('player.hls_error');
                if (data.type === Hls.ErrorTypes.NETWORK_ERROR) {
                    hlsInstance.startLoad();
                } else if (data.type === Hls.ErrorTypes.MEDIA_ERROR) {
                    hlsInstance.recoverMediaError();
                } else {
                    // Playback that can't recover is reported as failed, which leaves the watched state alone
                    session.failed = true;
                    hlsInstance.destroy();
                }
            }
        });
    } else if (isHls && video.canPlayType('application/vnd.apple.mpegurl')) {
        // Safari native HLS support; its playlist also starts at startTicks, so it needs no seek
        video.src = url;
        video.addEventListener('loadedmetadata', () => {
            if (!current()) return;
            video.play().catch(e => console.warn('Autoplay block:', e));
        }, { once: true });
    } else {
        // Direct play/stream
        video.src = url;
        if (startSeconds > 0) {
            video.addEventListener('loadedmetadata', () => {
                if (current()) video.currentTime = startSeconds;
            }, { once: true });
        }
        video.play().catch(e => console.warn('Autoplay block:', e));
    }
    if (streamType === 'Videos') showTextSubtitle(session);
    updatePlayerControls(session);
}

// Shows the item time and the bar; the bar stays still while the thumb is being dragged
function updatePlayerControls(session) {
    const pos = session.position();
    const total = session.total();
    $('player-duration').textContent = formatDurationSeconds(total);
    // previewSeek owns the elapsed time and the bar during a drag
    if (seekDragging) return;
    $('player-cur-time').textContent = formatDurationSeconds(pos);
    $('player-seek').value = total > 0 ? Math.min(100, pos / total * 100) : 0;
}

// Moves within the transcoded range in place; any other HLS point restarts the transcode after a pause
function seekTo(session, seconds) {
    const video = $('video-player');
    const total = session.total();
    if (total > 0) seconds = Math.min(seconds, total);
    seconds = Math.max(0, seconds);
    // A waiting track change restarts at the new point instead
    if (session.tracksChanged) return scheduleRestart(session, seconds);
    if (!session.isHls) {
        video.currentTime = seconds;
        return;
    }
    const rel = seconds - session.offset;
    const seekable = video.seekable;
    const end = seekable.length ? seekable.end(seekable.length - 1) : 0;
    if (rel >= 0 && rel <= end) {
        clearTimeout(session.seekTimer);
        session.seekTimer = null;
        session.pendingSeek = null;
        video.currentTime = rel;
        return;
    }
    scheduleRestart(session, seconds);
}

// Waits 400 ms after the last seek, then restarts the transcode at that point
function scheduleRestart(session, seconds) {
    session.pendingSeek = seconds;
    updatePlayerControls(session);
    clearTimeout(session.seekTimer);
    session.seekTimer = setTimeout(() => { session.seekTimer = null; restartStream(session, seconds); }, 400);
}

// Starts the transcode again at seconds of item time, stopping the old transcode first
async function restartStream(session, seconds) {
    const total = session.total();
    // The last second holds nothing to transcode, so end the playback as the stream's end does
    // (pendingSeek makes the Stopped report say the end)
    if (total > 0 && seconds > total - 1) {
        session.pendingSeek = total;
        session.stop();
        upNextAtEnd(session);
        return;
    }
    session.tracksChanged = false;
    session.pendingSeek = seconds;
    if (hlsInstance) {
        hlsInstance.destroy();
        hlsInstance = null;
    }
    if (session.isHls && session.playSessionId) API.stopActiveEncodings(session.playSessionId).catch(() => {});
    await openStream(session, Math.round(seconds * 1e7));
    // A newer seek keeps its own target
    if (activePlayback === session && session.pendingSeek === seconds && !session.seekTimer) session.pendingSeek = null;
}

// Seeks to a point on the bar once the drag ends
function seekVideo(percent) {
    seekDragging = false;
    hideTrickplay();
    if (!activePlayback) return;
    seekTo(activePlayback, percent / 100 * activePlayback.total());
}

// Skips relative to the current item time
function skipTime(deltaSeconds) {
    if (!activePlayback) return;
    seekTo(activePlayback, activePlayback.position() + deltaSeconds);
}

// Shows the elapsed time under the thumb, and the trickplay thumbnail over it, while the bar is being dragged
function previewSeek(percent) {
    if (!activePlayback) return;
    seekDragging = true;
    $('player-cur-time').textContent = formatDurationSeconds(percent / 100 * activePlayback.total());
    showTrickplay(Number(percent));
}

// Picks another audio track and restarts the transcode at the current point
function changeAudioTrack(value) {
    const session = activePlayback;
    if (!session) return;
    session.audioIndex = Number(value);
    session.tracksChanged = true;
    scheduleRestart(session, session.position());
}

// A text subtitle is added to the player; a picture one is burned in, so it restarts the transcode
function changeSubtitleTrack(value) {
    const session = activePlayback;
    if (!session) return;
    session.subtitleIndex = Number(value);
    if (session.burnedIn || isImageSubtitle(findStream(session, 'subtitle', session.subtitleIndex)?.Codec)) {
        clearTextSubtitle();
        session.tracksChanged = true;
        scheduleRestart(session, session.position());
    } else {
        showTextSubtitle(session);
    }
}

// CodecType comes lower case from the server: 'audio', 'subtitle' or 'video'
function isStreamType(s, type) {
    return (s.CodecType || '').toLowerCase() === type;
}

// The stream of this type with this index, if the session has one
function findStream(session, type, index) {
    return session.streams.find(s => s.Index === index && isStreamType(s, type));
}

// Picture subtitles are burned in by the transcode (the server's codec names)
function isImageSubtitle(codec) {
    const c = (codec || '').toLowerCase();
    return c.includes('pgs') || ['dvd_subtitle', 'dvdsub', 'vobsub', 'dvb_subtitle', 'dvbsub', 'xsub', 'sup'].includes(c);
}

// The text track of the subtitle that showTextSubtitle added, if any
function playerSubtitleTrack(video) {
    return video.querySelector('track[data-player-subtitle]')?.track || null;
}

// Disables every text track but the player's own subtitle. A track left from an earlier playback,
// or one that hls.js or the browser adds for captions in the stream, is not in the subtitle menu,
// so it would stay on screen while the menu says Off. No HLS manifest carries subtitle renditions
// today; if one ever does, exempt the picked rendition here
function disableOtherTextTracks(video) {
    const own = playerSubtitleTrack(video);
    for (const track of Array.from(video.textTracks || [])) {
        if (track !== own && track.mode !== 'disabled') track.mode = 'disabled';
    }
}

// Removes the text subtitle track that showTextSubtitle added, disabled first: a browser may keep
// showing the cues of a removed track that was showing
function clearTextSubtitle() {
    const video = $('video-player');
    video.querySelectorAll('track[data-player-subtitle]').forEach(track => {
        track.track.mode = 'disabled';
        track.remove();
    });
    disableOtherTextTracks(video);
}

// Adds the chosen text subtitle as WebVTT; without a label, hls.js leaves the track alone
function showTextSubtitle(session) {
    clearTextSubtitle();
    const stream = findStream(session, 'subtitle', session.subtitleIndex);
    if (!session.mediaSourceId || !stream || isImageSubtitle(stream.Codec)) return;
    const track = document.createElement('track');
    track.kind = 'subtitles';
    track.dataset.playerSubtitle = '1';
    // The route takes the start in ticks: the transcode's offset, or 0 for direct play
    track.src = API.base + '/Videos/' + session.itemId + '/' + encodeURIComponent(session.mediaSourceId) +
        '/Subtitles/' + stream.Index + '/' + Math.round(session.offset * 1e7) + '/Stream.vtt?api_key=' +
        encodeURIComponent(API.token);
    track.addEventListener('error', () => {
        if (activePlayback === session) $('player-subtitle').textContent = t('player.subtitles_error');
    });
    $('video-player').appendChild(track);
    track.track.mode = 'showing';
    disableOtherTextTracks($('video-player'));
}

// Language name in the page's language; und, mis, mul and zxx name no language
function trackLanguageName(code) {
    if (!code || ['und', 'mis', 'mul', 'zxx'].includes(code.toLowerCase())) return '';
    let name = code;
    try {
        name = new Intl.DisplayNames([document.documentElement.lang || 'en'], { type: 'language' }).of(code) || code;
    } catch (e) {
        // Not a code Intl knows: the code stays as it is
    }
    return name.charAt(0).toUpperCase() + name.slice(1);
}

// The title, then each tag the title does not already contain (Jellyfin's DisplayTitle)
function trackTitle(title, tags) {
    const parts = tags.filter(Boolean);
    if (!title) return parts.join(' - ');
    const extra = parts.filter(tag => !title.toLowerCase().includes(tag.toLowerCase()));
    return [title, ...extra].join(' - ');
}

// Audio menu label: language, profile or codec, channel layout or count, then the flags
function audioTrackLabel(s) {
    const codec = (s.Codec || '').toLowerCase();
    const codecNames = { ac3: 'Dolby Digital', eac3: 'Dolby Digital+', dca: 'DTS' };
    const profile = s.Profile || '';
    const layout = s.ChannelLayout ? s.ChannelLayout.charAt(0).toUpperCase() + s.ChannelLayout.slice(1) : '';
    const tags = [
        trackLanguageName(s.Language),
        (profile && profile.toLowerCase() !== 'lc') ? profile : (codecNames[codec] || codec.toUpperCase()),
        layout || (s.Channels ? t('player.track_channels', { count: s.Channels }) : ''),
        s.IsDefault ? t('player.track_default') : '',
        s.IsExternal ? t('player.track_external') : '',
    ];
    return trackTitle(s.Title, tags) || t('player.track_undefined');
}

// Subtitle menu label: language, default and forced flags, codec name, then external
function subtitleTrackLabel(s) {
    const codec = (s.Codec || '').toLowerCase();
    const codecNames = { dvb_subtitle: 'DVBSUB', dvb_teletext: 'DVBTXT', dvd_subtitle: 'DVDSUB', hdmv_pgs_subtitle: 'PGSSUB' };
    const tags = [
        trackLanguageName(s.Language) || t('player.track_undefined'),
        s.IsDefault ? t('player.track_default') : '',
        s.IsForced ? t('player.track_forced') : '',
        codecNames[codec] || codec.toUpperCase(),
        s.IsExternal ? t('player.track_external') : '',
    ];
    return trackTitle(s.Title, tags) || t('player.track_undefined');
}

// Jellyfin's Default mode: external first, then forced in the audio's language, forced, default
function defaultSubtitleIndex(subs, chosenAudio) {
    const audioLang = (chosenAudio?.Language || '').toLowerCase();
    let best = null;
    let bestKey = 0;
    for (const s of subs) {
        const sameLanguage = (s.Language || '').toLowerCase() === audioLang;
        const key = (s.IsExternal ? 8 : 0) + (s.IsForced && sameLanguage ? 4 : 0) + (s.IsForced ? 2 : 0) + (s.IsDefault ? 1 : 0);
        if (key > bestKey) {
            best = s;
            bestKey = key;
        }
    }
    return best ? best.Index : -1;
}

// Fills the audio and subtitle menus from the session; null hides both
function renderTrackMenus(session) {
    const audio = session ? session.streams.filter(s => isStreamType(s, 'audio')) : [];
    const subs = session ? session.streams.filter(s => isStreamType(s, 'subtitle')) : [];
    $('player-audio').replaceChildren(...audio.map(s => new Option(audioTrackLabel(s), s.Index)));
    $('player-subs').replaceChildren(new Option(t('player.subtitles_off'), -1), ...subs.map(s => new Option(subtitleTrackLabel(s), s.Index)));
    $('player-audio').value = String(session?.audioIndex ?? '');
    $('player-subs').value = String(session?.subtitleIndex ?? '');
    $('player-audio-wrap').hidden = audio.length < 2;
    $('player-subs-wrap').hidden = subs.length < 1;
}

// --- Video Player (Cinema Mode) ---
// carry: the previous episode's track languages (episodeTrackCarry), chosen again here where they exist.
// queued: the play queue (queue.js) started this item; any other play ends the queue.
async function playItem(itemId, streamType = 'Videos', title = '', { fromStart = false, carry = null, queued = false } = {}) {
    const video = $('video-player');
    const modal = $('player-modal');
    if (!video || !modal) return;
    const request = ++playRequest;
    if (!queued) clearQueue();

    // Report a session that is still playing as stopped before starting another
    stopPlayback();

    // Destroy any existing HLS instance
    if (hlsInstance) {
        hlsInstance.destroy();
        hlsInstance = null;
    }

    $('player-title').textContent = title || t('player.playback');
    $('player-subtitle').textContent = t('player.loading');
    // Clear the times and the bar so the previous item's values never show
    $('player-cur-time').textContent = '0:00';
    $('player-duration').textContent = '0:00';
    $('player-seek').value = 0;
    seekDragging = false;
    clearTextSubtitle();
    renderTrackMenus(null);
    resetEpisodeNav();
    hideSkipSegment();
    hideTrickplay();

    // Show modal immediately while loading
    if (!modal.open) {
        if (typeof modal.showModal === 'function') modal.showModal();
        else modal.setAttribute('open', '');
    }

    // Read the resume point only after the Stopped report, so it includes the position just saved
    await pendingStop;
    // The item gives the resume point and PlaybackInfo the tracks; a failed PlaybackInfo leaves no menus
    const [item, info] = await Promise.all([
        API.get('/Items/' + itemId).catch(e => {
            console.warn('Could not load item for resume point:', e);
            return null;
        }),
        streamType === 'Videos'
            ? API.getPlaybackInfo(itemId).catch(e => {
                console.warn('Could not load playback info:', e);
                return null;
            })
            : null,
    ]);
    if (!modal.open || request !== playRequest) return; // closed or replaced while loading
    if (!title && item?.Name) $('player-title').textContent = item.Name;

    const startTicks = (streamType !== 'Videos' || fromStart) ? 0 : resumePositionTicks(item);
    const runtimeTicks = item?.RunTimeTicks || 0;

    // Track choices as Jellyfin's defaults pick them; with no PlaybackInfo there are none
    const source = info?.MediaSources?.[0];
    const streams = source?.MediaStreams || [];
    const audioStreams = streams.filter(s => isStreamType(s, 'audio'));
    const subStreams = streams.filter(s => isStreamType(s, 'subtitle'));
    const defaultAudio = source?.DefaultAudioStreamIndex;
    const chosenAudio = audioStreams.find(s => s.Index === defaultAudio) || audioStreams.find(s => s.IsDefault) || audioStreams[0] || null;
    let audioIndex = null;
    let subtitleIndex = null;
    if (source) {
        audioIndex = chosenAudio ? chosenAudio.Index : null;
        const defaultSub = source.DefaultSubtitleStreamIndex;
        // The server's choice for this item (a remembered one, or off) wins over the client's default
        const serverSubtitle = defaultSub === -1 || subStreams.some(s => s.Index === defaultSub);
        subtitleIndex = serverSubtitle ? defaultSub : defaultSubtitleIndex(subStreams, chosenAudio);
        ({ audioIndex, subtitleIndex } = carriedTracks(streams, carry, audioIndex, subtitleIndex, serverSubtitle));
    }

    // The session owns this playback: stop() reports the position once and tears the player down
    let stopped = false;
    const session = {
        itemId,
        streamType,
        runtime: runtimeTicks > 0 ? runtimeTicks / 1e7 : null,
        offset: 0,
        startAt: startTicks / 1e7,
        isHls: false,
        playSessionId: null,
        mediaSourceId: source?.Id || null,
        streams,
        // The item's chapters (PageUp, PageDown), and its tile sets for the seek bar's thumbnails
        chapters: item?.Chapters || [],
        trickplay: null,
        segments: [],
        segmentsLoad: null,
        audioIndex,
        subtitleIndex,
        burnedIn: false,
        tracksChanged: false,
        loads: 0,
        failed: false,
        finished: false,
        pendingSeek: null,
        seekTimer: null,
        // Item time in seconds: the pending seek target, else where the stream is
        position() {
            if (this.pendingSeek !== null) return this.pendingSeek;
            const video = $('video-player');
            // Before metadata has loaded currentTime reads 0 even when resuming, so use the start position
            if (video.readyState < HTMLMediaElement.HAVE_METADATA) return this.startAt;
            return this.offset + video.currentTime;
        },
        // The item's runtime when known, else the stream's length
        total() {
            if (this.runtime) return this.runtime;
            const duration = $('video-player').duration;
            return Number.isFinite(duration) && duration > 0 ? this.offset + duration : 0;
        },
        // The track ids the playback reports carry; a null one is left out
        reportFields() {
            const fields = {};
            if (this.mediaSourceId !== null) fields.MediaSourceId = this.mediaSourceId;
            if (this.audioIndex !== null) fields.AudioStreamIndex = this.audioIndex;
            if (this.subtitleIndex !== null) fields.SubtitleStreamIndex = this.subtitleIndex;
            if (this.playSessionId) fields.PlaySessionId = this.playSessionId;
            return fields;
        },
        stop() {
            if (stopped) return pendingStop;
            stopped = true;
            clearTimeout(session.seekTimer);
            // Leaving from the next-episode card counts as watched to the end, credits included
            const ticks = Math.round((session.finished ? Math.max(session.position(), session.total()) : session.position()) * 1e7);
            if (playerReportInterval) {
                clearInterval(playerReportInterval);
                playerReportInterval = null;
            }
            if (activePlayback === session) activePlayback = null;
            // A failed playback leaves the watched state alone, as in Jellyfin
            const reported = API.reportStopped(itemId, ticks, { ...session.reportFields(), ...(session.failed ? { Failed: true } : {}) }).catch(e => console.warn('Reporting playback stop failed:', e));
            pendingStop = reported;
            // Once the Stopped report is in, refresh the open view so it shows the new resume point
            reported.then(() => {
                API.invalidateCache('/Items');
                API.invalidateCache('/Shows');
                if (currentPage === 'home') return renderHome();
                if (currentPage === 'detail') return renderDetail(currentDetailId);
            }).catch(e => console.warn('Refreshing views after playback failed:', e));
            video.pause();
            if (hlsInstance) {
                hlsInstance.destroy();
                hlsInstance = null;
            }
            if (session.isHls && session.playSessionId) API.stopActiveEncodings(session.playSessionId).catch(e => console.warn('Stopping the transcode failed:', e));
            video.removeAttribute('src');
            video.load();
            clearTextSubtitle();
            return reported;
        },
    };
    activePlayback = session;
    session.trickplay = trickplayInfo(item, session.mediaSourceId);
    renderTrackMenus(session);
    if (streamType === 'Videos') session.segmentsLoad = loadMediaSegments(session);
    loadEpisodeNav(session, item);
    updatePipButton(session);
    updateMediaSession(session, item, $('player-title').textContent);

    // Periodic progress reporting
    if (playerReportInterval) clearInterval(playerReportInterval);
    playerReportInterval = setInterval(() => {
        if (!video.paused && !video.ended) {
            const ticks = Math.round(session.position() * 1e7);
            API.reportProgress(itemId, ticks, false, session.reportFields()).catch(() => {});
        }
    }, 8000);

    // Video events for custom UI
    // A text track added during the playback (captions that hls.js finds in the stream) stays off
    video.textTracks.onaddtrack = () => disableOtherTextTracks(video);
    video.ontimeupdate = () => {
        if (activePlayback !== session) return;
        updatePlayerControls(session);
        checkUpNext(session);
        checkSkipSegment(session);
        updateMediaPosition(session);
    };

    video.onplay = () => { updatePlayIcon(true); setMediaPlaybackState('playing'); };
    video.onplaying = () => { session.failed = false; };
    video.onpause = () => { updatePlayIcon(false); setMediaPlaybackState('paused'); };
    video.onvolumechange = () => {
        updateVolumeIcon();
        const slider = $('player-vol');
        if (slider) slider.value = video.volume;
    };

    video.onended = () => {
        if (activePlayback !== session) return;
        session.stop();
        upNextAtEnd(session);
    };

    await openStream(session, startTicks);
    if (activePlayback === session) API.reportPlaying(itemId, startTicks, session.reportFields()).catch(() => {});
}

// --- Player bars auto-hide ---
// The top bar, the controls and the cursor fade out after `delay` ms without input while the video plays.
// Pointer, touch, key and focus input brings them back, as do pause, end, seek and buffering. They stay
// while paused, while a menu is open and while the pointer is over a bar; a tap toggles them.
function playerAutoHide(modal, video, delay = 3000) {
    const container = modal.querySelector('.jf-player-container');
    const bars = '.jf-player-topbar, .jf-player-controls';
    let timer = null;
    // The select whose native popup is open; no event says when one is dismissed, so the next
    // pointer move or tap outside it counts as closed
    let openMenu = null;
    let lastPointer = 'mouse';
    // Touch browsers keep :hover on the last tapped control, so a hovered bar holds them for a mouse only
    const barHovered = () => lastPointer !== 'touch'
        && !!container.querySelector('.jf-player-topbar:hover, .jf-player-controls:hover');
    const canHide = () => !video.paused && !video.ended && !video.seeking
        && video.readyState >= 3 // HAVE_FUTURE_DATA: not buffering
        && !openMenu
        && !barHovered()
        && !container.querySelector('[aria-expanded="true"], details[open]');
    const hide = () => {
        clearTimeout(timer);
        timer = null;
        if (canHide()) container.classList.add('jf-player-idle');
    };
    const show = () => {
        container.classList.remove('jf-player-idle');
        clearTimeout(timer);
        timer = setTimeout(hide, delay);
    };
    const onBar = e => !!e.target?.closest?.(bars);
    const isSelect = e => e.target?.tagName === 'SELECT';

    // A touch outside the bars waits for its click, which toggles them (tap)
    const pointer = e => {
        lastPointer = e.pointerType || 'mouse';
        if (openMenu && e.target !== openMenu) openMenu = null;
    };
    container.addEventListener('pointermove', e => {
        pointer(e);
        if (e.pointerType !== 'touch' || onBar(e)) show();
    });
    container.addEventListener('pointerdown', e => {
        pointer(e);
        if (isSelect(e)) openMenu = e.target;
        if (lastPointer !== 'touch' || onBar(e)) show();
    });
    container.addEventListener('pointerleave', e => { if (e.pointerType !== 'touch') hide(); });
    container.addEventListener('focusin', show);
    container.addEventListener('focusout', e => { if (isSelect(e)) openMenu = null; });
    container.addEventListener('change', e => { if (isSelect(e)) openMenu = null; show(); });
    // Runs after the player's shortcuts: a key they took (Space, Alt+Left/Right) opened no popup
    document.addEventListener('keydown', e => {
        if (!modal.open) return;
        if (isSelect(e)) {
            if (e.key === 'Escape' || e.key === 'Tab') openMenu = null;
            else if (!e.defaultPrevented && (e.key === ' ' || e.key === 'F4' || (e.altKey && e.key.startsWith('Arrow')))) openMenu = e.target;
        }
        show();
    });
    for (const type of ['play', 'playing', 'pause', 'ended', 'seeking', 'seeked', 'waiting']) {
        video.addEventListener(type, show);
    }

    return {
        show,
        hide,
        // A touch click toggles the bars instead of playing or pausing; returns whether it did
        tap(e) {
            if ((e.pointerType || lastPointer) !== 'touch') return false;
            if (container.classList.contains('jf-player-idle') || !canHide()) show();
            else hide();
            return true;
        },
        // The player closed: bars visible, no timer left for the next item
        reset() {
            clearTimeout(timer);
            timer = null;
            openMenu = null;
            container.classList.remove('jf-player-idle');
        },
    };
}

// The dialog closed: stop the session and the queue, reset the bars, and leave fullscreen, which the dialog
// may hold, and picture-in-picture
function onPlayerClosed() {
    stopPlayback();
    clearQueue();
    resetEpisodeNav();
    hideSkipSegment();
    hideTrickplay();
    clearMediaSession();
    playerBars?.reset();
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    if (document.pictureInPictureElement) document.exitPictureInPicture().catch(() => {});
}

// A click on the video plays or pauses; a tap shows or hides the bars
function playerAreaClick(e) {
    if (playerBars?.tap(e)) return;
    togglePlayPause();
}

function closePlayer() {
    const m = $('player-modal');
    if (m) {
        if (typeof m.close === 'function') m.close();
        else m.removeAttribute('open');
    }
}

// --- Next episode: the control bar's previous/next buttons and the next-episode card ---
// episodeNav holds the playing item's neighbours once loaded: the play queue's (queue.js) when one plays,
// else an episode's in its series. upNext is the card while it shows.
let episodeNav = null;
let upNext = null;
const UP_NEXT_SECONDS = 10;

// The episodes before and after this one in season order, across seasons. Specials (season 0) only
// count when this episode is one. Other versions of this episode, and multi-episode files that cover
// any of its numbers, are skipped: next has a higher number than this one ends on, prev a lower one.
function adjacentEpisodes(episodes, currentId) {
    const current = episodes.find(e => e.Id === currentId);
    if (!current) return { prev: null, next: null };
    const isSpecial = e => e.ParentIndexNumber === 0;
    const order = n => (n == null ? Number.MAX_SAFE_INTEGER : n);
    const season = e => order(e.ParentIndexNumber);
    const first = e => order(e.IndexNumber);
    const last = e => order(e.IndexNumberEnd ?? e.IndexNumber);
    const list = episodes
        .filter(e => isSpecial(current) || !isSpecial(e))
        .sort((a, b) => season(a) - season(b) || first(a) - first(b));
    const after = e => season(e) > season(current) || (season(e) === season(current) && first(e) > last(current));
    const before = e => season(e) < season(current) || (season(e) === season(current) && last(e) < first(current));
    return { prev: list.findLast(before) || null, next: list.find(after) || null };
}

// What the next episode keeps of this one's tracks: the audio and subtitle languages, or subtitles off
function episodeTrackCarry(session) {
    const audio = findStream(session, 'audio', session.audioIndex);
    const sub = findStream(session, 'subtitle', session.subtitleIndex);
    return {
        audioLanguage: (audio?.Language || '').toLowerCase(),
        subtitleLanguage: (sub?.Language || '').toLowerCase(),
        subtitleForced: !!sub?.IsForced,
        subtitlesOff: session.subtitleIndex === -1 && session.streams.some(s => isStreamType(s, 'subtitle')),
    };
}

// As in Jellyfin: a track in the last episode's language, else the default this episode picked. When
// the audio changes and no subtitle carries over, the default subtitle is picked again for that audio,
// unless the subtitle is the server's choice for this episode (subtitleFromServer), off included.
function carriedTracks(streams, carry, audioIndex, subtitleIndex, subtitleFromServer = false) {
    if (!carry) return { audioIndex, subtitleIndex };
    const inLanguage = (type, language) => (language
        ? streams.filter(s => isStreamType(s, type) && (s.Language || '').toLowerCase() === language)
        : []);
    const audio = inLanguage('audio', carry.audioLanguage);
    const pickedAudio = audio.find(s => s.Index === audioIndex) || audio.find(s => s.IsDefault) || audio[0];
    const audioChanged = !!pickedAudio && pickedAudio.Index !== audioIndex;
    if (pickedAudio) audioIndex = pickedAudio.Index;
    if (carry.subtitlesOff) return { audioIndex, subtitleIndex: -1 };
    const subs = inLanguage('subtitle', carry.subtitleLanguage);
    const sameKind = subs.filter(s => !!s.IsForced === carry.subtitleForced);
    const pickedSub = sameKind.find(s => s.Index === subtitleIndex) || sameKind.find(s => s.IsDefault) || sameKind[0] || subs[0];
    if (pickedSub) subtitleIndex = pickedSub.Index;
    else if (audioChanged && !subtitleFromServer) subtitleIndex = defaultSubtitleIndex(streams.filter(s => isStreamType(s, 'subtitle')), pickedAudio);
    return { audioIndex, subtitleIndex };
}

// Where the credits start, in seconds: an Outro media segment, else a credits chapter in the second half
function creditsStart(segments, chapters, runtime) {
    // Jellyfin names a segment's start StartTicks; older Rustyfin builds sent SegmentStartTicks
    const segmentStart = s => s.StartTicks ?? s.SegmentStartTicks ?? 0;
    const outro = (segments || []).find(s => s.Type === 'Outro' && segmentStart(s) > 0);
    if (outro) return segmentStart(outro) / 1e7;
    const start = c => (c.StartPositionTicks ?? c.StartTicks ?? 0) / 1e7;
    const credits = (chapters || []).find(c => /\b(credits|outro)\b/i.test(c.Name || '') && start(c) > (runtime || 0) / 2);
    return credits ? start(credits) : null;
}

// When the card shows, in seconds: at the credits, else 10 s before the end; null shows it at the end
function upNextStart(total, credits) {
    if (credits !== null && credits > 0 && (!total || credits < total)) return credits;
    return total >= 60 ? total - UP_NEXT_SECONDS : null;
}

// The item's media segments (intros, credits), loaded once per session
function loadMediaSegments(session) {
    return API.get(`/MediaSegments/${session.itemId}`).then(r => r?.Items || []).catch(() => []);
}

// Loads the playing item's neighbours, the user's autoplay switch and its credits: an episode's in its series,
// any item's in the play queue. Other items get none.
async function loadEpisodeNav(session, item) {
    const queue = queueNeighbours();
    const episode = item?.Type === 'Episode' && !!item.SeriesId;
    if (!queue && !episode) return;
    const video = session.streamType !== 'Audio';
    const [episodes, me, segments] = await Promise.all([
        queue ? null : API.get(`/Shows/${item.SeriesId}/Episodes`).then(r => r?.Items || []).catch(e => {
            console.warn('Could not load the episode list:', e);
            return [];
        }),
        API.getMe().catch(() => null),
        video ? (session.segmentsLoad ||= loadMediaSegments(session)) : [],
    ]);
    if (activePlayback !== session) return;
    const { prev, next } = queue || adjacentEpisodes(episodes, session.itemId);
    episodeNav = {
        session,
        prev,
        next,
        queue: !!queue,
        // Jellyfin's UserConfiguration.EnableNextEpisodeAutoPlay, on unless the user turned it off. As in
        // Jellyfin it holds back the next video only: music plays on.
        autoPlay: !video || me?.Configuration?.EnableNextEpisodeAutoPlay !== false,
        credits: video ? creditsStart(segments, item?.Chapters, session.runtime) : null,
        dismissed: false,
    };
    updateEpisodeButtons();
}

function resetEpisodeNav() {
    hideUpNext();
    episodeNav = null;
    updateEpisodeButtons();
}

// Previous and next show for episodes and queues only, and are disabled at their ends. The OS media
// controls (Media Session) get them too.
function updateEpisodeButtons() {
    for (const [id, episode, step] of [['player-btn-prev-episode', episodeNav?.prev, -1], ['player-btn-next-episode', episodeNav?.next, 1]]) {
        setMediaSessionHandler(step < 0 ? 'previoustrack' : 'nexttrack', episode ? () => playAdjacentEpisode(step) : null);
        const button = $(id);
        if (!button) continue;
        button.hidden = !episodeNav;
        button.disabled = !episode;
        const label = t(episodeNav?.queue ? (step < 0 ? 'player.previous_item' : 'player.next_item') : (step < 0 ? 'player.previous_episode' : 'player.next_episode'));
        button.title = label;
        button.setAttribute?.('aria-label', label);
    }
}

// Plays a neighbour from its start with this item's track languages; finished reports this one as watched.
// In a queue the neighbour is the queue's previous or next item.
function playEpisode(session, episode, { finished = false } = {}) {
    const carry = session.streamType !== 'Audio' ? episodeTrackCarry(session) : null;
    if (finished) session.finished = true;
    const nav = episodeNav;
    if (nav?.queue && nav.session === session) {
        playQueueStep(episode === nav.prev ? -1 : 1, carry);
        return;
    }
    playItem(episode.Id, 'Videos', episode.Name || '', { fromStart: true, carry });
}

function playAdjacentEpisode(step) {
    const nav = episodeNav;
    const episode = step < 0 ? nav?.prev : nav?.next;
    if (episode) playEpisode(nav.session, episode);
}

// Shows the card from its start time on; seeking back before that hides it again. Music in a queue plays on
// without a card.
function checkUpNext(session) {
    const nav = episodeNav;
    if (!nav || nav.session !== session || !nav.next || nav.dismissed || session.streamType === 'Audio') return;
    const start = upNextStart(session.total(), nav.credits);
    if (start === null) return;
    if (session.position() >= start) {
        if (!upNext) showUpNext(nav);
    } else if (upNext) {
        hideUpNext();
    }
}

// An episode that ended shows the card unless it was cancelled; a song in a queue goes straight on to the next
function upNextAtEnd(session) {
    const nav = episodeNav;
    if (!nav || nav.session !== session || !nav.next || nav.dismissed || upNext) return;
    if (session.streamType === 'Audio') playEpisode(session, nav.next, { finished: true });
    else showUpNext(nav);
}

function episodeNumberLabel(e) {
    if (e.ParentIndexNumber == null || e.IndexNumber == null) return '';
    const episode = e.IndexNumberEnd != null && e.IndexNumberEnd !== e.IndexNumber ? `${e.IndexNumber}-${e.IndexNumberEnd}` : e.IndexNumber;
    return t('player.episode_number', { season: e.ParentIndexNumber, episode });
}

// With autoplay on the card counts down and then plays; with it off it waits for Play now
function showUpNext(nav) {
    const card = $('player-up-next');
    if (!card) return;
    const episode = nav.next;
    const picture = cardImage(episode, true);
    const img = $('player-up-next-img');
    img.hidden = !picture;
    if (picture) img.src = API.imageUrl(picture.id, picture.type, { maxWidth: 320, tag: picture.tag });
    else img.removeAttribute('src');
    $('player-up-next-heading').textContent = t(episode.Type === 'Episode' ? 'player.next_episode' : 'player.up_next');
    $('player-up-next-number').textContent = episodeNumberLabel(episode);
    $('player-up-next-title').textContent = episode.Name || '';
    upNext = { nav, remaining: UP_NEXT_SECONDS, timer: nav.autoPlay ? setInterval(upNextTick, 1000) : null };
    $('player-up-next-countdown').hidden = !nav.autoPlay;
    renderUpNextCountdown();
    card.hidden = false;
    // Screen readers hear the card once; the ticking countdown is hidden from them
    const announce = episode.Type === 'Episode' ? 'player.up_next_announce' : 'player.up_next_item_announce';
    $('player-up-next-status').textContent = t(nav.autoPlay ? announce : `${announce}_manual`,
        { seconds: UP_NEXT_SECONDS, title: episode.Name || '' });
}

function renderUpNextCountdown() {
    if (upNext) $('player-up-next-countdown').textContent = t('player.up_next_countdown', { count: upNext.remaining });
}

function upNextTick() {
    if (!upNext) return;
    upNext.remaining -= 1;
    if (upNext.remaining <= 0) playUpNextNow();
    else renderUpNextCountdown();
}

function playUpNextNow() {
    const nav = upNext?.nav;
    hideUpNext();
    if (nav?.next) playEpisode(nav.session, nav.next, { finished: true });
}

// Cancel closes the card for this episode, which then ends as usual
function cancelUpNext() {
    if (upNext) upNext.nav.dismissed = true;
    hideUpNext();
}

function hideUpNext() {
    if (upNext) clearInterval(upNext.timer);
    upNext = null;
    const card = $('player-up-next');
    if (card) card.hidden = true;
    const status = $('player-up-next-status');
    if (status) status.textContent = '';
}

function togglePlayPause() {
    const v = $('video-player');
    if (!v) return;
    if (v.paused) v.play();
    else v.pause();
}

function updatePlayIcon(isPlaying) {
    const slot = $('player-play-icon');
    if (slot) {
        slot.innerHTML = isPlaying
            ? icon('Pause', { size: 20, class: 'w-5 h-5 fill-current' })
            : icon('Play', { size: 20, class: 'w-5 h-5 ml-0.5 fill-current' });
    }
}

function setVolume(val) {
    const v = $('video-player');
    if (v) {
        v.volume = parseFloat(val);
        updateVolumeIcon();
        // Sync slider with video volume
        const slider = $('player-vol');
        if (slider && parseFloat(slider.value) !== parseFloat(val)) {
            slider.value = val;
        }
    }
}

function toggleMute() {
    const v = $('video-player');
    if (v) {
        v.muted = !v.muted;
        updateVolumeIcon();
    }
}

function updateVolumeIcon() {
    const v = $('video-player');
    const slot = $('player-vol-icon');
    if (!v || !slot) return;
    if (v.muted || v.volume === 0) {
        // Muted: speaker with X
        slot.innerHTML = icon('VolumeX', { size: 16, class: 'w-4 h-4' });
    } else if (v.volume < 0.5) {
        // Low volume: speaker only
        slot.innerHTML = icon('Volume1', { size: 16, class: 'w-4 h-4' });
    } else {
        // High volume: speaker with waves
        slot.innerHTML = icon('Volume2', { size: 16, class: 'w-4 h-4' });
    }
}

// The speeds Jellyfin Web offers; < and > step through them
const PLAYBACK_RATES = [0.5, 0.75, 1, 1.25, 1.5, 1.75, 2, 2.5, 3, 3.5, 4];

// The default rate too, so the speed stays when the next item loads and the menu stays true
function setPlaybackRate(rate) {
    const v = $('video-player');
    if (!v) return;
    v.playbackRate = v.defaultPlaybackRate = parseFloat(rate);
    const menu = $('player-speed');
    if (menu) menu.value = String(v.playbackRate);
}

// One speed up (1) or down (-1), as Jellyfin Web steps: from a speed it doesn't list, up goes to the slowest
function stepPlaybackRate(step) {
    const v = $('video-player');
    if (!v) return;
    const index = PLAYBACK_RATES.indexOf(v.playbackRate);
    setPlaybackRate(PLAYBACK_RATES[Math.min(Math.max(index + step, 0), PLAYBACK_RATES.length - 1)]);
}

// Volume up or down by delta on Jellyfin's 0-100 scale, which is the cube root of the element's volume
function stepVolume(delta) {
    const v = $('video-player');
    if (!v) return;
    const level = Math.min(Math.round(Math.cbrt(v.volume) * 100), 100);
    setVolume(Math.pow(Math.min(Math.max(level + delta, 0), 100) / 100, 3));
}

// Seeks to a percentage of the item (the 0-9 keys, Home and End)
function seekPercent(percent) {
    if (activePlayback) seekTo(activePlayback, percent / 100 * activePlayback.total());
}

// Steps frames forward or back while paused, at the video's frame rate (24 when unknown), as Jellyfin Web does
function seekFrames(frames) {
    const session = activePlayback;
    const video = $('video-player');
    if (!session || !video?.paused) return;
    const stream = session.streams.find(s => isStreamType(s, 'video'));
    if (!stream) return;
    const fps = stream.ReferenceFrameRate || stream.RealFrameRate || stream.AverageFrameRate || 24;
    seekTo(session, session.position() + frames / fps);
}

// Chapter starts in seconds, in order
function chapterStarts(session) {
    return (session.chapters || []).map(c => (c.StartPositionTicks ?? c.StartTicks ?? 0) / 1e7).sort((a, b) => a - b);
}

// PageUp: the next chapter, else the next item
function nextChapter() {
    const session = activePlayback;
    if (!session) return;
    const position = session.position();
    const next = chapterStarts(session).find(start => start > position);
    if (next !== undefined) seekTo(session, next);
    else playAdjacentEpisode(1);
}

// PageDown: the chapter that started over 10 s ago, else the previous item; without one, the first chapter
function previousChapter() {
    const session = activePlayback;
    if (!session) return;
    let position = session.position() - 10;
    if (!episodeNav?.prev) position = Math.max(position, 0);
    const starts = chapterStarts(session).filter(start => start <= position);
    if (starts.length) seekTo(session, starts[starts.length - 1]);
    else playAdjacentEpisode(-1);
}

// The action of a player key, as Jellyfin Web's video player maps them, or null. Shift picks the other action
// of a key (< and >, previous and next item) and turns the rest off.
function playerKeyAction(e) {
    const key = e.key.length === 1 ? e.key.toLowerCase() : e.key;
    const digit = /^(?:Digit|Numpad)(\d)$/.exec(e.code || '');
    if (digit) return () => seekPercent(Number(digit[1]) * 10);
    // > and < from their own key (or Shift with . and , on US layouts) change the speed; . and , step frames
    if (e.key === '>' || (e.shiftKey && e.code === 'Period')) return () => stepPlaybackRate(1);
    if (e.key === '<' || (e.shiftKey && e.code === 'Comma')) return () => stepPlaybackRate(-1);
    if (e.key === '.') return () => seekFrames(1);
    if (e.key === ',') return () => seekFrames(-1);
    if (e.shiftKey) {
        if (key === 'n') return () => playAdjacentEpisode(1);
        if (key === 'p') return () => playAdjacentEpisode(-1);
        return null;
    }
    switch (key) {
    case 'k': return togglePlayPause;
    case 'ArrowUp': return () => stepVolume(2);
    case 'ArrowDown': return () => stepVolume(-2);
    case 'l': case 'ArrowRight': return () => skipTime(30);
    case 'j': case 'ArrowLeft': return () => skipTime(-10);
    case 'f': return toggleFullscreen;
    case 'm': return toggleMute;
    case 'Home': return () => seekPercent(0);
    case 'End': return () => seekPercent(100);
    case 'PageUp': return nextChapter;
    case 'PageDown': return previousChapter;
    default: return null;
    }
}

// --- Picture-in-picture: the standard API, or Safari's presentation mode ---
function pipSupported(video) {
    return !!(document.pictureInPictureEnabled && video?.requestPictureInPicture)
        || typeof video?.webkitSetPresentationMode === 'function';
}

// The button shows for videos where the browser can float them
function updatePipButton(session) {
    const button = $('player-btn-pip');
    if (button) button.hidden = !session || session.streamType === 'Audio' || !pipSupported($('video-player'));
}

function togglePictureInPicture() {
    const video = $('video-player');
    if (!video) return;
    if (document.pictureInPictureElement) {
        document.exitPictureInPicture().catch(() => {});
    } else if (document.pictureInPictureEnabled && video.requestPictureInPicture) {
        video.requestPictureInPicture().catch(e => console.warn('Picture-in-picture failed:', e));
    } else if (typeof video.webkitSetPresentationMode === 'function') {
        video.webkitSetPresentationMode(video.webkitPresentationMode === 'picture-in-picture' ? 'inline' : 'picture-in-picture');
    }
}

// --- Media Session: the OS media keys, lock screen and notification show the item and drive the player ---
const hasMediaSession = () => typeof navigator !== 'undefined' && 'mediaSession' in navigator;
// The artwork sizes Jellyfin Web offers, largest first (Firefox on Linux takes the first)
const ARTWORK_SIZES = [512, 384, 256, 192, 128, 96];

// Title, artist, album and artwork as Jellyfin Web shows them: an episode under its series, a song under its
// artists and album. The artwork is the item's picture, else its album's or series'.
function mediaSessionMetadata(item, title) {
    const artist = item?.Type === 'Episode'
        ? [item.SeriesName, episodeNumberLabel(item)].filter(Boolean).join(' · ')
        : (item?.Artists?.length ? item.Artists.join(', ') : item?.AlbumArtist || '');
    let picture = null;
    if (item?.ImageTags?.Primary) picture = { id: item.Id, tag: item.ImageTags.Primary };
    else if (item?.AlbumId && item.AlbumPrimaryImageTag) picture = { id: item.AlbumId, tag: item.AlbumPrimaryImageTag };
    else if (item?.SeriesId && item.SeriesPrimaryImageTag) picture = { id: item.SeriesId, tag: item.SeriesPrimaryImageTag };
    const artwork = picture
        ? ARTWORK_SIZES.map(size => ({ src: API.imageUrl(picture.id, 'Primary', { maxHeight: size, tag: picture.tag }), sizes: `${size}x${size}` }))
        : [];
    return { title: item?.Name || title || '', artist, album: item?.Album || '', artwork };
}

function updateMediaSession(session, item, title) {
    if (!hasMediaSession()) return;
    navigator.mediaSession.metadata = new MediaMetadata(mediaSessionMetadata(item, title));
    navigator.mediaSession.playbackState = 'paused';
}

function setMediaPlaybackState(state) {
    if (hasMediaSession()) navigator.mediaSession.playbackState = state;
}

// Where the playback is, for the lock screen's progress bar
function updateMediaPosition(session) {
    if (!hasMediaSession() || !navigator.mediaSession.setPositionState) return;
    const duration = session.total();
    if (!(duration > 0)) return;
    try {
        navigator.mediaSession.setPositionState({
            duration,
            position: Math.min(Math.max(session.position(), 0), duration),
            playbackRate: $('video-player').playbackRate || 1,
        });
    } catch {
        // A browser that rejects the state keeps the last one
    }
}

function clearMediaSession() {
    if (!hasMediaSession()) return;
    navigator.mediaSession.metadata = null;
    navigator.mediaSession.playbackState = 'none';
}

// A null handler removes the action, so the OS hides its button; browsers without an action throw
function setMediaSessionHandler(action, handler) {
    if (!hasMediaSession()) return;
    try {
        navigator.mediaSession.setActionHandler(action, handler);
    } catch {
        // Not supported here
    }
}

// The actions Jellyfin Web binds; previous and next follow the item's neighbours (updateEpisodeButtons)
function bindMediaSession() {
    const video = () => $('video-player');
    setMediaSessionHandler('play', () => video()?.play());
    setMediaSessionHandler('pause', () => video()?.pause());
    setMediaSessionHandler('stop', closePlayer);
    setMediaSessionHandler('seekbackward', d => skipTime(-(d?.seekOffset || 10)));
    setMediaSessionHandler('seekforward', d => skipTime(d?.seekOffset || 30));
    setMediaSessionHandler('seekto', d => { if (activePlayback && Number.isFinite(d?.seekTime)) seekTo(activePlayback, d.seekTime); });
}
bindMediaSession();

// --- Skip intro and outro: Jellyfin Web asks to skip each media segment of these types ---
const SKIP_SEGMENT_TYPES = ['Intro', 'Outro'];
// The button shows for 8 s from the segment's start, and while the bars show
const SKIP_PROMPT_MS = 8000;
// skipPrompt: the segment the button offers to skip, while it shows
let skipPrompt = null;

const segmentStart = s => (s.StartTicks ?? s.SegmentStartTicks ?? 0) / 1e7;
const segmentEnd = s => (s.EndTicks ?? s.SegmentEndTicks ?? 0) / 1e7;

// The segment to offer at seconds: an intro or outro of 3 s or more. An outro that runs to the end of an item
// with a next one is left to the next-episode card, as in Jellyfin.
function skippableSegment(segments, seconds, total, hasNext) {
    return (segments || []).find(s => SKIP_SEGMENT_TYPES.includes(s.Type)
        && segmentEnd(s) - segmentStart(s) >= 3
        && seconds >= segmentStart(s) && seconds < segmentEnd(s)
        && !(s.Type === 'Outro' && hasNext && total > 0 && segmentEnd(s) >= total - 1)) || null;
}

async function checkSkipSegment(session) {
    if (session.streamType === 'Audio' || !session.segmentsLoad) return;
    const segments = await session.segmentsLoad;
    if (activePlayback !== session) return;
    session.segments = segments;
    const hasNext = !!(episodeNav?.session === session && episodeNav.next);
    const segment = skippableSegment(segments, session.position(), session.total(), hasNext);
    if (segment === skipPrompt?.segment) return;
    hideSkipSegment();
    if (segment) showSkipSegment(segment);
}

function showSkipSegment(segment) {
    const button = $('player-skip-segment');
    if (!button) return;
    button.textContent = t('player.skip_segment', { type: t(`player.segment_${segment.Type.toLowerCase()}`) });
    button.classList.remove('jf-skip-expired');
    button.hidden = false;
    skipPrompt = { segment, timer: setTimeout(() => button.classList.add('jf-skip-expired'), SKIP_PROMPT_MS) };
}

function hideSkipSegment() {
    if (skipPrompt) clearTimeout(skipPrompt.timer);
    skipPrompt = null;
    const button = $('player-skip-segment');
    if (button) button.hidden = true;
}

// Seeks to the segment's end, unless less than a second of it is left
function skipSegment() {
    const session = activePlayback;
    const segment = skipPrompt?.segment;
    hideSkipSegment();
    if (session && segment && session.position() < segmentEnd(segment) - 1) seekTo(session, segmentEnd(segment));
}

// --- Trickplay: the thumbnail above the seek bar while it is dragged or hovered ---
// The item's tile set for this source: the widest up to a fifth of the screen, else the narrowest, as Jellyfin Web picks
function trickplayInfo(item, mediaSourceId) {
    const bySource = item?.Trickplay;
    if (!bySource) return null;
    const sets = Object.values(bySource[mediaSourceId] || bySource[item.Id] || Object.values(bySource)[0] || {});
    if (!sets.length) return null;
    const screenWidth = (globalThis.screen?.width || 1920) * (globalThis.devicePixelRatio || 1);
    const fits = sets.filter(s => s.Width <= screenWidth * 0.2).sort((a, b) => b.Width - a.Width);
    return fits[0] || [...sets].sort((a, b) => a.Width - b.Width)[0];
}

// The tile image and the thumbnail's place in it at seconds
function trickplayThumb(info, itemId, mediaSourceId, seconds) {
    if (!info?.Interval || !info.ThumbnailCount) return null;
    const index = Math.min(Math.max(Math.floor(seconds * 1000 / info.Interval), 0), info.ThumbnailCount - 1);
    const perTile = Math.max(info.TileWidth * info.TileHeight, 1);
    const inTile = index % perTile;
    const q = new URLSearchParams();
    if (mediaSourceId) q.set('MediaSourceId', mediaSourceId);
    if (API.token) q.set('api_key', API.token);
    return {
        url: `${API.base}/Videos/${itemId}/Trickplay/${info.Width}/${Math.floor(index / perTile)}.jpg?${q}`,
        x: (inTile % info.TileWidth) * info.Width,
        y: Math.floor(inTile / info.TileWidth) * info.Height,
        width: info.Width,
        height: info.Height,
    };
}

// Shows the thumbnail and time over percent of the bar
function showTrickplay(percent) {
    const session = activePlayback;
    const bubble = $('player-trickplay');
    if (!session || !bubble) return;
    const seconds = percent / 100 * session.total();
    const thumb = trickplayThumb(session.trickplay, session.itemId, session.mediaSourceId, seconds);
    const img = $('player-trickplay-img');
    img.hidden = !thumb;
    if (thumb) {
        img.style.width = `${thumb.width}px`;
        img.style.height = `${thumb.height}px`;
        img.style.backgroundImage = `url("${thumb.url}")`;
        img.style.backgroundPosition = `-${thumb.x}px -${thumb.y}px`;
    }
    $('player-trickplay-time').textContent = formatDurationSeconds(seconds);
    bubble.style.left = `${Math.min(Math.max(percent, 0), 100)}%`;
    bubble.hidden = false;
}

function hideTrickplay() {
    const bubble = $('player-trickplay');
    if (bubble) bubble.hidden = true;
}

// A mouse over the seek bar, or a finger on it, previews that point
function hoverSeek(e) {
    const bar = e.currentTarget;
    if (!activePlayback || !bar?.clientWidth) return;
    showTrickplay(Math.min(Math.max(e.offsetX / bar.clientWidth, 0), 1) * 100);
}

function toggleFullscreen() {
    const video = $('video-player');
    if (!video) return;
    if (!document.fullscreenElement) {
        // The player's container, so its bars and the next-episode card stay visible and the next episode
        // plays on in fullscreen (a dialog can't be the fullscreen element); where that fails (iOS), the video alone
        const videoOnly = () => {
            if (video.requestFullscreen) video.requestFullscreen().catch(() => {});
            else video.webkitEnterFullscreen?.();
        };
        const container = $('player-modal')?.querySelector('.jf-player-container');
        if (container && container.requestFullscreen) container.requestFullscreen().catch(videoOnly);
        else videoOnly();
    } else {
        document.exitFullscreen().catch(() => {});
    }
}

// Global Keyboard Shortcuts: Jellyfin Web's video player keys (playerKeyAction). Keys with Ctrl, Alt or Meta
// are left to the browser.
document.addEventListener('keydown', e => {
    if ($('player-modal')?.open) {
        if (e.key === 'Escape') { closePlayer(); return; }
        if (e.ctrlKey || e.altKey || e.metaKey) return;
        // Space on a button, menu or text field focused by keyboard works that control. After a click or
        // tap, and on the seek and volume sliders, Space still toggles play.
        const onControl = e.target instanceof Element && e.target.matches(':focus-visible')
            && e.target.closest('button, select, textarea, a[href], input:not([type="range"])');
        if (e.code === 'Space') {
            if (!onControl) { e.preventDefault(); togglePlayPause(); }
            return;
        }
        const action = playerKeyAction(e);
        if (action) { e.preventDefault(); action(); }
    }
});

export {
    resumePositionTicks, playWithResumeChoice, stopPlayback, seekVideo, skipTime, previewSeek, changeAudioTrack,
    changeSubtitleTrack, playItem, playerAutoHide, onPlayerClosed, playerAreaClick, closePlayer, playAdjacentEpisode,
    playUpNextNow, cancelUpNext, togglePlayPause, setVolume, toggleMute, setPlaybackRate, toggleFullscreen,
    togglePictureInPicture, skipSegment, hoverSeek, hideTrickplay,
};

// Shared state that inline handlers or other modules assign: reached through window
Object.defineProperties(window, {
    playerBars: { get: () => playerBars, set: value => { playerBars = value; } },
    seekDragging: { get: () => seekDragging, set: value => { seekDragging = value; } },
});
