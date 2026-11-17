// The library grid: paging, infinite scroll, the letter rail, BlurHash placeholders, the poster loader,
// and its sort and filters
import { $, icon, toast, escapeHtml, scanRing } from './util.js';
import { createCardElement, cardImage, bannerImage } from './cards.js';
import { renderGenreRows, genreNameOf } from './genres.js';
import { hashRoute } from './router.js';
import { musicTab, rememberMusicTab, musicTabBar, renderMusicTab } from './music.js';
import { KINDS, renderContainerPage, renderCollectionsTab } from './collections.js';
import { renderPhotoLibrary } from './photos.js';
import { renderSuggestions, renderUpcoming, renderStudios, studioNameOf } from './libtabs.js';
let currentLibraryId = null;
// The route shown (library/<this>): the library id, then a music library's tab and genre
let currentLibraryRoute = null;
// The open list's tab ('' for the library's own list, 'episodes', 'favorites'), and the genre it is
// limited to ({ id, name }, from the Genres tab), or null
let currentLibraryTab = '';
let currentLibraryGenre = null;
// What else the open list is limited to, or null: a network's shows ({ StudioIds, name }, from the
// Networks tab), or in a library of mixed content a folder's items ({ ParentId, name, crumbs })
let currentLibraryScope = null;


// --- BlurHash previews (https://blurha.sh): cards paint the server's ImageBlurHashes at once ---
const BLURHASH_DIGITS = '0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz#$%*+,-.:;=?@[]^_{|}~';

// RGBA pixels (a Uint8ClampedArray of w*h*4) for a BlurHash, or null when the hash is malformed
function decodeBlurhash(hash, w, h) {
    if (typeof hash !== 'string' || hash.length < 6) return null;
    const digits = [];
    for (const ch of hash) {
        const d = BLURHASH_DIGITS.indexOf(ch);
        if (d < 0) return null;
        digits.push(d);
    }
    const num = (a, b) => digits.slice(a, b).reduce((v, d) => v * 83 + d, 0);
    const nx = (digits[0] % 9) + 1, ny = Math.floor(digits[0] / 9) + 1;
    if (digits.length !== 4 + 2 * nx * ny) return null;
    const toLinear = v => { v /= 255; return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; };
    const toSrgb = v => {
        v = Math.min(1, Math.max(0, v));
        return Math.round((v <= 0.0031308 ? v * 12.92 : 1.055 * v ** (1 / 2.4) - 0.055) * 255);
    };
    const maxAc = (digits[1] + 1) / 166;
    const ac = q => Math.sign(q - 9) * ((q - 9) / 9) ** 2 * maxAc;
    const dc = num(2, 6);
    const colors = [[toLinear(dc >> 16), toLinear((dc >> 8) & 255), toLinear(dc & 255)]];
    for (let i = 1; i < nx * ny; i++) {
        const v = num(4 + 2 * i, 6 + 2 * i);
        colors.push([ac(Math.floor(v / 361)), ac(Math.floor(v / 19) % 19), ac(v % 19)]);
    }
    const cosX = [], cosY = [];
    for (let i = 0; i < nx; i++) for (let x = 0; x < w; x++) cosX[i * w + x] = Math.cos((Math.PI * x * i) / w);
    for (let j = 0; j < ny; j++) for (let y = 0; y < h; y++) cosY[j * h + y] = Math.cos((Math.PI * y * j) / h);
    const px = new Uint8ClampedArray(w * h * 4);
    for (let y = 0; y < h; y++) {
        for (let x = 0; x < w; x++) {
            let r = 0, g = 0, b = 0;
            for (let j = 0; j < ny; j++) {
                for (let i = 0; i < nx; i++) {
                    const basis = cosX[i * w + x] * cosY[j * h + y];
                    const c = colors[i + j * nx];
                    r += c[0] * basis; g += c[1] * basis; b += c[2] * basis;
                }
            }
            const o = 4 * (x + y * w);
            px[o] = toSrgb(r); px[o + 1] = toSrgb(g); px[o + 2] = toSrgb(b); px[o + 3] = 255;
        }
    }
    return px;
}


// A BlurHash's average colour as a CSS colour (its DC component: digits 2-5), or null
function blurhashAverage(hash) {
    if (typeof hash !== 'string' || hash.length < 6) return null;
    let v = 0;
    for (const ch of hash.slice(2, 6)) {
        const d = BLURHASH_DIGITS.indexOf(ch);
        if (d < 0) return null;
        v = v * 83 + d;
    }
    if (v > 0xffffff) return null;
    return `rgb(${v >> 16}, ${(v >> 8) & 255}, ${v & 255})`;
}

// The skeleton tone (input.css --jf-skeleton) and the least luma a cell's colour may have: a dark
// poster's average lifted toward it, so a cell is never near the page's black
const SKELETON_RGB = [63, 70, 81];
const TINT_MIN_LUMA = 64;

// The colour a cell shows for a picture not shown yet: its BlurHash average, mixed toward the
// skeleton tone just enough to reach TINT_MIN_LUMA when darker (so a darker poster never gets a
// lighter cell). Painted as a CSS background, it needs no canvas (which a browser's
// fingerprinting protection may block or blank). null without a BlurHash.
const lumaOf = c => 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
function blurhashTint(hash) {
    const avg = blurhashAverage(hash);
    if (!avg) return null;
    const c = avg.match(/\d+/g).map(Number);
    const luma = lumaOf(c);
    if (luma >= TINT_MIN_LUMA) return avg;
    const t = (TINT_MIN_LUMA - luma) / (lumaOf(SKELETON_RGB) - luma);
    return `rgb(${c.map((v, i) => Math.round(v + (SKELETON_RGB[i] - v) * t)).join(', ')})`;
}


// Each .jf-blurhash[data-blurhash] under root (root included) gets its BlurHash as a background
// image, once. Not a <canvas>: browsers composite every canvas in a layer of its own, and
// everything drawn above one then gets a layer too (two per card), which a fling can't draw in
// time. A tiny bitmap stretched by the browser looks the same and is painted with the card.
function paintBlurhashes(root) {
    const boxes = root.matches?.('.jf-blurhash[data-blurhash]') ? [root] : root.querySelectorAll?.('.jf-blurhash[data-blurhash]') || [];
    for (const box of boxes) {
        const w = Number(box.dataset.w) || 20;
        const h = Number(box.dataset.h) || 30;
        const url = blurhashImageUrl(box.dataset.blurhash, w, h);
        delete box.dataset.blurhash;
        if (url) box.style.backgroundImage = `url(${url})`;
        else box.remove();
    }
}

// A BlurHash as a w x h bitmap's data URL (an uncompressed 24-bit BMP, built from the decoded
// pixels: no canvas, so no read-back a browser's fingerprinting protection could alter), or null
// for a malformed hash. Kept by hash and size, as a grid shows the same pictures again.
const blurhashUrls = new Map();
function blurhashImageUrl(hash, w, h) {
    const key = `${hash}|${w}|${h}`;
    if (blurhashUrls.has(key)) return blurhashUrls.get(key);
    const px = decodeBlurhash(hash, w, h);
    if (!px) return null;
    const row = (w * 3 + 3) & ~3; // rows are padded to 4 bytes
    const size = 54 + row * h;
    const bmp = new Uint8Array(size);
    const view = new DataView(bmp.buffer);
    bmp[0] = 0x42; bmp[1] = 0x4d; // "BM"
    view.setUint32(2, size, true);
    view.setUint32(10, 54, true); // pixel data offset
    view.setUint32(14, 40, true); // BITMAPINFOHEADER
    view.setInt32(18, w, true);
    view.setInt32(22, h, true); // positive: rows from the bottom up
    view.setUint16(26, 1, true);
    view.setUint16(28, 24, true);
    for (let y = 0; y < h; y++) {
        const src = (h - 1 - y) * w * 4;
        const dst = 54 + y * row;
        for (let x = 0; x < w; x++) {
            bmp[dst + x * 3] = px[src + x * 4 + 2];
            bmp[dst + x * 3 + 1] = px[src + x * 4 + 1];
            bmp[dst + x * 3 + 2] = px[src + x * 4];
        }
    }
    let binary = '';
    for (let i = 0; i < size; i++) binary += String.fromCharCode(bmp[i]);
    const url = `data:image/bmp;base64,${btoa(binary)}`;
    if (blurhashUrls.size >= 5000) blurhashUrls.delete(blurhashUrls.keys().next().value);
    blurhashUrls.set(key, url);
    return url;
}

// --- Poster loader: card pictures (img[data-src]) start here, not by the browser's lazy loading ---
// Lazy loading starts a picture only once its card is close, keeps loading pictures already
// scrolled past, and gives them the same priority as the ones on screen. This loader looks
// further ahead in the scroll direction, starts what is on screen first, keeps a connection free
// for the page's own requests, and stops pictures that are no longer near.
const POSTER_MAX_LOADING = 5;
// How many loaded pictures' URLs the loader remembers (see posterLoader.seen)
const POSTER_SEEN_MAX = 20000;
// At most this many remembered pictures start outside the cap in one run (about a screen of cards)
const POSTER_GUESSES_MAX = 30;
// A picture that loads within this long after its start came from the browser's cache: no fade-in
const POSTER_CACHED_MS = 50;

// The key of the picture an <img> shows: item, type and tag (a new tag is another picture)
function posterKey(img) {
    const { imgId, imgType, imgTag } = img.dataset;
    return imgTag ? `${imgId}|${imgType}|${imgTag}` : img.dataset.src;
}

// The widths card pictures are asked for. A few fixed steps, so the server, the browser and the
// service worker share their copies between screens and card sizes.
const POSTER_WIDTHS = [160, 240, 320, 480, 640, 960];

// The width to ask for a picture shown cssWidth px wide at devicePixelRatio dpr: the smallest step
// that is at least 90% of the device pixels (a picture a little short of them looks the same).
function posterWidth(cssWidth, dpr) {
    const want = cssWidth * (dpr || 1) * 0.9;
    return POSTER_WIDTHS.find(w => w >= want) ?? POSTER_WIDTHS[POSTER_WIDTHS.length - 1];
}

// The band of the page, relative to the viewport's top, that pictures are loaded in: 2 screens
// ahead in the scroll direction, up to 4.5 when scrolling fast (the library grid's overscan of up
// to ten rows, LIBRARY_OVERSCAN), and half a screen behind. Pictures on screen still start first.
// velocity is in px per ms, negative when scrolling up.
function posterWindow(velocity, vh) {
    const screensPerSecond = (Math.abs(velocity) * 1000) / vh;
    const ahead = vh * Math.min(4.5, 2 + screensPerSecond / 2);
    const behind = vh / 2;
    return velocity < 0 ? { top: -ahead, bottom: vh + behind } : { top: -behind, bottom: vh + ahead };
}

// Which pictures to start and which to stop. entries are { top, bottom, left, right, loading }
// (rects relative to the viewport). Pictures in the window start closest first (on screen first,
// then by distance) while fewer than max load; loading ones that left the window stop. Returns
// { start, stop } as entry indexes.
function posterPlan(entries, vw, vh, win, max) {
    const distance = r => Math.max(0, r.top - vh, -r.bottom) + Math.max(0, r.left - vw, -r.right);
    const inWindow = r => r.bottom > win.top && r.top < win.bottom;
    const stop = [];
    let loading = 0;
    entries.forEach((r, i) => {
        if (!r.loading) return;
        if (inWindow(r)) loading++;
        else stop.push(i);
    });
    const start = entries
        .map((r, i) => ({ i, d: distance(r), ok: !r.loading && r.bottom > r.top && inWindow(r) }))
        .filter(c => c.ok)
        .sort((a, b) => a.d - b.d)
        .slice(0, Math.max(0, max - loading))
        .map(c => c.i);
    return { start, stop };
}

