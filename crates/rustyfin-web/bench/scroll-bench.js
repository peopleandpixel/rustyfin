// Scroll benchmark for poster grids. Paste into the browser console (or run it through a browser
// automation tool) on a page with a grid, then call rfScrollBench.run(opts) and poll
// rfScrollBench.result() (null while running). rfScrollBench.start({ speed }) is run in 'steady'.
//
// Each grid cell overlapping the viewport is in one stage per frame:
//   placeholder - a library cell whose item has not arrived ([data-i]): a skeleton, counted
//                 apart from what is painted
//   colour      - a placeholder painted in its picture's BlurHash average colour (data-tint)
//   blank       - a card that shows nothing yet (no painted BlurHash, no loaded picture)
//   preview     - its BlurHash is painted
//   poster      - its <img> has loaded and is not transparent
// Cards without a picture (they show their type icon) are left out. `skipped` counts viewport cells
// content-visibility still skips.
//
// Modes:
//   steady { speed, maxMs }  px/s from the top to the end (or for maxMs), driven from the page
//   fling  { v0, tau, from, stopBelow }
//                            v = v0 * exp(-t / tau) px/s, from the current position (or `from`,
//                            a fraction of the page), until v < stopBelow (default 20 px/s: a
//                            trackpad's momentum; a spun mouse wheel stops abruptly, e.g. 1500)
//   jump   { to }            an instant scroll to a fraction of the page (a scrollbar drag, End)
//   rail   { letter }        a click on the library's letter rail
//   watch                    records while real input (wheel, touch, scrollbar) scrolls; the
//                            motion starts at the first change and stops at the last change
//                            before 250 ms without one
// The summary has, for cells that entered the viewport during the motion, the time from entering to
// a preview or poster (0 when already shown) and the share within 100 ms; the mean share of each
// stage in the viewport during the motion; after the stop, the shares at 0-2000 ms and the time
// until the viewport has no blanks and only posters; frame counts; and the library page and image
// requests from Resource Timing, with the query (which carries api_key) left out.
(() => {
    const state = { running: false, summary: null };
    const STAGES = ['blank', 'preview', 'poster'];

    function stage(cell) {
        if (cell.hasAttribute('data-i')) return cell.dataset.tint ? 'colour' : 'placeholder';
        const img = cell.querySelector('img');
        if (img && img.complete && img.naturalWidth > 0 && getComputedStyle(img).opacity !== '0') return 'poster';
        const hash = cell.querySelector('.jf-blurhash');
        if (hash && !hash.hasAttribute('data-blurhash')) return 'preview';
        if (!img) return 'noimage';
        return 'blank';
    }

    const percentile = (sorted, p) =>
        sorted.length ? sorted[Math.min(sorted.length - 1, Math.floor((sorted.length - 1) * p))] : null;
    const round = n => (n == null || !Number.isFinite(n) ? n : Math.round(n));

    function run(opts = {}) {
        const mode = opts.mode || 'steady';
        const selector = opts.selector || '#lib-grid > *';
        const watchMs = opts.watchMs ?? (mode === 'steady' ? 2000 : 4000);
        const scroller = document.scrollingElement;
        performance.clearResourceTimings();
        performance.setResourceTimingBufferSize(20000);
        state.running = true;
        state.summary = null;
        const cells = new Map(); // grid index -> { enter, shown, poster }
        const motion = { frames: 0, placeholder: 0, colour: 0, blank: 0, preview: 0, poster: 0, skipped: 0 };
        const after = [];
        const recent = [];
        let stopAt = null;
        let t0 = performance.now();
        let lastFrame = null;
        let maxGap = 0;
        const max = () => scroller.scrollHeight - innerHeight;

        if (mode === 'steady') scroller.scrollTop = 0;
        if (mode === 'fling' && opts.from != null) scroller.scrollTop = opts.from * max();
        if (mode === 'jump') {
            scroller.scrollTop = opts.to * max();
            stopAt = performance.now();
        }
        if (mode === 'rail') {
            const btn = document.querySelector(`[data-action="jump-letter"][data-letter="${opts.letter}"]`);
            if (!btn) {
                state.running = false;
                state.summary = { error: `no rail letter ${opts.letter}` };
                return 'no rail';
            }
            const y0 = scroller.scrollTop;
            btn.click();
            const landed = () => (scroller.scrollTop !== y0 ? (stopAt = performance.now()) : requestAnimationFrame(landed));
            requestAnimationFrame(landed);
        }
        const flingStart = scroller.scrollTop;
        let lastY = scroller.scrollTop;
        let lastChange = null;
        let moved = mode !== 'watch';

        function snapshot(t) {
            const c = { placeholder: 0, colour: 0, blank: 0, preview: 0, poster: 0, skipped: 0, total: 0 };
            const all = document.querySelectorAll(selector);
            for (let i = 0; i < all.length; i++) {
                const r = all[i].getBoundingClientRect();
                if (r.bottom <= 0 || r.top >= innerHeight) continue;
                const s = stage(all[i]);
                if (s === 'noimage') continue;
                c[s]++;
                c.total++;
                if (all[i].checkVisibility && !all[i].checkVisibility({ contentVisibilityAuto: true })) c.skipped++;
                if (stopAt == null) {
                    // A virtual grid's cells carry their list position; others are in list order
                    const key = all[i].dataset.pos ?? i;
                    let cell = cells.get(key);
                    if (!cell) cells.set(key, (cell = { enter: t, shown: null, poster: null }));
                    if (s !== 'placeholder' && s !== 'blank' && cell.shown == null) cell.shown = t;
                    if (s === 'poster' && cell.poster == null) cell.poster = t;
                }
            }
            return c;
        }

        function frame(now) {
            if (lastFrame != null && stopAt == null) maxGap = Math.max(maxGap, now - lastFrame);
            lastFrame = now;
            const t = now - t0;
            // The cells are counted before this frame moves the page: they are what the last frame
            // painted, after its scroll event (which a page-driven scroll fires only next frame)
            const c = moved ? snapshot(t) : null;
            if (stopAt == null && mode === 'steady') {
                scroller.scrollTop = Math.min(max(), (opts.speed * t) / 1000);
                if (scroller.scrollTop >= max() - 1 || (opts.maxMs && t >= opts.maxMs)) stopAt = now;
            } else if (stopAt == null && mode === 'fling') {
                const tau = opts.tau || 325;
                scroller.scrollTop = Math.min(max(), flingStart + (opts.v0 * tau * (1 - Math.exp(-t / tau))) / 1000);
                if (opts.v0 * Math.exp(-t / tau) < (opts.stopBelow || 20) || scroller.scrollTop >= max() - 1) stopAt = now;
            } else if (stopAt == null && mode === 'watch') {
                if (scroller.scrollTop !== lastY) {
                    if (!moved) t0 = now;
                    moved = true;
                    lastChange = now;
                    lastY = scroller.scrollTop;
                } else if (moved && now - lastChange > 250) {
                    stopAt = lastChange;
                    for (const r of recent) if (r.now >= stopAt) after.push({ t: r.now - stopAt, ...r.c });
                } else if (!moved && now - t0 > 30000) {
                    stopAt = now;
                }
            }
            if (!moved) {
                requestAnimationFrame(frame);
                return;
            }
            if (!c) {
                requestAnimationFrame(frame); // the first frame that moved: counted from the next
                return;
            }
            if (stopAt == null) {
                if (mode === 'watch') {
                    recent.push({ now, c });
                    if (recent.length > 60) recent.shift();
                }
                if (c.total) {
                    motion.frames++;
                    for (const k of ['placeholder', 'colour', ...STAGES, 'skipped']) motion[k] += c[k] / c.total;
                }
            } else {
                after.push({ t: now - stopAt, ...c });
                if (now - stopAt >= watchMs) return finish();
            }
            requestAnimationFrame(frame);
        }

        function finish() {
            const pct = (s, k) => (s.total ? round((100 * s[k]) / s.total) : null);
            const at = ms => {
                const s = after.find(a => a.t >= ms) || after[after.length - 1];
                return s && { placeholder: pct(s, 'placeholder'), colour: pct(s, 'colour'), blank: pct(s, 'blank'), preview: pct(s, 'preview'), poster: pct(s, 'poster') };
            };
            const firstAll = test => {
                const s = after.find(a => a.total && test(a));
                return s ? round(s.t) : null;
            };
            const m = k => (motion.frames ? round((100 * motion[k]) / motion.frames) : null);
            const list = [...cells.values()];
            const lat = key => list.map(c => (c[key] == null ? Infinity : c[key] - c.enter)).sort((a, b) => a - b);
            const shown = lat('shown');
            const posters = lat('poster');
            const within = (arr, ms) => (arr.length ? round((100 * arr.filter(x => x <= ms).length) / arr.length) : null);
            const path = e => new URL(e.name).pathname;
            const entries = performance.getEntriesByType('resource');
            const pages = entries
                .filter(e => path(e) === '/Items' || /\/Users\/[^/]+\/Items$/.test(path(e)))
                .map(e => ({
                    start: round(e.startTime - stopAt),
                    queued: round(e.requestStart - e.startTime),
                    server: round(e.responseStart - e.requestStart),
                    ms: round(e.responseEnd - e.startTime),
                    kb: round((e.transferSize || e.encodedBodySize) / 1024),
                }));
            const images = entries.filter(e => /\/Items\/[^/]+\/Images\//.test(path(e)));
            // Pictures asked from 500 ms before the stop on: how long each waited for a connection
            // (requestStart - startTime, with any connect), the server's time to the first byte,
            // and the whole request
            const late = images.filter(e => e.startTime >= stopAt - 500 && e.responseEnd > 0);
            const dist = f => {
                const v = late.map(f).sort((a, b) => a - b);
                return { p50: round(percentile(v, 0.5)), p95: round(percentile(v, 0.95)) };
            };
            state.summary = {
                mode,
                viewport: `${innerWidth}x${innerHeight}@${devicePixelRatio}`,
                motionMs: round(stopAt - t0),
                frames: motion.frames,
                maxFrameGapMs: round(maxGap),
                cellsEntered: list.length,
                shownWithin100msPct: within(shown, 100),
                posterWithin100msPct: within(posters, 100),
                shownP95Ms: round(percentile(shown, 0.95)),
                posterP95Ms: round(percentile(posters, 0.95)),
                duringMotionPct: { placeholder: m('placeholder'), colour: m('colour'), blank: m('blank'), preview: m('preview'), poster: m('poster'), skipped: m('skipped') },
                // What the viewport painted from the items (BlurHash colour, BlurHash or poster), the
                // skeletons of cells without their item left out
                paintedDuringMotionPct: motion.frames ? round((100 * (motion.colour + motion.preview + motion.poster)) / motion.frames) : null,
                gridCells: document.querySelectorAll(selector).length,
                domNodes: document.getElementsByTagName('*').length,
                afterStop: { 0: at(0), 50: at(50), 100: at(100), 250: at(250), 500: at(500), 1000: at(1000), 2000: at(2000) },
                msToNoBlank: firstAll(a => a.placeholder === 0 && a.blank === 0),
                msToAllCards: firstAll(a => a.placeholder === 0 && a.colour === 0 && a.blank === 0),
                msToAllPosters: firstAll(a => a.poster === a.total),
                pages: pages.filter(p => p.start > -1000).slice(-12),
                pageCount: pages.length,
                imageRequests: images.length,
                imagesAroundStop: late.length ? {
                    count: late.length,
                    queuedMs: dist(e => e.requestStart - e.startTime),
                    serverMs: dist(e => e.responseStart - e.requestStart),
                    totalMs: dist(e => e.responseEnd - e.startTime),
                    kb: round(late.reduce((n, e) => n + (e.transferSize || e.encodedBodySize), 0) / 1024),
                } : null,
                stopAt,
            };
            state.running = false;
        }

        requestAnimationFrame(frame);
        return 'started';
    }

    window.rfScrollBench = {
        run,
        start: (opts = {}) => run({ ...opts, mode: 'steady' }),
        result: () => (state.running ? null : state.summary),
    };
})();
