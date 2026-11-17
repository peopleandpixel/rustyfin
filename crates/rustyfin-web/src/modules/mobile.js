// Mobile orientation for the player. Like Jellyfin 12.4's playbackorientation.js, a video in fullscreen
// on a touch device locks the screen to landscape, and the lock is released when fullscreen ends.
// Only a lock taken here is released, so an unlock never undoes a lock someone else holds.
let locked = false;

export function lockLandscape() {
    if (!globalThis.matchMedia?.('(pointer: coarse)').matches) return;
    const o = globalThis.screen?.orientation;
    if (!o || typeof o.lock !== 'function') return;
    locked = true;
    try { o.lock('landscape').catch(() => {}); } catch { /* the lock is refused: nothing to undo */ }
}

export function unlockOrientation() {
    if (!locked) return;
    locked = false;
    const o = globalThis.screen?.orientation;
    if (o && typeof o.unlock === 'function') o.unlock();
}