const posterLoader = {
    near: new Set(), // cards in or near the viewport whose picture has not loaded
    loading: new Set(),
    watched: new Set(), // every card the observer watches, so cards of a page left behind are dropped
    // Every picture that loaded in this page, by item, type and tag: { url, width }, the most
    // recently used last. One shown again (a recycled cell, a page visited again) is likely in the
    // browser's cache: when its card is in the window and the width is enough, it starts at once,
    // outside the queue (see run)
    seen: new Map(),
    seenToken: null,
    velocity: 0,
    lastY: window.scrollY,
    lastScroll: 0,
    queued: false,
    io: null,

    // Takes the cards with a picture to load under each of roots (roots included), all at once: no
    // layout is read here, and one run in the next frame measures them together, so a page of new
    // cards costs one layout, not one per card
    watch(roots) {
        const pictures = 'img[data-src], img[data-img-tag]';
        const imgs = [];
        for (const root of roots) {
            if (root.matches?.(pictures)) imgs.push(root);
            else if (root.querySelectorAll) imgs.push(...root.querySelectorAll(pictures));
        }
        if (!imgs.length) return;
        this.io ??= new IntersectionObserver(entries => {
            for (const e of entries) {
                // A card scrolled sideways out of a row is clipped by it, so it never intersects;
                // it counts as near while its row is, and starts after the cards in view
                const r = e.boundingClientRect;
                const inNearRow = e.target.parentElement?.classList.contains('jf-scroll-row')
                    && r.bottom > -3 * innerHeight && r.top < 4 * innerHeight;
                if (e.isIntersecting || inNearRow) this.near.add(e.target);
                else this.near.delete(e.target);
            }
            this.schedule();
        }, { rootMargin: '300% 0px' });
        for (const img of imgs) {
            if (img.getAttribute('src')) continue;
            const card = img.closest('.jf-card') || img;
            card._poster = img;
            this.watched.add(card);
            this.io.observe(card);
            // Counted as near until the observer says otherwise, so the next run sizes it up
            this.near.add(card);
        }
        this.schedule();
    },

    onScroll() {
        const now = performance.now();
        const y = window.scrollY;
        const dt = now - this.lastScroll;
        if (dt > 0 && dt < 200) this.velocity = 0.7 * this.velocity + 0.3 * ((y - this.lastY) / dt);
        else this.velocity = 0;
        this.lastY = y;
        this.lastScroll = now;
        this.schedule();
    },

    schedule() {
        if (this.queued) return;
        this.queued = true;
        requestAnimationFrame(() => { this.queued = false; this.run(); });
    },

    run() {
        if (performance.now() - this.lastScroll > 200) this.velocity = 0;
        const cards = [...this.near].filter(card => {
            if (card.isConnected && card._poster && !card._posterDone) return true;
            this.forget(card);
            return false;
        });
        for (const card of this.loading) if (!this.near.has(card)) this.stop(card);
        const all = cards.map(card => {
            const r = card.getBoundingClientRect();
            return { top: r.top, bottom: r.bottom, left: r.left, right: r.right, loading: this.loading.has(card) };
        });
        const win = posterWindow(this.velocity, innerHeight);
        const onScreen = r => r.bottom > 0 && r.top < innerHeight;
        // Pictures loaded before, in the window, at least as wide as the card needs now: started at
        // once, nearest first. One the browser has in memory is complete as soon as its src is set
        // (a hit: not counted); any other (most come from the disk cache in a few ms) stays an
        // ordinary load, counted against the cap and stopped when it leaves the window, and at most
        // POSTER_GUESSES_MAX such guesses start a run
        const seen = this.seenNow();
        const hits = new Set();
        let guesses = 0;
        const distance = r => Math.max(0, r.top - innerHeight, -r.bottom);
        const order = all.map((r, i) => i).sort((a, b) => distance(all[a]) - distance(all[b]));
        order.forEach(i => {
            const r = all[i];
            if (r.loading || r.bottom <= r.top || !(r.bottom > win.top && r.top < win.bottom) || guesses >= POSTER_GUESSES_MAX) return;
            const img = cards[i]._poster;
            const key = posterKey(img);
            const known = key && seen.get(key);
            if (!known) return;
            const fits = img.dataset.imgTag
                ? known.width >= posterWidth(r.right - r.left, devicePixelRatio)
                : known.url === img.dataset.src;
            if (!fits) return;
            seen.delete(key);
            seen.set(key, known);
            if (img.dataset.imgTag) {
                img.dataset.src = known.url;
                img.dataset.w = known.width;
            }
            this.start(cards[i], onScreen(r), r.right - r.left);
            if (img.complete) {
                hits.add(i);
            } else {
                guesses++;
                r.loading = true;
            }
        });
        const kept = all.map((r, i) => i).filter(i => !hits.has(i));
        const entries = kept.map(i => all[i]);
        const planCards = kept.map(i => cards[i]);
        const { start, stop } = posterPlan(entries, innerWidth, innerHeight, win, POSTER_MAX_LOADING);
        for (const i of stop) this.stop(planCards[i]);
        for (const i of start) this.start(planCards[i], onScreen(entries[i]), entries[i].right - entries[i].left);
        // A picture started ahead of the screen gets the high priority once its card is on it
        entries.forEach((r, i) => {
            if (r.loading && onScreen(r) && !stop.includes(i)) planCards[i]._poster.fetchPriority = 'high';
        });
    },

    // Drops the cards no longer in the page (a page left, a grid redrawn)
    sweep() {
        for (const card of this.watched) if (!card.isConnected) this.forget(card);
    },

    // Starts a picture. One without a fixed data-src gets the width step for its card's width.
    start(card, onScreen, cssWidth) {
        const img = card._poster;
        img.fetchPriority = onScreen ? 'high' : 'low';
        if (!img.dataset.src) {
            const { imgId, imgType, imgTag } = img.dataset;
            const width = posterWidth(cssWidth, devicePixelRatio);
            img.dataset.w = width;
            img.dataset.src = API.imageUrl(imgId, imgType, { maxWidth: width, tag: imgTag });
        }
        const started = performance.now();
        const done = e => {
            if (e?.type === 'load') {
                this.remember(img);
                gridDiagOnPoster?.(img);
                // Loaded within a few frames: from the browser's cache, shown without the fade-in
                // (the card's onload has just set its opacity; no frame has drawn it yet)
                if (img.complete !== false && performance.now() - started < POSTER_CACHED_MS) img.style.transition = 'none';
            }
            this.unlisten(card);
            card._posterDone = true;
            this.loading.delete(card);
            this.forget(card);
            this.schedule();
        };
        card._posterDoneListener = done;
        img.addEventListener('load', done);
        img.addEventListener('error', done);
        this.loading.add(card);
        img.src = img.dataset.src;
    },

    // Stops a picture's load (dropping src aborts its request); it starts again when it comes near
    stop(card) {
        this.loading.delete(card);
        this.unlisten(card);
        if (!card._posterDone) card._poster?.removeAttribute('src');
    },

    remember(img) {
        const key = posterKey(img);
        if (!key || !img.dataset.src) return;
        const seen = this.seenNow();
        seen.delete(key);
        if (seen.size >= POSTER_SEEN_MAX) seen.delete(seen.keys().next().value);
        seen.set(key, { url: img.dataset.src, width: Number(img.dataset.w) || 0 });
    },

    // The URLs carry the session's token: another session (sign-out, another user) starts empty
    seenNow() {
        if (this.seenToken !== API.token) {
            this.seen.clear();
            this.seenToken = API.token;
        }
        return this.seen;
    },

    unlisten(card) {
        const done = card._posterDoneListener;
        if (!done) return;
        card._poster.removeEventListener('load', done);
        card._poster.removeEventListener('error', done);
        card._posterDoneListener = null;
    },

    forget(card) {
        this.stop(card);
        this.near.delete(card);
        this.watched.delete(card);
        this.io?.unobserve(card);
    },
};
document.addEventListener('scroll', () => posterLoader.onScroll(), { capture: true, passive: true });
window.addEventListener('resize', () => posterLoader.schedule());
window.addEventListener('hashchange', () => posterLoader.sweep());

// Cards come from many renderers (grids, rows, innerHTML strings), so new ones are painted and
// their pictures queued as they are added to the pages (in <main>; the player, dialogs and the
// sidebar hold no cards and are not watched)
new MutationObserver(records => {
    const added = [];
    for (const record of records) {
        for (const node of record.addedNodes) {
            if (node.nodeType !== 1) continue;
            paintBlurhashes(node);
            added.push(node);
        }
    }
    posterLoader.watch(added);
}).observe(document.querySelector('main') || document.documentElement, { childList: true, subtree: true });

// A card's picture loaded: it fades in over its BlurHash, which stays under it. A failed one drops
// both the skeleton and the BlurHash and shows the type icon instead.
function posterLoaded(img) {
    img.style.opacity = '0.8';
    // visibility, not display: hiding it repaints, but lays nothing out again
    img.parentElement?.querySelectorAll('.jf-poster-skeleton').forEach(el => { el.style.visibility = 'hidden'; el.style.animation = 'none'; });
}
// A card the pointer goes over shows its picture at full brightness from then on, as before:
// one card at a time, a plain style change (no will-change, no layer)
document.addEventListener('pointerover', e => {
    const img = e.target.closest?.('.jf-card')?.querySelector('img.jf-thumb-img');
    if (img && img.style.opacity === '0.8') img.style.opacity = '1';
}, { passive: true });
function posterFailed(img) {
    if (!img.getAttribute('src')) return; // a load the poster loader stopped
    img.style.display = 'none';
    img.parentElement?.querySelectorAll('.jf-poster-skeleton, .jf-blurhash').forEach(el => { el.style.display = 'none'; });
    const ic = img.nextElementSibling;
    if (ic) ic.style.display = 'flex';
}

// Library list state: each load gets a new generation, and responses from older generations are dropped
let libraryGen = 0;
let libraryQuery = null;
let libraryTotal = null;
// Page requests in flight for the current load: page number -> AbortController
let libraryInFlight = new Map();
let libraryLastY = 0;
let libraryDir = 1;
let libraryUpdateQueued = false;
let libraryRequested = new Set();
// The lean index of the current load (every item's name, picture tag and BlurHash, by position):
// a placeholder near the viewport becomes a lite card from it at once, before its page arrives
let libraryIndex = null;
// The full items of the pages that arrived, by position; cells become cards from them near the screen
let libraryItems = [];
let letterClicks = 0;
// The letter rail: the anchor of the last jump ({ k, first }) and the pending frame of an update
let railAnchor = null;
let railFrame = 0;
const LIBRARY_PAGE_SIZE = 50;

