// The user's data on items (played, favourite, likes, resume point, unplayed count), kept in step with
// the server as Jellyfin Web does. The server's UserDataChanged message and this tab's own marks take
// one path, userDataChanged: every card of the item redraws, the cached lists go, and the open lists,
// item page and queue hear of it (the 'jf-userdata' event). The folders above a changed item that are
// on screen (its season and series) then read their own data again, for their unplayed counts.
import { applyCardUserData } from './cards.js';

// The fields a change can carry; the last values each item took, so a change this tab already shows
// (its own mark, echoed by the server) draws nothing again
const USER_DATA_KEYS = ['Played', 'IsFavorite', 'Likes', 'PlaybackPositionTicks', 'PlayedPercentage', 'UnplayedItemCount', 'PlayCount'];
const lastUserData = new Map();

// The folders waiting to read their data again, and the timer that reads them
const PARENT_READ_DELAY_MS = 300;
const parentsToRead = new Set();
let parentTimer = 0;

// Other item ids on screen that aren't cards (the open item page), from app.js
let shownIdsSource = () => [];
function setShownIds(fn) {
    shownIdsSource = fn;
}

function changesSomething(id, data) {
    const last = lastUserData.get(id) || {};
    return USER_DATA_KEYS.some(key => data[key] !== undefined && last[key] !== data[key]);
}

function remember(id, data) {
    const last = { ...lastUserData.get(id) };
    for (const key of USER_DATA_KEYS) if (data[key] !== undefined) last[key] = data[key];
    lastUserData.set(id, last);
}

const cardsOf = id => document.querySelectorAll(`.jf-card[data-id="${CSS.escape(String(id))}"]`);

// The season and series of a changed item, as its cards name them (data-parents)
function parentsOf(id) {
    const ids = new Set();
    for (const card of cardsOf(id)) {
        for (const parent of (card.dataset.parents || '').split(' ')) if (parent && parent !== id) ids.add(parent);
    }
    return ids;
}

function isShown(id) {
    return cardsOf(id).length > 0 || shownIdsSource().some(shown => String(shown) === id);
}

// User data of item `id` changed: `data` is a whole or partial UserData (a mark's answer, or an entry of
// the server's UserDataChanged with `fromServer`)
function userDataChanged(id, data, { fromServer = false } = {}) {
    id = String(id);
    // Lists, Next Up and Resume hold user data too
    API.invalidateCache('/Items');
    API.invalidateCache('/Shows');
    API.invalidateCache('/UserItems');
    API.invalidateCache(`/Users/${API.userId}/Items`);
    if (fromServer && !changesSomething(id, data)) return;
    remember(id, data);
    applyCardUserData(id, data);
    document.dispatchEvent(new CustomEvent('jf-userdata', { detail: { id, data } }));
    readParentsOf(id);
}

function readParentsOf(id) {
    for (const parent of parentsOf(id)) parentsToRead.add(parent);
    if (parentsToRead.size === 0 || parentTimer) return;
    parentTimer = setTimeout(readParents, PARENT_READ_DELAY_MS);
}

// Each waiting folder that is still on screen reads its data again: its played state and unplayed
// count follow its episodes (the server sends only one level up)
async function readParents() {
    parentTimer = 0;
    const ids = [...parentsToRead];
    parentsToRead.clear();
    for (const id of ids) {
        if (!isShown(id)) continue;
        try {
            const data = await API.get(`/UserItems/${encodeURIComponent(id)}/UserData?userId=${encodeURIComponent(API.userId)}`);
            if (data) userDataChanged(id, data, { fromServer: true });
        } catch {
            // The folder keeps what it shows; the next change or a reload reads it again
        }
    }
}

// The server's UserDataChanged (Jellyfin's UserDataChangeInfo: UserId and UserDataList), for this user
function serverUserDataChanged(info) {
    if (!info || (info.UserId && API.userId && String(info.UserId).replace(/-/g, '') !== String(API.userId).replace(/-/g, ''))) return;
    for (const entry of info.UserDataList || []) {
        if (entry?.ItemId) userDataChanged(entry.ItemId, entry, { fromServer: true });
    }
}

export { userDataChanged, serverUserDataChanged, setShownIds };