// The sorts: an /Items SortBy with its default SortOrder, and the label key. The first is the default.
const LIBRARY_SORTS = [
    ['SortName,ProductionYear:Ascending', 'library.sort_name'],
    ['PremiereDate,SortName,ProductionYear:Descending', 'library.sort_release_date'],
    ['DateCreated,SortName,ProductionYear:Descending', 'library.sort_date_added'],
    ['CommunityRating,SortName,ProductionYear:Descending', 'library.sort_rating'],
    ['OfficialRating,SortName:Ascending', 'library.sort_parental_rating'],
    ['PlayCount,SortName:Descending', 'library.sort_play_count'],
    ['DatePlayed,SortName:Descending', 'library.sort_date_played'],
    ['Runtime,SortName:Ascending', 'library.sort_runtime'],
    ['DateLastContentAdded,SortName:Descending', 'library.sort_date_episode_added'],
    ['SeriesDatePlayed,SortName:Descending', 'library.sort_date_played'],
    ['SeriesSortName,ParentIndexNumber,IndexNumber:Ascending', 'library.sort_name'],
];
// Each list's sort menu, as Jellyfin Web's per library tab (its sort menu's entries Rustyfin can
// order by; Random is left out, since the grid pages through one order), by the first SortBy key
const LIBRARY_SORT_MENUS = {
    movies: ['SortName', 'CommunityRating', 'DateCreated', 'DatePlayed', 'OfficialRating', 'PlayCount', 'PremiereDate', 'Runtime'],
    series: ['SortName', 'CommunityRating', 'DateCreated', 'DateLastContentAdded', 'SeriesDatePlayed', 'OfficialRating', 'PremiereDate'],
    episodes: ['SeriesSortName', 'CommunityRating', 'DateCreated', 'PremiereDate', 'DatePlayed', 'OfficialRating', 'PlayCount', 'Runtime'],
    albums: ['SortName', 'CommunityRating', 'PremiereDate', 'DateCreated'],
    boxsets: ['SortName', 'CommunityRating', 'DateCreated', 'OfficialRating', 'PremiereDate'],
    playlists: ['SortName', 'DateCreated', 'DateLastContentAdded', 'PremiereDate', 'Runtime'],
    mixed: ['SortName', 'CommunityRating', 'DateCreated', 'DateLastContentAdded', 'DatePlayed', 'OfficialRating', 'PlayCount', 'PremiereDate', 'Runtime'],
};
// A list's sorts, in its menu's order: [[value, label key]]
function librarySorts(list = 'movies') {
    const keys = LIBRARY_SORT_MENUS[list] || LIBRARY_SORT_MENUS.mixed;
    return keys.map(key => LIBRARY_SORTS.find(([value]) => value.split(/[,:]/)[0] === key));
}
// The list a library tab shows, for its sorts, its item type and its saved preferences
function libraryList(contentType, tab = '') {
    if (tab === 'episodes') return 'episodes';
    return {
        Movies: 'movies', movies: 'movies', TvShows: 'series', tvshows: 'series', Music: 'albums', music: 'albums',
        BoxSets: 'boxsets', boxsets: 'boxsets', Playlists: 'playlists', playlists: 'playlists',
    }[contentType] || 'mixed';
}
// The view settings (Jellyfin Web's view menu): the card picture or a list, and the card size
const LIBRARY_LAYOUTS = [['poster', 'library.view_poster'], ['thumb', 'library.view_thumb'], ['banner', 'library.view_banner'], ['list', 'library.view_list']];
const LIBRARY_SIZES = [['small', 'library.size_small'], ['medium', 'library.size_medium'], ['large', 'library.size_large']];
// Saved in CustomPrefs under Jellyfin's ImageType and ViewMode names
const LAYOUT_IMAGE_TYPES = { poster: 'Primary', thumb: 'Thumb', banner: 'Banner' };
// Each filter's /Items parameter, with the delimiter Jellyfin Web joins its values with
const LIBRARY_FILTER_PARAMS = { Filters: ',', SeriesStatus: ',', Genres: '|', OfficialRatings: '|', Years: ',' };
// The DisplayPreferences client the sort and filters are saved under, one entry per library id
const LIBRARY_PREFS_CLIENT = 'rustyfin';
// The open library's sort and filters ({ sort, reversed, filters: { param: [values] } }), and each
// loaded library's, keyed by user and library
let libraryPrefs = libraryPrefsFromDisplay(null);
const libraryPrefsCache = new Map();
// The pending preference saves: each waits for the one before, so the server keeps the last
let librarySaves = Promise.resolve();

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
// The route is a library id, then for a music library its tab (library/<id>/songs) and, for a
// genre, its id (library/<id>/genres/<genre id>)
async function renderLibrary(route) {
    const [libId, tab, sub] = String(route || '').split('/');
    const session = API.token;
    currentLibraryId = libId;
    currentLibraryRoute = route;
    const page = $('page-library');
    // The Collections and Playlists views (library/collections, library/playlists): collections.js
    if (KINDS[libId]) {
        stopLibraryLoad();
        destroyInfiniteScroll();
        renderContainerPage(libId);
        return;
    }
    
    // Show skeleton grid immediately for instant visual feedback
    const lib = libraries.find(l => (l.id || l.Id) === libId);
    const libName = lib ? (lib.name || lib.Name) : t('common.library');
    const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';
    const libIcon = contentType === 'Movies' ? 'Film' : contentType === 'TvShows' ? 'Tv' : contentType === 'Music' ? 'Music' : 'Folder';
    const headerIcon = { Movies: 'Film', TvShows: 'Tv', Music: 'Music' }[lib?.content_type] || 'Folder';
    // A photo library browses its folders in place: library/<id>/folder/<folder id> (photos.js)
    if (contentType === 'Photos') {
        stopLibraryLoad();
        destroyInfiniteScroll();
        renderPhotoLibrary(lib, libId, tab === 'folder' && sub ? sub : '');
        return;
    }
    // A music library's tabs other than Albums are drawn by music.js; Albums is this grid
    const musicView = contentType === 'Music' ? musicTab(libId, tab, sub) : null;
    if (musicView && musicView !== 'albums') {
        stopLibraryLoad();
        // A scan refresh draws this tab again (see library-admin.js), not the one last shown
        currentLibraryRoute = musicView === 'genre' ? `${libId}/genres/${sub}` : `${libId}/${musicView}`;
        renderMusicTab(libId, libName, musicView, sub);
        return;
    }
    if (musicView) rememberMusicTab(libId, 'albums');
    // A movie or TV library's tabs (libraryTabBar): its list, Suggestions, Favorites, Upcoming and
    // Episodes, and Genres and Networks (a genre's row header, or a network's card, opens the list
    // of that genre or network: library/<id>/genres/<genre id>, library/<id>/studios/<studio id>)
    const videoTabs = contentType === 'Movies' || contentType === 'TvShows';
    const known = videoTabs && libraryTabs(contentType).some(([key]) => key === tab);
    const rows = {
        genres: !sub && renderGenreRows,
        suggestions: renderSuggestions,
        upcoming: renderUpcoming,
        studios: !sub && renderStudios,
        // A movie library's Collections tab: the collections holding its movies (collections.js)
        collections: (id, name, _, bar) => renderCollectionsTab(id, name, bar),
    }[known ? tab : ''];
    if (rows) {
        stopLibraryLoad();
        destroyInfiniteScroll();
        rows(libId, libName, contentType, libraryTabBar(libId, contentType, tab));
        return;
    }
    currentLibraryTab = known && (tab === 'episodes' || tab === 'favorites') ? tab : '';
    currentLibraryGenre = known && tab === 'genres' && sub ? { id: sub, name: '' } : null;
    // In a library of mixed content, a folder's card opens the folder here (library/<id>/folder/<folder id>)
    const mixed = !videoTabs && !musicView;
    currentLibraryScope = known && tab === 'studios' && sub ? { StudioIds: sub, name: '' }
        : mixed && tab === 'folder' && sub ? { ParentId: sub, name: '', crumbs: [] } : null;
    const list = libraryList(contentType, currentLibraryTab);
    
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
    // The saved sort and filters decide the first page and the index, so they come first; the
    // previous load stops meanwhile
    stopLibraryLoad();
    const ticket = libraryGen;
    const [prefs, genreName] = await Promise.all([
        loadLibraryPrefs(libId, list),
        currentLibraryGenre ? genreNameOf(libId, contentType, currentLibraryGenre.id) : '',
        loadLibraryScope(libId, currentLibraryScope),
    ]);
    if (ticket !== libraryGen || API.token !== session) return;
    libraryPrefs = prefs;
    if (currentLibraryGenre) currentLibraryGenre.name = genreName;
    const subset = currentLibraryGenre?.name || currentLibraryScope?.name;
    renderLetterRail(libId);

    const gen = startLibraryLoad(libId);
    loadLibraryIndex(gen);
    try {
        // Fetch first page with pagination + total count, and the values the filter menu offers
        const [{ items, total }, filterLists] = await Promise.all([
            fetchLibraryPage(0),
            API.getFilters(libId, libraryQuery.IncludeItemTypes).catch(() => ({}))
        ]);
        if (gen !== libraryGen || API.token !== session) return;
        const filterCount = libraryFilterCount(libraryPrefs.filters);

        page.innerHTML = `
        <div class="jf-page">
            <!-- Header bar with title, count, and actions -->
            <div class="flex flex-col md:flex-row md:items-center justify-between gap-4 mb-6 pb-4 border-b border-white/10">
                <div>
                    <div class="flex items-center gap-2">
                        <span class="inline-flex">${icon(headerIcon, { size: 28 })}</span>
                        <h1 class="text-2xl lg:text-3xl font-extrabold text-white tracking-tight">${escapeHtml(subset && !currentLibraryScope?.ParentId ? `${libName}: ${subset}` : libName)}</h1>
                        ${scanRing(libId, libName, 'w-5 h-5')}
                    </div>
                    <p id="lib-count-label" class="text-xs text-white/40 mt-1">${t('library.item_count', { count: total })}</p>
                </div>

                <div class="flex flex-wrap items-center gap-2">
                    <!-- Sort selector, with a toggle that reverses its order -->
                    <div class="flex items-center gap-1">
                        <select id="lib-sort" class="jf-input !py-1.5 text-xs">
                            ${librarySorts(list).map(([value, key]) => `<option value="${value}"${value === libraryPrefs.sort ? ' selected' : ''}>${t(key)}</option>`).join('')}
                        </select>
                        <button id="lib-sort-reverse" type="button" class="jf-btn-ghost self-stretch !py-1.5 !px-2 text-xs" aria-pressed="${libraryPrefs.reversed}" title="${escapeHtml(t('library.sort_reverse'))}" aria-label="${escapeHtml(t('library.sort_reverse'))}">
                            ${icon('ArrowDownUp', { size: 16 })}
                        </button>
                    </div>

                    <!-- View settings: poster, thumb, banner or list, and the card size -->
                    <button id="lib-view-button" type="button" popovertarget="lib-view-menu" class="jf-btn-ghost inline-flex items-center gap-1.5 !py-1.5 !px-3 text-xs">
                        ${icon(libraryPrefs.view.layout === 'list' ? 'List' : 'LayoutGrid', { size: 14 })}
                        ${t('library.view_settings')}
                    </button>
                    <div id="lib-view-menu" popover role="dialog" class="jf-filter-menu" aria-label="${escapeHtml(t('library.view_settings'))}">
                        ${libraryViewMenu(libraryPrefs.view)}
                    </div>

                    <!-- Filter menu, and a reset while any filter is on -->
                    <button id="lib-filter-button" type="button" popovertarget="lib-filter-menu" class="jf-btn-ghost inline-flex items-center gap-1.5 !py-1.5 !px-3 text-xs">
                        ${icon('Funnel', { size: 14 })}
                        ${t('library.filters')}
                        <span id="lib-filter-count" class="jf-filter-count"${filterCount ? '' : ' hidden'}>${filterCount}</span>
                    </button>
                    <div id="lib-filter-menu" popover role="dialog" class="jf-filter-menu" aria-label="${escapeHtml(t('library.filters'))}">
                        ${libraryFilterMenu(contentType, filterLists, libraryPrefs.filters)}
                    </div>
                    <button id="lib-filter-clear" type="button" class="jf-btn-ghost inline-flex items-center gap-1.5 !py-1.5 !px-3 text-xs" data-action="clear-library-filters" data-id="${escapeHtml(libId)}"${filterCount ? '' : ' hidden'}>
                        ${icon('FunnelX', { size: 14 })}
                        ${t('library.clear_filters')}
                    </button>

                    <!-- Plays the library's items in a random order -->
                    <button class="jf-btn-ghost inline-flex items-center gap-1.5 !py-1.5 !px-3 text-xs" data-action="shuffle" data-id="${escapeHtml(libId)}">
                        ${icon('Shuffle', { size: 14 })}
                        ${t('player.shuffle')}
                    </button>

                    <!-- Scan trigger -->
                    <button class="jf-btn-ghost inline-flex items-center gap-1.5 !py-1.5 !px-3 text-xs" data-action="scan-library-page" data-id="${escapeHtml(libId)}" data-name="${escapeHtml(libName)}">
                        ${icon('RefreshCw', { size: 14 })}
                        ${t('library.scan_button')}
                    </button>
                </div>
            </div>

            ${musicView ? musicTabBar(libId, 'albums') : ''}
            ${videoTabs ? libraryTabBar(libId, contentType, currentLibraryGenre ? 'genres' : currentLibraryScope ? 'studios' : currentLibraryTab || 'main') : ''}
            ${currentLibraryScope?.ParentId ? folderCrumbs(libId, libName, currentLibraryScope) : ''}

            <!-- Media Grid Container -->
            <div id="lib-grid-container" data-multiselect></div>
        </div>`;
        $('lib-sort')?.addEventListener('change', () => libraryControlsChanged(libId));
        $('lib-sort-reverse')?.addEventListener('click', e => {
            const btn = e.currentTarget;
            btn.setAttribute('aria-pressed', btn.getAttribute('aria-pressed') === 'true' ? 'false' : 'true');
            libraryControlsChanged(libId);
        });
        $('lib-filter-menu')?.addEventListener('change', () => libraryControlsChanged(libId));
        $('lib-view-menu')?.addEventListener('change', () => libraryControlsChanged(libId));
        fillLibraryGrid(items, total);
    } catch (e) {
        if (gen !== libraryGen || API.token !== session) return;
        page.classList.remove('jf-with-rail');
        page.innerHTML = `<div class="p-6 rounded-xl bg-red-500/10 text-red-400">${escapeHtml(e.message)}</div>`;
    }
}

// The library list's sort: the chosen sort, with its order flipped while the reverse toggle is pressed
function librarySort(prefs = libraryPrefs) {
    const [sortBy, sortOrder] = prefs.sort.split(':');
    if (!prefs.reversed) return { sortBy, sortOrder };
    return { sortBy, sortOrder: sortOrder === 'Ascending' ? 'Descending' : 'Ascending' };
}

// The /Items parameters of the chosen filters, each list joined as Jellyfin Web joins it
function libraryFilterQuery(filters) {
    const query = {};
    for (const [param, delimiter] of Object.entries(LIBRARY_FILTER_PARAMS)) {
        if (filters[param]?.length) query[param] = filters[param].join(delimiter);
    }
    return query;
}

function libraryFilterCount(filters) {
    return Object.keys(LIBRARY_FILTER_PARAMS).reduce((n, param) => n + (filters[param]?.length || 0), 0);
}

// Sort, filters and view from a DisplayPreferences answer: SortBy and SortOrder, and in CustomPrefs each
// filter's /Items value, and the view's ViewMode, ImageType and CardSize. Nothing saved, or a sort the
// list (libraryList) doesn't offer, gives the defaults.
function libraryPrefsFromDisplay(display, list = 'movies') {
    const sortBy = display?.SortBy || '';
    const sorts = librarySorts(list);
    const sort = sorts.find(([value]) => value.split(':')[0] === sortBy)?.[0];
    const custom = display?.CustomPrefs || {};
    const filters = {};
    for (const [param, delimiter] of Object.entries(LIBRARY_FILTER_PARAMS)) {
        filters[param] = typeof custom[param] === 'string' ? custom[param].split(delimiter).filter(Boolean) : [];
    }
    const layout = custom.ViewMode === 'list' ? 'list'
        : Object.keys(LAYOUT_IMAGE_TYPES).find(key => LAYOUT_IMAGE_TYPES[key] === custom.ImageType) || 'poster';
    const size = LIBRARY_SIZES.some(([key]) => key === custom.CardSize) ? custom.CardSize : 'medium';
    return {
        sort: sort || sorts[0][0],
        reversed: !!sort && !!display.SortOrder && display.SortOrder !== sort.split(':')[1],
        filters,
        view: { layout, size },
    };
}

// The view's CustomPrefs; the defaults (poster, medium) save nothing
function libraryViewCustom(view) {
    const custom = {};
    if (!view) return custom;
    if (view.layout === 'list') custom.ViewMode = 'list';
    else if (view.layout !== 'poster' && LAYOUT_IMAGE_TYPES[view.layout]) custom.ImageType = LAYOUT_IMAGE_TYPES[view.layout];
    if (view.size && view.size !== 'medium') custom.CardSize = view.size;
    return custom;
}

// The DisplayPreferences body that saves a library's sort, filters and view. The server replaces
// the stored CustomPrefs with these, as Jellyfin 12.4 does; they are the whole set this list's
// preferences hold, so an unset filter is simply left out.
function libraryPrefsToDisplay(libId, prefs) {
    const { sortBy, sortOrder } = librarySort(prefs);
    return {
        Id: libId,
        Client: LIBRARY_PREFS_CLIENT,
        SortBy: sortBy,
        SortOrder: sortOrder,
        CustomPrefs: { ...libraryFilterQuery(prefs.filters), ...libraryViewCustom(prefs.view) },
        RememberIndexing: false,
        RememberSorting: true,
        PrimaryImageHeight: 0,
        PrimaryImageWidth: 0,
        ShowBackdrop: true,
        ShowSidebar: false,
        ScrollDirection: 'Horizontal',
    };
}

// The DisplayPreferences id a list's preferences are saved under: the library's id, and for a TV
// library's Episodes tab its own (Jellyfin Web keeps view settings per tab and library too)
function libraryPrefsId(libId, list = currentLibraryTab) {
    return list === 'episodes' ? `${libId}-episodes` : libId;
}

// The user's saved sort, filters and view for a library list, read from the server once per session
async function loadLibraryPrefs(libId, list) {
    const id = libraryPrefsId(libId, list);
    const key = `${API.userId}:${id}`;
    if (!libraryPrefsCache.has(key)) {
        const display = await API.getDisplayPreferences(id, LIBRARY_PREFS_CLIENT).catch(() => null);
        libraryPrefsCache.set(key, libraryPrefsFromDisplay(display, list));
    }
    return libraryPrefsCache.get(key);
}

// Keeps the open library's sort, filters and view, for this session and on the server. Saves go one at a
// time in the order they were made, so a slow earlier save can't land after a later one.
function saveLibraryPrefs(libId) {
    const id = libraryPrefsId(libId);
    libraryPrefsCache.set(`${API.userId}:${id}`, libraryPrefs);
    const body = libraryPrefsToDisplay(id, libraryPrefs);
    librarySaves = librarySaves
        .then(() => API.saveDisplayPreferences(id, LIBRARY_PREFS_CLIENT, body))
        .catch(e => toast(t('library.prefs_save_failed', { message: e.message }), 'error'));
}

// The filter menu's groups, one checkbox per value; a saved value the server no longer lists stays offered
function libraryFilterMenu(contentType, lists, filters) {
    const box = (param, value, label) => `<label><input type="checkbox" data-param="${param}" value="${escapeHtml(value)}"${filters[param].includes(String(value)) ? ' checked' : ''}><span title="${escapeHtml(label)}">${escapeHtml(label)}</span></label>`;
    const group = (title, boxes) => boxes.length ? `<fieldset><legend>${escapeHtml(title)}</legend><div class="jf-filter-options">${boxes.join('')}</div></fieldset>` : '';
    const listed = (param, values) => [...new Set([...(values || []).map(String), ...filters[param]])];
    const choices = (param, pairs) => pairs.map(([value, key]) => box(param, value, t(key)));
    const values = (param, list) => list.map(value => box(param, value, value));
    return [
        group(t('library.filter_status'), choices('Filters', [
            ['IsUnplayed', 'library.filter_unplayed'], ['IsPlayed', 'library.filter_played'],
            ['IsResumable', 'library.filter_resumable'], ['IsFavorite', 'library.filter_favorites'],
        ])),
        contentType === 'TvShows' ? group(t('library.filter_series_status'), choices('SeriesStatus', [
            ['Continuing', 'library.filter_continuing'], ['Ended', 'library.filter_ended'],
        ])) : '',
        group(t('library.filter_genres'), values('Genres', listed('Genres', lists.Genres).sort((a, b) => a.localeCompare(b)))),
        group(t('library.filter_ratings'), values('OfficialRatings', listed('OfficialRatings', lists.OfficialRatings))),
        group(t('library.filter_years'), values('Years', listed('Years', lists.Years).sort((a, b) => b - a))),
    ].join('');
}

// The sort and filters the controls show
function libraryControlsPrefs() {
    const filters = Object.fromEntries(Object.keys(LIBRARY_FILTER_PARAMS).map(param => [param, []]));
    for (const box of document.querySelectorAll('#lib-filter-menu input:checked')) filters[box.dataset.param].push(box.value);
    const checked = name => document.querySelector(`#lib-view-menu input[name="${name}"]:checked`)?.value;
    return {
        sort: $('lib-sort')?.value || LIBRARY_SORTS[0][0],
        reversed: $('lib-sort-reverse')?.getAttribute('aria-pressed') === 'true',
        filters,
        view: { layout: checked('lib-layout') || 'poster', size: checked('lib-size') || 'medium' },
    };
}

// The view menu: the layout (poster, thumb, banner or list) and the card size, as radio groups
function libraryViewMenu(view) {
    const group = (title, name, options, value) => `<fieldset><legend>${escapeHtml(title)}</legend><div class="jf-filter-options">${options.map(([key, label]) => `<label><input type="radio" name="${name}" value="${key}"${key === value ? ' checked' : ''}><span>${escapeHtml(t(label))}</span></label>`).join('')}</div></fieldset>`;
    return group(t('library.view_layout'), 'lib-layout', LIBRARY_LAYOUTS, view.layout)
        + group(t('library.view_size'), 'lib-size', LIBRARY_SIZES, view.size);
}

// A movie or TV library's tabs in Jellyfin Web's order, [key, label key]: a movie library's
// Movies, Suggestions, Favorites, Collections and Genres; a TV library's Shows, Suggestions,
// Upcoming, Genres, Networks and Episodes. A tab's route is library/<id>/<key>, the first's the id.
function libraryTabs(contentType) {
    return contentType === 'TvShows' ? [
        ['main', 'library.tab_shows'], ['suggestions', 'library.tab_suggestions'], ['upcoming', 'library.tab_upcoming'],
        ['genres', 'library.tab_genres'], ['studios', 'library.tab_networks'], ['episodes', 'library.tab_episodes'],
    ] : [
        ['main', 'library.tab_movies'], ['suggestions', 'library.tab_suggestions'], ['favorites', 'library.tab_favorites'],
        ['collections', 'collections.collections'], ['genres', 'library.tab_genres'],
    ];
}

function libraryTabBar(libId, contentType, active) {
    return `
    <nav class="jf-tabs" role="tablist" aria-label="${escapeHtml(t('library.tabs'))}">
        ${libraryTabs(contentType).map(([key, label]) => `
        <button type="button" role="tab" class="jf-tab" aria-selected="${key === active}" data-action="navigate" data-page="library" data-id="${escapeHtml(key === 'main' ? libId : `${libId}/${key}`)}">${t(label)}</button>`).join('')}
    </nav>`;
}

// The card shape the open list's view draws: 'poster', 'backdrop' (thumb, and episodes' own
// pictures), 'banner' or 'list'
function libraryShape(prefs = libraryPrefs, list = currentLibraryTab) {
    const layout = prefs.view?.layout || 'poster';
    if (layout === 'thumb' || (layout === 'poster' && list === 'episodes')) return 'backdrop';
    return layout;
}

// createCardElement's options for the shape
function libraryCardOptions(shape = libraryShape()) {
    return { poster: { backdrop: false }, backdrop: { backdrop: true }, banner: { banner: true }, list: { list: true } }[shape];
}

// The grid's classes for the view: its shape and card size set the columns (input.css)
function libraryGridClass(prefs = libraryPrefs) {
    return `jf-lib-grid jf-vgrid jf-shape-${libraryShape(prefs)} jf-size-${prefs.view?.size || 'medium'}`;
}

// A cell whose item has not arrived, in the view's shape; the grid measures its cells by it
function libraryPlaceholder(shape = libraryShape()) {
    if (shape === 'list') {
        return '<div class="jf-card jf-card-pending jf-list-row"><div class="jf-list-thumb"></div><div class="p-3 min-w-0 flex-1"><p class="text-sm">&nbsp;</p><div class="text-xs">&nbsp;</div></div></div>';
    }
    const box = { backdrop: 'jf-poster-backdrop', banner: 'jf-poster-banner' }[shape] || 'jf-poster';
    return LIBRARY_PLACEHOLDER.replace('"jf-poster"', `"${box}"`);
}

// The picture a full card of `item` shows in the open view, for telling whether a lite card shows it
function libraryPictureTag(item, shape = libraryShape()) {
    if (shape === 'banner') return bannerImage(item)?.tag ?? null;
    if (shape === 'list' || shape === 'poster') return cardImage(item, false)?.tag ?? null;
    return cardImage(item, true)?.tag ?? null;
}

// The filter button's count and the reset button follow the chosen filters
function showLibraryFilterCount() {
    const count = libraryFilterCount(libraryPrefs.filters);
    const badge = $('lib-filter-count');
    if (badge) {
        badge.textContent = count;
        badge.hidden = !count;
    }
    const clear = $('lib-filter-clear');
    if (clear) clear.hidden = !count;
}

// A sort or filter change: kept for the library, then a new load
function libraryControlsChanged(libId) {
    libraryPrefs = libraryControlsPrefs();
    showLibraryFilterCount();
    saveLibraryPrefs(libId);
    reloadLibrary(libId);
}

// The reset in the header and in the empty state: every filter off, the sort kept
function clearLibraryFilters(libId) {
    for (const box of document.querySelectorAll('#lib-filter-menu input:checked')) box.checked = false;
    libraryControlsChanged(libId);
}

// The list with no items: with filters on, says none match and offers the reset; else the library is
// empty. Focusable, so focus can move to it when the list empties under the user.
function libraryEmptyHtml() {
    if (!libraryFilterCount(libraryPrefs.filters)) {
        return `<div class="text-center py-20 text-white/40" tabindex="-1">${t('library.empty')}</div>`;
    }
    return `<div class="flex flex-col items-center gap-4 py-20 text-white/40" tabindex="-1">${t('library.no_filter_results')}
        <button type="button" class="jf-btn-ghost inline-flex items-center gap-1.5 !py-1.5 !px-3 text-xs" data-action="clear-library-filters" data-id="${escapeHtml(currentLibraryId)}">${icon('FunnelX', { size: 14 })}${t('library.clear_filters')}</button></div>`;
}

// The /Items query for the library list; paging is added per request
function libraryListQuery(libId) {
    const lib = libraries.find(l => (l.id || l.Id) === libId);
    const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';

    const list = libraryList(contentType, currentLibraryTab);
    const includeItemTypes = { movies: 'Movie', series: 'Series', episodes: 'Episode', albums: 'MusicAlbum' }[list];

    const { sortBy, sortOrder } = librarySort();
    return {
        LibraryId: libId,
        IncludeItemTypes: includeItemTypes,
        SortBy: sortBy,
        SortOrder: sortOrder,
        Fields: 'PrimaryImageAspectRatio',
        ...libraryFilterQuery(libraryPrefs.filters),
        // A genre's list (the Genres tab's row headers)
        ...(currentLibraryGenre ? { GenreIds: currentLibraryGenre.id } : {}),
        // A network's shows, or a folder's own items
        ...(currentLibraryScope?.StudioIds ? { StudioIds: currentLibraryScope.StudioIds } : {}),
        ...(currentLibraryScope?.ParentId ? { ParentId: currentLibraryScope.ParentId } : {}),
        // The Favorites tab: the user's favourites, with the filters chosen as well
        ...(currentLibraryTab === 'favorites' ? { Filters: [libraryFilterQuery(libraryPrefs.filters).Filters, 'IsFavorite'].filter(Boolean).join(',') } : {}),
    };
}

// Gives the open network or folder its name, and a folder its way down from the library: the
// folders above it, top first (its ancestors up to the library's own folder)
async function loadLibraryScope(libId, scope) {
    if (scope?.StudioIds) scope.name = await studioNameOf(libId, scope.StudioIds);
    if (!scope?.ParentId) return;
    const [folder, ancestors] = await Promise.all([
        API.getItem(scope.ParentId).catch(() => null),
        API.get(`/Items/${scope.ParentId}/Ancestors`).catch(() => []),
    ]);
    scope.name = folder?.Name || '';
    const above = [];
    for (const a of Array.isArray(ancestors) ? ancestors : []) {
        if (a.Id === libId || a.Type === 'CollectionFolder' || a.Type === 'UserRootFolder') break;
        above.unshift({ id: a.Id, name: a.Name || '' });
    }
    scope.crumbs = above;
}

// A folder's breadcrumbs: the library, the folders above, then the folder itself (not a link)
function folderCrumbs(libId, libName, scope) {
    const link = (route, name) => `<button type="button" class="jf-crumb" data-action="navigate" data-page="library" data-id="${escapeHtml(route)}">${escapeHtml(name)}</button>`;
    const parts = [link(libId, libName), ...scope.crumbs.map(c => link(`${libId}/folder/${c.id}`, c.name)),
        `<span class="jf-crumb" aria-current="page">${escapeHtml(scope.name)}</span>`];
    return `<nav class="jf-crumbs" aria-label="${escapeHtml(t('library.folders'))}">${parts.join('<span aria-hidden="true">›</span>')}</nav>`;
}

// A library card; in a library of mixed content a folder's card opens the folder in place
function libraryCard(item) {
    const card = createCardElement(item, '', libraryCardOptions());
    const lib = libraries.find(l => (l.id || l.Id) === currentLibraryId);
    const contentType = lib?.content_type || lib?.Content_type || lib?.ContentType || '';
    if (card && (item.Type || item.type) === 'Folder' && libraryList(contentType) === 'mixed') {
        card.dataset.page = 'library';
        card.dataset.id = `${currentLibraryId}/folder/${item.Id || item.id}`;
    }
    return card;
}

// Starts a load: a new generation, with the previous load's requests dropped
function startLibraryLoad(libId) {
    stopLibraryLoad();
    libraryQuery = libraryListQuery(libId);
    libraryRequested = new Set([0]); // the first page is requested by the load itself
    libraryIndexHashRetries = 0;
    libraryIndexMissing = null;
    libraryIndexAt = null;
    return ++libraryGen;
}

// Ends the current load: its page and index requests are aborted and its items dropped. A new
// load starts with it, and leaving the library page calls it (see navigate)
function stopLibraryLoad() {
    // Anything of this load still running (its first page, its pages, its index) is stale now
    libraryGen++;
    libraryTotalSeq = librarySeq;
    clearTimeout(libraryIndexRefresh);
    for (const controller of libraryInFlight.values()) controller.abort();
    libraryInFlight = new Map();
    libraryIndexRequest?.abort();
    libraryIndexRequest = null;
    clearTimeout(librarySettle);
    libraryFlyingNow = false;
    libraryView?.resizeObserver.disconnect();
    libraryView = null;
    libraryTotal = null;
    libraryIndex = null;
    libraryItems = [];
    libraryRequested = new Set();
    railAnchor = null;
}

// One page of the current load, with the total record count
async function fetchLibraryPage(startIndex, signal) {
    const params = { ...libraryQuery, StartIndex: startIndex, Limit: LIBRARY_PAGE_SIZE };
    // A page the scroll position asks for can be dropped, so it goes past the shared GET cache
    const data = signal
        ? await API.get('/Items?' + new URLSearchParams(Object.entries(params).filter(([, v]) => v !== undefined && v !== null && v !== '')).toString(), { signal })
        : await API.getItems(params);
    const items = data.Items || data.items || [];
    return { items, total: data.TotalRecordCount ?? data.total_record_count ?? items.length };
}

// The index of the whole list: the list query with RustyfinCardIndex=true (a Rustyfin extension:
// per item only what a card shows, about 300 bytes, 50 of gzip; Fields given empty, EnableUserData
// and the image options go along so a server without the extension still answers lean), for
// every item (Limit past the server's default cap). Cards made from it get their watch state when
// their page arrives. Its request is dropped with the load. It goes out with the first page, not after it
// (one round trip less on a real network), so it asks for at least LIBRARY_INDEX_LIMIT items.
// An index whose pictures mostly have no BlurHash yet (a server just upgraded computes them in
// the background) is asked again, LIBRARY_INDEX_HASH_RETRY_MS later, a few times.
async function loadLibraryIndex(gen) {
    // Thumb and banner views draw other pictures than the Primary
    const imageTypes = libraryShape() === 'poster' || libraryShape() === 'list' ? 'Primary' : 'Primary,Thumb,Backdrop,Banner';
    const params = { ...libraryQuery, EnableUserData: false, EnableImageTypes: imageTypes, ImageTypeLimit: 1, Limit: Math.max(libraryTotal ?? 0, LIBRARY_INDEX_LIMIT) };
    const q = new URLSearchParams();
    for (const [k, v] of Object.entries(params)) {
        if (k !== 'Fields' && v !== undefined && v !== null && v !== '') q.set(k, v);
    }
    q.set('Fields', '');
    q.set('RustyfinCardIndex', 'true');
    libraryIndexRequest?.abort();
    const controller = new AbortController();
    libraryIndexRequest = controller;
    try {
        // Low priority: the first screens' pages and posters come first
        const asked = performance.now();
        const data = await API.get('/Items?' + q.toString(), { priority: 'low', signal: controller.signal });
        if (gen !== libraryGen) return;
        libraryIndexRequest = null;
        libraryIndexMs = performance.now() - asked;
        const fresh = data.Items || data.items || [];
        const hadIndex = !!libraryIndex;
        libraryIndex = fresh;
        libraryIndexAt = performance.now();
        // Skeletons take their item; on a later index, so do lite cards whose picture is not
        // shown yet and that now have a BlurHash. Not while flying: the settle makes the cells
        // then, and a whole flying band of cards at once would be a long task mid-fling.
        if (!libraryFlyingNow) libraryRefreshCells(cell => cell.hasAttribute('data-i')
            || (hadIndex && cell.firstElementChild?.dataset.lite && !cell.firstElementChild._posterDone
                && !cell.querySelector('.jf-blurhash') && libraryHasHash(fresh[cell.dataset.pos])));
        const pictured = fresh.filter(item => item.ImageTags?.Primary);
        const missing = pictured.filter(item => !libraryHasHash(item)).length;
        // Asked again only while the server is still adding BlurHashes: pictures it can't hash
        // would otherwise cost every retry on every visit
        const falling = libraryIndexMissing == null || missing < libraryIndexMissing;
        libraryIndexMissing = missing;
        if (pictured.length && missing / pictured.length > 0.05 && falling && libraryIndexHashRetries < LIBRARY_INDEX_HASH_RETRIES) {
            libraryIndexHashRetries++;
            clearTimeout(libraryIndexRefresh);
            libraryIndexRefresh = setTimeout(() => { if (gen === libraryGen) loadLibraryIndex(gen); }, LIBRARY_INDEX_HASH_RETRY_MS);
        }
    } catch (e) {
        if (libraryIndexRequest === controller) libraryIndexRequest = null;
        if (e.name !== 'AbortError') console.warn('Library index failed; pages still load', e);
    }
}

// Whether an item's Primary picture has a BlurHash
function libraryHasHash(item) {
    const tag = item?.ImageTags?.Primary;
    return !!(tag && item.ImageBlurHashes?.Primary?.[tag]);
}

// Whether a lite card must be drawn again for its full item: it shows another item (the list's
// order moved between the two requests), or the item has watch state the index leaves out. Most
// cards of a library have none, so their page changes nothing on screen.
function liteCardDiffers(card, item) {
    const data = item.UserData || {};
    return card.dataset.id !== String(item.Id || item.id)
        || Boolean(data.Played || data.IsFavorite || data.PlayedPercentage > 0);
}

// Gives a card the full item of the same picture from its page, keeping its picture and BlurHash
// elements (already loading or shown) so nothing flashes, and the card element itself (the
// poster loader tracks it). Only for the same item and picture; another one gets a new card.
function upgradeCard(card, item) {
    const full = libraryCard(item);
    const oldImg = card.querySelector('img.jf-thumb-img');
    const freshImg = full.querySelector('img.jf-thumb-img');
    if (oldImg && freshImg) {
        freshImg.replaceWith(oldImg);
        const oldHash = card.querySelector('.jf-blurhash');
        const freshHash = full.querySelector('.jf-blurhash');
        if (oldHash && freshHash) freshHash.replaceWith(oldHash);
        // A picture already shown keeps its skeleton hidden
        if (oldImg.style.opacity) full.querySelectorAll('.jf-poster-skeleton').forEach(el => { el.style.visibility = 'hidden'; });
    }
    card.replaceChildren(...full.childNodes);
    card.dataset.letter = railLetter(item);
    delete card.dataset.lite;
    delete card.dataset.stale;
}

// The picture tag a card shows, or null
const cardPictureTag = card => card.querySelector('img.jf-thumb-img')?.dataset.imgTag ?? null;

// --- The library grid is virtual: only the rows near the viewport are in the page ---
// #lib-grid keeps the whole list's height, and each rendered cell sits at its row and column, so
// a library of thousands costs the DOM and layout of a few screens. Each cell shows the full item
// of its page when that is here, else a lite card from the index, else a skeleton.
// libraryView: { grid, cols, gap, colWidth, cellHeight, pitch, height, cells: Map<index, element>,
// resizeObserver }
let libraryView = null;
let libraryIndexRequest = null;
// Where the list was when the page was left: { key, index }, for the way back to it, and whether
// the coming load was reached through history (only then is it restored)
let libraryMemory = null;
let libraryRestore = false;

const LIBRARY_PLACEHOLDER = '<div class="jf-card jf-card-pending"><div class="jf-poster"></div><div class="p-3"><p class="text-xs sm:text-sm">&nbsp;</p><div class="text-[11px] mt-0.5">&nbsp;</div></div></div>';

// How many library pages may load at once; the rest of the browser's connections stay free for
// the posters
const LIBRARY_PAGES_AT_ONCE = 3;

// Rows made beyond the screen (the overscan), so a fling lands on cells already made, with their
// items and pictures on the way: `behind` rows behind the scroll direction; ahead, `aheadMin`
// rows at rest, plus the rows the scroll covers in `leadMs` at its speed, up to `aheadMax`. A slow
// scroll keeps a few rows ahead; a fling gets up to ten.
const LIBRARY_OVERSCAN = { behind: 2, aheadMin: 6, aheadMax: 10, leadMs: 300 };

// Rows to make ahead at `speed` screens a second, for rows `pitch` px apart in a screen `vh` tall
function libraryRowsAhead(speed, pitch, vh) {
    const { aheadMin, aheadMax, leadMs } = LIBRARY_OVERSCAN;
    if (!(pitch > 0)) return aheadMin;
    const travelled = (Math.max(0, speed || 0) * vh * leadMs) / 1000;
    return Math.min(aheadMax, aheadMin + Math.ceil(travelled / pitch));
}

// The geometry of `total` cells in `cols` columns of a grid `width` wide with `gap` between
// cells, each `cellHeight` tall: the column width, the distance between two rows' tops (pitch)
// and the whole height
function libraryLayout(width, cols, gap, cellHeight, total) {
    const colWidth = Math.max(0, (width - (cols - 1) * gap) / cols);
    const pitch = cellHeight + gap;
    const rows = Math.ceil(total / cols);
    return { cols, gap, colWidth, cellHeight, pitch, height: rows ? rows * pitch - gap : 0 };
}

// Where cell `i` sits in the grid
function libraryCellBox(i, layout) {
    return { x: (i % layout.cols) * (layout.colWidth + layout.gap), y: Math.floor(i / layout.cols) * layout.pitch };
}

// The first and last grid cells in a band of the page. viewTop and viewBottom are relative to the
// grid's top; rowHeight is the distance between two rows' tops. null when the grid has no rows.
function libraryCellRange(viewTop, viewBottom, rowHeight, cols, total) {
    if (!(rowHeight > 0) || !(cols > 0) || !(total > 0)) return null;
    const firstRow = Math.max(0, Math.floor(viewTop / rowHeight));
    const lastRow = Math.max(firstRow, Math.floor(viewBottom / rowHeight));
    const first = Math.min(total - 1, firstRow * cols);
    return { first, last: Math.min(total - 1, Math.max(first, (lastRow + 1) * cols - 1)) };
}

// The pages to request for cells `range`, in order: the pages covering it, nearest its middle
// first (on a tie, the one ahead), then `ahead` pages further in the scroll direction (dir < 0:
// up), then one behind.
// Pages in `held` (requested already) are left out.
function libraryPagePlan(range, dir, pageSize, total, held, ahead = 2) {
    const pages = Math.ceil(total / pageSize);
    const first = Math.floor(range.first / pageSize);
    const last = Math.floor(range.last / pageSize);
    const mid = (first + last) / 2;
    const order = [];
    for (let p = first; p <= last; p++) order.push(p);
    order.sort((a, b) => Math.abs(a - mid) - Math.abs(b - mid) || (dir < 0 ? a - b : b - a));
    for (let k = 1; k <= ahead; k++) order.push(dir < 0 ? first - k : last + k);
    order.push(dir < 0 ? last + 1 : first - 1);
    return order.filter(p => p >= 0 && p < pages && !held.has(p));
}

// The grid's geometry now: its columns and gap from the stylesheet (they follow the width), and a
// cell's height from a skeleton cell at the column width (a card has the same height)
function libraryMeasure(grid, total) {
    const style = getComputedStyle(grid);
    const cols = parseInt(style.getPropertyValue('--jf-cols'), 10) || 1;
    const gap = parseFloat(style.columnGap) || 0;
    const width = grid.clientWidth;
    const probe = document.createElement('div');
    probe.innerHTML = libraryPlaceholder();
    const sample = probe.firstElementChild;
    sample.style.cssText = `visibility:hidden;width:${Math.max(0, (width - (cols - 1) * gap) / cols)}px`;
    grid.appendChild(sample);
    const cellHeight = sample.offsetHeight;
    sample.remove();
    return libraryLayout(width, cols, gap, cellHeight, total);
}

// Gives the grid the list's height (`total` items in the layout `v`)
function librarySize(v, total) {
    v.grid.style.height = `${v.height}px`;
}

// A new element for cell `i`: the full card, a lite card or a skeleton, placed at its row and
// column, announced as item i + 1 of the list's total. While the page flies (`tint`), a cell
// with an item is a skeleton in its picture's BlurHash average colour instead: one element, no
// picture to load; it becomes its card when the scroll settles.
function libraryCellElement(i, tint = false) {
    const v = libraryView;
    const full = libraryItems[i];
    const item = full || libraryIndex?.[i];
    // The positioned cell is the list item; the card in it is the link that opens the item
    const cell = document.createElement('div');
    cell.className = 'jf-vcell';
    let card;
    if (item && !tint) {
        card = libraryCard(item);
        card.dataset.letter = railLetter(item);
        if (!full) card.dataset.lite = '1';
        card.tabIndex = 0;
        card.setAttribute('role', 'link');
    } else {
        const probe = document.createElement('div');
        probe.innerHTML = libraryPlaceholder();
        card = probe.firstElementChild;
        cell.dataset.i = i;
        const tag = item?.ImageTags?.Primary;
        const color = tag ? blurhashTint(item.ImageBlurHashes?.Primary?.[tag]) : null;
        if (color) {
            cell.dataset.tint = '1';
            card.firstElementChild.style.backgroundColor = color;
        }
    }
    cell.appendChild(card);
    cell.dataset.pos = i;
    cell.setAttribute('role', 'listitem');
    cell.setAttribute('aria-posinset', String(i + 1));
    cell.setAttribute('aria-setsize', String(libraryTotal));
    libraryPlace(cell, i, v);
    return cell;
}

function libraryPlace(cell, i, v) {
    const box = libraryCellBox(i, v);
    cell.style.left = `${box.x}px`;
    cell.style.top = `${box.y}px`;
    cell.style.width = `${v.colWidth}px`;
    cell.style.height = `${v.cellHeight}px`;
}

// Puts exactly the cells `first`..`last` in the grid (and the focused cell, wherever it is),
// keeping those already there and in index order, so Tab follows the list. A cell that leaves is
// dropped from the poster loader too. New cells are tinted skeletons when `tint` (see
// libraryCellElement).
// tint: whether new cells are colour cells, or a function of the cell's index that says so
function libraryRenderRange(first, last, tint = false) {
    const v = libraryView;
    if (!v) return;
    const focused = document.activeElement?.closest?.('[data-pos]');
    const keep = focused && v.cells.get(Number(focused.dataset.pos)) === focused ? Number(focused.dataset.pos) : -1;
    for (const [i, cell] of v.cells) {
        if ((i < first || i > last) && i !== keep) {
            cell.remove();
            v.cells.delete(i);
            posterLoader.forget(cell.firstElementChild);
        }
    }
    let next = keep > last ? v.cells.get(keep) : null;
    for (let i = last; i >= first; i--) {
        let cell = v.cells.get(i);
        if (!cell) {
            cell = libraryCellElement(i, typeof tint === 'function' ? tint(i) : tint);
            v.cells.set(i, cell);
            v.grid.insertBefore(cell, next);
        }
        next = cell;
    }
}

// Draws again the rendered cells `test` picks (skeletons when the index arrives), keeping focus
function libraryRefreshCells(test) {
    const v = libraryView;
    if (!v) return;
    for (const [i, cell] of v.cells) if (test(cell, i)) libraryReplaceCell(i, cell);
}

function libraryReplaceCell(i, cell) {
    const v = libraryView;
    const fresh = libraryCellElement(i);
    const hadFocus = cell.contains(document.activeElement);
    cell.replaceWith(fresh);
    v.cells.set(i, fresh);
    posterLoader.forget(cell.firstElementChild);
    if (hadFocus) fresh.firstElementChild.focus({ preventScroll: true });
}

// Cell `i`'s page arrived: a skeleton becomes the full card; a lite card gets the watch state its
// index lacked (in place, same item and picture), or a new card when it showed another item or
// picture (a card is never reused for another picture, so its loader state can't go stale)
function libraryFillCell(i) {
    const cell = libraryView?.cells.get(i);
    const item = libraryItems[i];
    if (!cell || !item) return;
    const card = cell.firstElementChild;
    const same = card.dataset.id === String(item.Id || item.id)
        && cardPictureTag(card) === libraryPictureTag(item);
    if (cell.hasAttribute('data-i')) {
        // Mid-flight it stays a (tinted) skeleton; the settle turns it into its card
        if (!libraryFlying()) libraryReplaceCell(i, cell);
    } else if (!same) {
        // Another item at this position (the list moved: items added or removed meanwhile), or
        // the item got another picture: a new card
        libraryReplaceCell(i, cell);
    } else if (card.dataset.stale) {
        // Its page came again after a library change: the same item and picture, perhaps new text
        upgradeCard(card, item);
    } else if (card.dataset.lite) {
        if (liteCardDiffers(card, item)) upgradeCard(card, item);
        else delete card.dataset.lite;
    }
}

// The library changed on the server (a scan or refresh ended) while its list is open: the list is
// brought up to date in place. The screen's pages are asked again (a changed total is taken in
// place, see libraryChanged, and cards whose position or picture changed are replaced as their
// page comes); the cards on screen stay meanwhile, and the index is asked again once things
// settle. Returns false when no list is open.
function libraryRefreshInPlace() {

    if (!libraryView || libraryTotal == null) return false;
    for (const controller of libraryInFlight.values()) controller.abort();
    libraryInFlight = new Map();
    libraryRequested = new Set();
    // The cards on screen are drawn again from their pages, text included (a renamed title)
    for (const cell of libraryView.cells.values()) cell.firstElementChild.dataset.stale = '1';
    libraryUpdate(0);
    clearTimeout(libraryIndexRefresh);
    const gen = libraryGen;
    libraryIndexRefresh = setTimeout(() => {
        if (gen === libraryGen) loadLibraryIndex(gen);
    }, LIBRARY_INDEX_SETTLE_MS);
    return true;
}

// The grid's top in page coordinates (the header above it can change height)
function libraryGridTop(v) {
    return v.grid.getBoundingClientRect().top + window.scrollY;
}

// Renders the rows around the scroll position and requests their pages: the rows on screen, two
// screens ahead in the scroll direction (the poster loader's lookahead) and one behind; the pages
// covering the screen first, then their neighbours in the scroll direction, a few at a time.
// Requests far from a new landing are dropped. Runs on every scroll event (it comes once a frame,
// before the frame paints, so a fling's landing frame is already filled), on resize and when a
// page or the index arrives.
// While the page flies (faster than LIBRARY_FLYING_SPEED screens a second over consecutive
// events), only the screen's rows are rendered, as tinted skeletons: the rows pass in a few
// frames. When it settles (slower events, scrollend, or 150 ms without an event), the cells
// become cards. Calls that are not scroll events inherit the last event's speed for 150 ms.
function libraryUpdate(speed = null) {
    const v = libraryView;
    if (!v || libraryTotal == null) return;
    const recent = performance.now() - libraryLastScroll < 150;
    const wasFlying = libraryFlyingNow;
    const flying = speed == null ? libraryFlying() : speed > libraryFlyingLimit();
    libraryFlyingNow = flying;
    const now = speed ?? (recent ? libraryLastSpeed : 0);
    const gen = libraryGen;
    const viewTop = window.scrollY - libraryGridTop(v);
    const screen = libraryCellRange(viewTop, viewTop + innerHeight, v.pitch, v.cols, libraryTotal);
    if (!screen) return;
    // The overscan: rows behind and ahead of the screen in the scroll direction
    const ahead = libraryRowsAhead(now, v.pitch, innerHeight) * v.pitch;
    const behind = LIBRARY_OVERSCAN.behind * v.pitch;
    const band = libraryDir < 0
        ? libraryCellRange(viewTop - ahead, viewTop + innerHeight + behind, v.pitch, v.cols, libraryTotal)
        : libraryCellRange(viewTop - behind, viewTop + innerHeight + ahead, v.pitch, v.cols, libraryTotal);
    const onScreen = i => i >= screen.first && i <= screen.last;
    const offScreen = i => !onScreen(i);
    // Colour cells and skeletons whose item is here now
    const fillable = cell => cell.dataset.tint === '1' || (cell.hasAttribute('data-i') && !!(libraryItems[cell.dataset.pos] || libraryIndex?.[cell.dataset.pos]));
    if (flying) {
        // In flight, the band is colour cells (one element each) and they become cards on settling.
        // Slowing down (under the flying speed, still flying by the hysteresis), the screen's rows
        // become cards already, so the landing's pictures start as soon as they would without it.
        // (Making the resting overscan cards too cost 480-600 ms of main thread per flick.)
        const slowing = now <= LIBRARY_FLYING_SPEED;
        libraryRenderRange(band.first, band.last, slowing ? offScreen : true);
        if (slowing) libraryRefreshCells((cell, i) => onScreen(i) && fillable(cell));
        clearTimeout(librarySettle);
        librarySettle = setTimeout(() => libraryUpdate(0), 150);
    } else if (wasFlying) {
        // Settling: the screen's rows first (their pictures start in this frame), the overscan in
        // the next frame
        libraryRenderRange(band.first, band.last, offScreen);
        libraryRefreshCells((cell, i) => onScreen(i) && fillable(cell));
        requestAnimationFrame(() => { if (libraryView === v && !libraryFlyingNow) libraryRefreshCells(fillable); });
    } else {
        libraryRenderRange(band.first, band.last);
        libraryRefreshCells(fillable);
    }
    // The band's pages are asked for; requests further than the band (and two pages) are dropped
    const pagesAhead = Math.max(2, Math.ceil((libraryDir < 0 ? screen.first - band.first : band.last - screen.last) / LIBRARY_PAGE_SIZE));
    const firstPage = Math.floor(screen.first / LIBRARY_PAGE_SIZE);
    const lastPage = Math.floor(screen.last / LIBRARY_PAGE_SIZE);
    const keepBefore = libraryDir < 0 ? pagesAhead : 2;
    const keepAfter = libraryDir < 0 ? 2 : pagesAhead;
    for (const [page, controller] of libraryInFlight) {
        if (page < firstPage - keepBefore || page > lastPage + keepAfter) {
            controller.abort();
            libraryInFlight.delete(page);
            libraryRequested.delete(page);
        }
    }
    for (const page of libraryPagePlan(screen, libraryDir, LIBRARY_PAGE_SIZE, libraryTotal, libraryRequested, pagesAhead)) {
        if (libraryInFlight.size >= LIBRARY_PAGES_AT_ONCE) break;
        libraryRequested.add(page);
        loadLibraryPage(gen, page);
    }
}

function scheduleLibraryUpdate() {
    if (libraryUpdateQueued) return;
    libraryUpdateQueued = true;
    requestAnimationFrame(() => { libraryUpdateQueued = false; libraryUpdate(); });
}

// Above this speed (screens a second) the page is flying
const LIBRARY_FLYING_SPEED = 8;
let librarySettle = null;
let libraryLastScroll = 0;
let libraryLastSpeed = 0;

// Whether the page is flying now (the last scroll event was fast and recent)
function libraryFlying() {
    return libraryLastSpeed > libraryFlyingLimit() && performance.now() - libraryLastScroll < 150;
}

// The speed above which the page flies: once flying, it keeps flying down to three quarters of
// LIBRARY_FLYING_SPEED, so speeds around the limit don't switch every frame between colour cells
// and full cards (each switch makes the whole band again). Not lower: the landing's cards and
// pictures start when it stops flying, so a fling that settles late lands on BlurHashes.
let libraryFlyingNow = false;
function libraryFlyingLimit() {
    return libraryFlyingNow ? LIBRARY_FLYING_SPEED * 0.75 : LIBRARY_FLYING_SPEED;
}

// The speed of a scroll event in screens a second. The first event after a pause is a landing
// (a jump, a PageDown, a scrollbar click: no more motion follows), so it never counts as flying.
function libraryScrollSpeed(moved, dt, vh) {
    if (dt > 200) return 0;
    return (Math.abs(moved) / vh) * (1000 / Math.max(1, dt));
}

window.addEventListener('scroll', () => {
    if (!libraryView) return;
    const y = window.scrollY;
    const now = performance.now();
    if (y !== libraryLastY) libraryDir = y > libraryLastY ? 1 : -1;
    // Smoothed over consecutive events (frames of uneven length give uneven speeds); a landing
    // after a pause is 0, as is the event that brings it
    const raw = libraryScrollSpeed(y - libraryLastY, now - libraryLastScroll, innerHeight);
    const speed = raw && libraryLastSpeed ? (raw + libraryLastSpeed) / 2 : raw;
    libraryLastY = y;
    libraryLastScroll = now;
    libraryLastSpeed = speed;
    libraryUpdate(speed);
}, { passive: true });
// A fling or drag that ends (where the browser says so) gets its cards in the next frames, unless
// another scroll follows at once (a script scrolling frame by frame ends a scroll every frame)
window.addEventListener('scrollend', () => {
    if (!libraryView) return;
    const at = libraryLastScroll;
    requestAnimationFrame(() => requestAnimationFrame(() => {
        if (!libraryView || libraryLastScroll !== at) return;
        clearTimeout(librarySettle);
        libraryUpdate(0);
    }));
});

// The cell whose row's bottom is the first below the header (the rail and the memory use it)
function libraryFirstVisible() {
    const v = libraryView;
    if (!v || !libraryTotal) return 0;
    const below = $('main-navbar')?.getBoundingClientRect().bottom ?? 0;
    const y = window.scrollY + below - libraryGridTop(v);
    const row = Math.max(0, Math.floor((y - v.cellHeight) / v.pitch) + 1);
    return Math.min(libraryTotal - 1, row * v.cols);
}

// Scrolls so that cell `k`'s row is at the top, below the header
function libraryScrollToIndex(k) {
    const v = libraryView;
    if (!v) return;
    const margin = 4.5 * parseFloat(getComputedStyle(document.documentElement).fontSize);
    window.scrollTo({ top: libraryGridTop(v) + Math.floor(k / v.cols) * v.pitch - margin, behavior: 'instant' });
}

// Lays the grid out again for its width now (a resize, the sidebar, fonts arriving), keeping the
// first visible item where it was on screen
function libraryRelayout() {
    const v = libraryView;
    if (!v) return;
    const anchor = libraryFirstVisible();
    const before = libraryGridTop(v) + libraryCellBox(anchor, v).y - window.scrollY;
    const layout = libraryMeasure(v.grid, libraryTotal);
    if (layout.cols === v.cols && layout.colWidth === v.colWidth && layout.cellHeight === v.cellHeight) return;
    Object.assign(v, layout);
    librarySize(v, libraryTotal);
    for (const [i, cell] of v.cells) libraryPlace(cell, i, v);
    window.scrollTo({ top: libraryGridTop(v) + libraryCellBox(anchor, v).y - before, behavior: 'instant' });
    libraryUpdate();
}

// How long a changed total must hold before the index is asked again (a scan changes it often)
const LIBRARY_INDEX_SETTLE_MS = 3000;
// Page requests are numbered; the total comes from the newest answer that changed it
let librarySeq = 0;
let libraryTotalSeq = 0;
let libraryIndexRefresh = null;
// The index asks for at least this many items (the server's default cap is 10,000), so it can go
// out before the first page has told the total
const LIBRARY_INDEX_LIMIT = 100000;
// While over 5% of the index's pictures have no BlurHash, it is asked again this much later, up to
// LIBRARY_INDEX_HASH_RETRIES times a load
const LIBRARY_INDEX_HASH_RETRY_MS = 20000;
const LIBRARY_INDEX_HASH_RETRIES = 6;
let libraryIndexHashRetries = 0;
// How many of the last index's pictures had no BlurHash (null before the first index of a load)
let libraryIndexMissing = null;
// When the index came (performance.now()) and how long it took, for the diagnostics
let libraryIndexAt = null;
let libraryIndexMs = null;

// The list changed under the load (a page came with another total, as during a scan): the grid
// takes the new total in place, without a jump or a flash of skeletons
function libraryChanged(total) {
    const v = libraryView;
    libraryTotal = total;
    // Every page may have moved: they are asked again as the screen needs them, apart from those
    // in flight (the page that brought this total among them), whose answers are as new as it is
    libraryRequested = new Set(libraryInFlight.keys());
    libraryItems.length = Math.min(libraryItems.length, total);
    const countLabel = $('lib-count-label');
    if (countLabel) countLabel.textContent = t('library.item_count', { count: total });
    if (!v) return;
    const focusedPos = v.grid.contains(document.activeElement)
        ? Number(document.activeElement.closest('[data-pos]')?.dataset.pos ?? -1) : -1;
    if (total === 0) {
        // Nothing left: the empty state, as a load of no items shows it, with focus on it
        v.resizeObserver.disconnect();
        for (const cell of v.cells.values()) posterLoader.forget(cell.firstElementChild);
        libraryView = null;
        const container = $('lib-grid-container');
        if (container) {
            container.innerHTML = libraryEmptyHtml();
            if (focusedPos >= 0) container.firstElementChild.focus({ preventScroll: true });
        }
        return;
    }
    // The grid takes the new height and count in place; the cells in view stay, and their pages
    // correct them as they come (a card whose position now holds another item is replaced)
    Object.assign(v, libraryLayout(v.grid.clientWidth, v.cols, v.gap, v.cellHeight, total));
    librarySize(v, total);
    for (const [i, cell] of v.cells) {
        if (i >= total) {
            cell.remove();
            v.cells.delete(i);
            posterLoader.forget(cell.firstElementChild);
        } else {
            cell.setAttribute('aria-setsize', String(total));
        }
    }
    libraryUpdate();
    // A focused card that went with the end of the list hands focus to the new last one
    if (focusedPos >= total) v.cells.get(total - 1)?.firstElementChild?.focus({ preventScroll: true });
    // The index is asked again once the total has held for a while, not on every change
    clearTimeout(libraryIndexRefresh);
    const gen = libraryGen;
    libraryIndexRefresh = setTimeout(() => {
        if (gen === libraryGen && libraryTotal === total) loadLibraryIndex(gen);
    }, LIBRARY_INDEX_SETTLE_MS);
}

// Fills #lib-grid-container with the virtual grid of `total` items, `items` the first page
function fillLibraryGrid(items, total) {
    const container = $('lib-grid-container');
    // A library left before its first page came builds nothing in the hidden page
    if (!container || $('page-library')?.classList.contains('hidden')) return;
    libraryTotal = total;
    items.forEach((item, i) => { libraryItems[i] = item; });
    if (total === 0) {
        container.innerHTML = libraryEmptyHtml();
        return;
    }
    container.innerHTML = `<div id="lib-grid" class="${libraryGridClass()}" role="list"></div>`;
    const grid = container.firstElementChild;
    grid.setAttribute('aria-label', $('page-library')?.querySelector('h1')?.textContent || t('common.library'));
    const layout = libraryMeasure(grid, total);
    let resizeFrame = 0;
    const resizeObserver = new ResizeObserver(() => {
        cancelAnimationFrame(resizeFrame);
        resizeFrame = requestAnimationFrame(libraryRelayout);
    });
    libraryView = { ...layout, grid, cells: new Map(), resizeObserver };
    librarySize(libraryView, total);
    resizeObserver.observe(grid);
    document.fonts?.ready.then(() => { if (libraryView?.grid === grid) libraryRelayout(); });
    // Enter or Space on a focused card opens it, as a click does
    grid.addEventListener('keydown', e => {
        if ((e.key === 'Enter' || e.key === ' ') && e.target.matches?.('.jf-card[data-action]')) {
            e.preventDefault();
            e.target.click();
        }
    });

    // Back to the list where it was left, for the same query, when the page came back through
    // history (Back or Forward); any other visit starts at the top
    const memory = libraryRestore ? libraryMemory : null;
    libraryMemory = null;
    libraryRestore = false;
    if (memory && memory.key === JSON.stringify(libraryQuery) && memory.index < total) {
        libraryScrollToIndex(memory.index);
    }
    libraryLastY = window.scrollY;
    libraryUpdate();
    scheduleRailUpdate();
    // The index went out with the first page; a load without it (none asked, or it failed) asks now
    if (items.length < total && !libraryIndex && !libraryIndexRequest) loadLibraryIndex(libraryGen);
}

// Remembers where the library list is, before the page is left
function libraryRemember() {
    if (libraryView && libraryQuery) libraryMemory = { key: JSON.stringify(libraryQuery), index: libraryFirstVisible() };
}

// Keeps the items of one page and fills its rendered cells
async function loadLibraryPage(gen, page) {
    const seq = ++librarySeq;
    const controller = new AbortController();
    libraryInFlight.set(page, controller);
    try {
        const { items, total } = await fetchLibraryPage(page * LIBRARY_PAGE_SIZE, controller.signal);
        if (gen !== libraryGen) return;
        // The list changed since the load began (items added or removed, as during a scan): the
        // grid takes the new total in place, and this page's items count as of now. An answer
        // older than the one that set the total is left out, so the total never goes back.
        if (total !== libraryTotal) {
            if (seq < libraryTotalSeq) {
                // Its items are as old as its total: the page is asked again under the current
                // total (the update in finally), unless a newer request for it is already out
                if (libraryInFlight.get(page) === controller) libraryRequested.delete(page);
                return;
            }
            libraryTotalSeq = seq;
            libraryChanged(total);
        }
        items.forEach((item, j) => {
            const i = page * LIBRARY_PAGE_SIZE + j;
            libraryItems[i] = item;
            libraryFillCell(i);
        });
        scheduleRailUpdate();
    } catch (e) {
        if (gen !== libraryGen) return;
        // Only this request's own mark: an aborted one must not clear a newer request's
        if (libraryInFlight.get(page) === controller) libraryRequested.delete(page);
        if (e.name !== 'AbortError') toast(t('library.load_failed', { message: e.message }), 'error');
    } finally {
        if (libraryInFlight.get(page) === controller) libraryInFlight.delete(page);
        // The next pages the screen needs
        if (gen === libraryGen) scheduleLibraryUpdate();
    }
}

// --- Grid diagnostics (opt-in) ---
// With ?debug=grid in the page's URL (before the #) or localStorage 'rustyfin.gridDebug' = '1', a
// small panel shows what the library grid has on screen, the index's state and the requests'
// timings, so a user can report what their browser sees (Copy as text gives it as plain text). It
// shows counts and times only, never a URL (picture URLs carry the session's token).
// window.rfGridDiag() returns the same numbers. Off, it costs nothing past this check at start.
function gridDiagEnabled() {
    try {
        return new URLSearchParams(location.search).get('debug') === 'grid'
            || hashRoute(location.hash.slice(1)).query.get('debug') === 'grid'
            || sessionStorage.getItem('rustyfin.gridDebug') === '1'
            || localStorage.getItem('rustyfin.gridDebug') === '1';
    } catch {
        return false;
    }
}

// What the panel collects while it is on (nothing is collected when it is off): the last
// GRID_DIAG_RING requests of each kind as numbers only (no URL), the pictures' decode-to-paint
// times and widths, and the cells on screen at the last stop
const GRID_DIAG_RING = 300;
const gridDiagData = { image: [], items: [], paint: [], stop: null, stopLater: null };
// Set by the panel: the poster loader calls it for each picture loaded (null when the panel is off)
let gridDiagOnPoster = null;

const gridDiagPush = (list, value) => {
    list.push(value);
    if (list.length > GRID_DIAG_RING) list.shift();
};

// The median and 90th percentile of a list of numbers, rounded (nulls when empty)
function gridDiagSpread(values) {
    const v = values.filter(Number.isFinite).sort((a, b) => a - b);
    const at = p => (v.length ? Math.round(v[Math.min(v.length - 1, Math.floor((v.length - 1) * p))]) : null);
    return { p50: at(0.5), p90: at(0.9) };
}

// The library cells on screen by state
function gridDiagCells() {
    const cells = { poster: 0, blurhash: 0, shimmer: 0, colour: 0, skeleton: 0, icon: 0 };
    for (const cell of libraryView?.cells.values() || []) {
        const r = cell.getBoundingClientRect();
        if (r.bottom <= 0 || r.top >= innerHeight) continue;
        const img = cell.querySelector('img.jf-thumb-img');
        if (cell.hasAttribute('data-i')) cells[cell.dataset.tint ? 'colour' : 'skeleton']++;
        else if (!img) cells.icon++;
        else if (img.complete && img.naturalWidth > 0 && img.style.opacity) cells.poster++;
        else if (cell.querySelector('.jf-blurhash:not([data-blurhash])')) cells.blurhash++;
        else cells.shimmer++;
    }
    return cells;
}

// Timings of one kind of request: queueing (fetchStart to requestStart: waiting for a connection,
// and connecting), the server (requestStart to responseStart), the download (responseStart to
// responseEnd), the protocols and how many came from the browser's cache (transferSize 0)
function gridDiagRequests(list) {
    const net = list.filter(e => !e.cached);
    const protocols = {};
    for (const e of list) protocols[e.protocol || '?'] = (protocols[e.protocol || '?'] || 0) + 1;
    return {
        n: list.length,
        cached: list.length - net.length,
        protocols,
        queueMs: gridDiagSpread(net.map(e => e.queue)),
        serverMs: gridDiagSpread(net.map(e => e.server)),
        downloadMs: gridDiagSpread(net.map(e => e.download)),
    };
}

// The last index request's size and times, from its resource entry (its URL is matched and dropped)
function gridDiagIndexRequest() {
    const e = performance.getEntriesByType('resource').filter(r => r.name.includes('RustyfinCardIndex=true')).pop();
    return {
        ms: libraryIndexMs == null ? null : Math.round(libraryIndexMs),
        serverMs: e ? Math.round(e.responseStart - e.requestStart) : null,
        wireKB: e ? Math.round(e.transferSize / 1024) : null,
        rawKB: e ? Math.round(e.decodedBodySize / 1024) : null,
    };
}

function gridDiag() {
    const now = performance.now();
    const index = libraryIndex
        ? { items: libraryIndex.length, blurhashes: libraryIndex.filter(libraryHasHash).length, agoS: libraryIndexAt == null ? null : Math.round((now - libraryIndexAt) / 1000), ...gridDiagIndexRequest() }
        : libraryIndexRequest ? 'loading' : 'none';
    const widths = {};
    for (const p of gridDiagData.paint) widths[p.width] = (widths[p.width] || 0) + 1;
    return {
        browser: navigator.userAgent,
        viewport: `${innerWidth}x${innerHeight}`,
        dpr: devicePixelRatio,
        secure: window.isSecureContext,
        serviceWorker: !!navigator.serviceWorker?.controller,
        total: libraryTotal,
        index,
        pagesInFlight: libraryInFlight.size,
        pagesHeld: libraryItems.filter(Boolean).length,
        cells: gridDiagCells(),
        cellsInPage: libraryView?.cells.size ?? 0,
        flying: libraryFlying(),
        speed: Math.round(libraryLastSpeed * 10) / 10,
        postersLoading: posterLoader.loading.size,
        postersRemembered: posterLoader.seen.size,
        atLastStop: gridDiagData.stop,
        halfSecondAfterStop: gridDiagData.stopLater,
        images: gridDiagRequests(gridDiagData.image),
        items: gridDiagRequests(gridDiagData.items),
        // From a picture's load event to the frame after it decoded (decode and paint)
        decodePaintMs: gridDiagSpread(gridDiagData.paint.map(p => p.ms)),
        posterWidths: widths,
    };
}

// The numbers as plain text, one line each: the panel's body and what the copy button copies
function gridDiagText(d) {
    const c = s => `poster ${s.poster} · blurhash ${s.blurhash} · shimmer ${s.shimmer} · colour ${s.colour} · skeleton ${s.skeleton} · icon ${s.icon}`;
    const idx = typeof d.index === 'string' ? d.index : `${d.index.items} items, ${d.index.blurhashes} BlurHashes, ${d.index.agoS}s ago\n`
        + `  index ${d.index.ms ?? '-'} ms (server ${d.index.serverMs ?? '-'} ms) · ${d.index.wireKB ?? '-'} KB on the wire, ${d.index.rawKB ?? '-'} KB raw`;
    const ms = s => `${s.p50 ?? '-'}/${s.p90 ?? '-'}`;
    const req = r => `${r.n} (${r.cached} cached) · ${Object.entries(r.protocols).map(([k, n]) => `${k} ${n}`).join(', ') || '-'}\n`
        + `  queue ${ms(r.queueMs)} · server ${ms(r.serverMs)} · download ${ms(r.downloadMs)} ms (p50/p90)`;
    return [
        `Rustyfin grid diagnostics ${new Date().toISOString()}`,
        d.browser,
        `viewport ${d.viewport} @${d.dpr}x · secure ${d.secure ? 'yes' : 'no'} · service worker ${d.serviceWorker ? 'yes' : 'no'}`,
        `total ${d.total} · index ${idx}`,
        `pages held ${d.pagesHeld} items · in flight ${d.pagesInFlight}`,
        `on screen: ${c(d.cells)}`,
        `at last stop: ${d.atLastStop ? c(d.atLastStop) : '-'}`,
        `500 ms after: ${d.halfSecondAfterStop ? c(d.halfSecondAfterStop) : '-'}`,
        `cells in page ${d.cellsInPage} · ${d.flying ? 'flying' : 'settled'} · ${d.speed} screens/s`,
        `pictures loading ${d.postersLoading} · remembered ${d.postersRemembered} · widths ${Object.entries(d.posterWidths).map(([w, n]) => `${w}px ${n}`).join(', ') || '-'}`,
        `images ${req(d.images)}`,
        `  decode+paint ${ms(d.decodePaintMs)} ms (p50/p90)`,
        `items ${req(d.items)}`,
    ].join('\n');
}

// Copies text; navigator.clipboard needs a secure context, so plain http falls back to a selection
async function gridDiagCopy(text) {
    try {
        await navigator.clipboard.writeText(text);
        return true;
    } catch {
        const area = document.createElement('textarea');
        area.value = text;
        area.style.cssText = 'position:fixed;opacity:0';
        document.body.appendChild(area);
        area.select();
        const ok = document.execCommand('copy');
        area.remove();
        return ok;
    }
}

// asked: true when the route asked for it (#…?debug=grid), whatever gridDiagEnabled says
function startGridDiag(asked = false) {
    if (!(asked || gridDiagEnabled()) || $('rf-grid-diag')) return;
    window.rfGridDiag = gridDiag;
    // Requests as numbers only: the URL is read for its path and dropped
    const kind = name => {
        const path = new URL(name, location.href).pathname;
        return /\/Images\//.test(path) ? 'image' : /\/Items$/.test(path) || /\/Users\/[^/]+\/Items$/.test(path) ? 'items' : null;
    };
    new PerformanceObserver(list => {
        for (const e of list.getEntries()) {
            const k = kind(e.name);
            if (!k) continue;
            gridDiagPush(gridDiagData[k], {
                cached: e.transferSize === 0 && e.decodedBodySize > 0,
                // A response from the browser's cache has no protocol
                protocol: e.nextHopProtocol || (e.transferSize === 0 ? 'cache' : '?'),
                queue: e.requestStart - e.fetchStart,
                server: e.responseStart - e.requestStart,
                download: e.responseEnd - e.responseStart,
            });
        }
    }).observe({ type: 'resource', buffered: true });
    // Each picture the loader shows: from its load event to the frame after it decoded
    gridDiagOnPoster = img => {
        const loaded = performance.now();
        const width = Number(img.dataset.w) || img.naturalWidth;
        (img.decode ? img.decode() : Promise.resolve()).catch(() => {}).then(() => requestAnimationFrame(() => {
            gridDiagPush(gridDiagData.paint, { ms: performance.now() - loaded, width });
        }));
    };
    // The cells on screen when a scroll stops (no scroll event for 150 ms), and 500 ms later
    let stopTimer = 0;
    let laterTimer = 0;
    window.addEventListener('scroll', () => {
        clearTimeout(stopTimer);
        clearTimeout(laterTimer);
        stopTimer = setTimeout(() => {
            if (!libraryView) return;
            gridDiagData.stop = gridDiagCells();
            gridDiagData.stopLater = null;
            laterTimer = setTimeout(() => { if (libraryView) gridDiagData.stopLater = gridDiagCells(); }, 500);
        }, 150);
    }, { passive: true });

    const panel = document.createElement('div');
    panel.id = 'rf-grid-diag';
    panel.style.cssText = 'position:fixed;right:8px;bottom:8px;z-index:9999;padding:8px 10px;max-width:420px;'
        + 'font:11px/1.35 ui-monospace,monospace;color:#e6edf3;background:rgba(0,0,0,.85);border:1px solid #30363d;border-radius:8px';
    const body = document.createElement('pre');
    body.style.cssText = 'margin:0;white-space:pre-wrap;user-select:text';
    const copy = document.createElement('button');
    copy.type = 'button';
    copy.textContent = 'Copy as text';
    copy.style.cssText = 'margin-top:6px;padding:2px 8px;border:1px solid #30363d;border-radius:6px;background:#161b22;color:#e6edf3;cursor:pointer';
    copy.addEventListener('click', async () => {
        const ok = await gridDiagCopy(gridDiagText(gridDiag()));
        copy.textContent = ok ? 'Copied' : 'Copy failed: select the text';
        setTimeout(() => { copy.textContent = 'Copy as text'; }, 2000);
    });
    panel.append(body, copy);
    document.body.appendChild(panel);
    setInterval(() => {
        body.textContent = libraryView ? gridDiagText(gridDiag()) : 'grid diagnostics: open a library';
    }, 500);
}
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', () => startGridDiag());
else startGridDiag();

// A sort or filter change: a new load for the new query, shown from the top of the list
async function reloadLibrary(libId) {
    renderLetterRail(libId);

    window.scrollTo({ top: 0, behavior: 'instant' });
    const gen = startLibraryLoad(libId);
    const container = $('lib-grid-container');
    if (container) {
        container.innerHTML = '<div class="flex justify-center items-center py-24"><div class="w-10 h-10 border-3 border-[#00a4dc] border-t-transparent rounded-full animate-spin"></div></div>';
    }

    loadLibraryIndex(gen);
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
    if (libraryTotal === null || !libraryView) return;
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
        if (stale() || !libraryView) return;
        if (k < n) {
            libraryScrollToIndex(k);
            // The clicked section stays lit while its row is the top row (that row often starts with the previous letter)
            railAnchor = { k, first: libraryFirstVisible() };
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

function setRailLetter(letter) {
    for (const b of $('lib-letter-rail').children) {
        if (b.dataset.letter === letter) b.setAttribute('aria-current', 'true');
        else b.removeAttribute('aria-current');
    }
}

// Lights the section of the first item at the top of the view, or after a jump the clicked one (see jumpToLetter)
function updateLetterRail() {
    const page = $('page-library');
    if (!libraryView || page.classList.contains('hidden') || !page.classList.contains('jf-with-rail')) return;
    let i = libraryFirstVisible();
    if (railAnchor && i === railAnchor.first) i = railAnchor.k;
    else railAnchor = null;
    const item = libraryItems[i] || libraryIndex?.[i];
    if (item) setRailLetter(railLetter(item)); // a cell without its item keeps the highlight
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

// A mark made from a card (cardmenu.js): the open list's loaded item takes it, so a card drawn again
// from it (scrolled away and back) shows it too
document.addEventListener('jf-userdata', e => {
    const { id, data } = e.detail;
    for (const item of libraryItems) {
        if (item && String(item.Id || item.id) === String(id)) {
            const merged = { ...item.UserData, ...data };
            if (data.Played) {
                merged.PlayedPercentage = 0;
                if (merged.UnplayedItemCount) merged.UnplayedItemCount = 0;
            }
            item.UserData = merged;
        }
    }
});

export {
    currentLibraryId, currentLibraryRoute, blurhashTint, posterLoaded, posterFailed, LIBRARY_PAGE_SIZE, setupInfiniteScroll,
    destroyInfiniteScroll, addInfiniteScrollSentinel, renderLibrary, stopLibraryLoad, libraryRefreshInPlace, libraryRemember, startGridDiag,
    jumpToLetter, clearLibraryFilters,
};

// Shared state that inline handlers or other modules assign: reached through window
Object.defineProperties(window, {
    libraryRestore: { get: () => libraryRestore, set: value => { libraryRestore = value; } },
});
