/**
 * Rustyfin API Client
 */
const API = {
    base: window.location.origin,
    token: localStorage.getItem('rustyfin_token'),
    userId: localStorage.getItem('rustyfin_user_id'),
    userName: localStorage.getItem('rustyfin_user_name'),
    isAdmin: localStorage.getItem('rustyfin_is_admin') === 'true',

    headers() {
        const h = { 'Content-Type': 'application/json' };
        if (this.token) h['Authorization'] = `Bearer ${this.token}`;
        return h;
    },

    authHeader() {
        return 'MediaBrowser Client="Rustyfin Web", Device="Browser", DeviceId="web-' + (localStorage.getItem('rustyfin_device_id') || this._initDeviceId()) + '", Version="0.1.0"';
    },

    _initDeviceId() {
        const id = 'rf-' + Date.now() + '-' + Math.random().toString(36).slice(2, 8);
        localStorage.setItem('rustyfin_device_id', id);
        return id;
    },

    async get(path) {
        const res = await fetch(this.base + path, { headers: this.headers() });
        if (!res.ok) throw new Error(`GET ${path}: ${res.status}`);
        return res.json();
    },

    async post(path, body) {
        const res = await fetch(this.base + path, {
            method: 'POST', headers: this.headers(), body: body ? JSON.stringify(body) : undefined
        });
        if (!res.ok) throw new Error(`POST ${path}: ${res.status}`);
        const text = await res.text();
        return text ? JSON.parse(text) : {};
    },

    async del(path) {
        const res = await fetch(this.base + path, { method: 'DELETE', headers: this.headers() });
        if (!res.ok) throw new Error(`DELETE ${path}: ${res.status}`);
        const text = await res.text();
        return text ? JSON.parse(text) : {};
    },

    // Auth
    async login(username, password) {
        const res = await fetch(this.base + '/Users/AuthenticateByName', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json', 'X-Emby-Authorization': this.authHeader() },
            body: JSON.stringify({ Username: username, Pw: password })
        });
        if (!res.ok) throw new Error('Login fehlgeschlagen');
        const data = await res.json();
        this.token = data.AccessToken || data.access_token;
        this.userId = data.User?.Id || data.user?.id || '';
        this.userName = data.User?.Name || data.user?.name || username;
        this.isAdmin = data.User?.Policy?.IsAdministrator || data.user?.policy?.is_administrator || false;
        localStorage.setItem('rustyfin_token', this.token);
        localStorage.setItem('rustyfin_user_id', this.userId);
        localStorage.setItem('rustyfin_user_name', this.userName);
        localStorage.setItem('rustyfin_is_admin', this.isAdmin);
        return data;
    },

    logout() {
        this.post('/Sessions/Logout').catch(() => {});
        this.token = null; this.userId = null; this.userName = null; this.isAdmin = false;
        ['rustyfin_token', 'rustyfin_user_id', 'rustyfin_user_name', 'rustyfin_is_admin'].forEach(k => localStorage.removeItem(k));
    },

    isLoggedIn() { return !!this.token; },

    // System
    getPublicInfo() { return this.get('/System/Info/Public'); },
    getFullInfo() { return this.get('/System/Info'); },
    getConfig() { return this.get('/System/Configuration'); },
    updateConfig(cfg) { return this.post('/System/Configuration', cfg); },
    getLogs() { return this.get('/System/Logs'); },
    getActivityLog(start = 0, limit = 50) { return this.get(`/System/ActivityLog/Entries?StartIndex=${start}&Limit=${limit}`); },
    shutdown() { return this.post('/System/Shutdown'); },
    restart() { return this.post('/System/Restart'); },

    // Startup / Setup Wizard
    getStartupConfig() { return this.get('/Startup/Configuration'); },
    updateStartupConfig(cfg) { return this.post('/Startup/Configuration', cfg); },
    getStartupUser() { return this.get('/Startup/User'); },
    createStartupUser(name, password) { return this.post('/Startup/User', { Name: name, Password: password }); },
    completeStartup() { return this.post('/Startup/Complete'); },

    // Libraries
    getLibraries() { return this.get('/Library/VirtualFolders'); },
    createLibrary(name, contentType, paths) { return this.post('/Library/VirtualFolders', { Name: name, ContentType: contentType, Paths: paths }); },
    deleteLibrary(id) { return this.del(`/Library/VirtualFolders/${id}`); },
    scanLibrary(id) { return this.post(`/Library/VirtualFolders/${id}/Refresh`); },
    scanAllLibraries() { return this.post('/Library/Refresh'); },

    // Items
    getItems(params = {}) {
        const q = new URLSearchParams();
        Object.entries(params).forEach(([k, v]) => { if (v !== undefined && v !== null && v !== '') q.set(k, v); });
        return this.get('/Items?' + q.toString());
    },
    getItem(id) { return this.get(`/Items/${id}`); },
    getChildren(id, params = {}) {
        const q = new URLSearchParams(params);
        return this.get(`/Items/${id}/Children?${q}`);
    },
    getSimilar(id) { return this.get(`/Items/${id}/Similar`); },
    getLatest(limit = 20) { return this.get(`/Items/Latest?Limit=${limit}`); },
    getResume(limit = 12) { return this.get(`/Items/Resume?Limit=${limit}`); },
    getFilters() { return this.get('/Items/Filters'); },
    deleteItem(id) { return this.del(`/Items/${id}`); },

    // User Items
    getUserItems(params = {}) {
        const q = new URLSearchParams(params);
        return this.get(`/Users/${this.userId}/Items?${q}`);
    },
    getUserLatest(limit = 20) { return this.get(`/Users/${this.userId}/Items/Latest?Limit=${limit}`); },

    // Images
    imageUrl(itemId, type = 'Primary', opts = {}) {
        const q = new URLSearchParams(opts);
        return `${this.base}/Items/${itemId}/Images/${type}?${q}`;
    },

    // Playback
    reportPlaying(itemId, positionTicks = 0) {
        return this.post('/Sessions/Playing', { ItemId: itemId, PositionTicks: positionTicks });
    },
    reportProgress(itemId, positionTicks, isPaused = false) {
        return this.post('/Sessions/Playing/Progress', { ItemId: itemId, PositionTicks: positionTicks, IsPaused: isPaused });
    },
    reportStopped(itemId, positionTicks) {
        return this.post('/Sessions/Playing/Stopped', { ItemId: itemId, PositionTicks: positionTicks });
    },
    markPlayed(itemId) { return this.post(`/Users/${this.userId}/PlayedItems/${itemId}`); },
    markUnplayed(itemId) { return this.del(`/Users/${this.userId}/PlayedItems/${itemId}`); },
    setFavorite(itemId) { return this.post(`/Users/${this.userId}/FavoriteItems/${itemId}`); },
    unsetFavorite(itemId) { return this.del(`/Users/${this.userId}/FavoriteItems/${itemId}`); },

    // Streaming
    streamUrl(itemId, type = 'Videos') { return `${this.base}/${type}/${itemId}/stream`; },

    // Search
    search(term, limit = 20) { return this.get(`/Search/Hints?SearchTerm=${encodeURIComponent(term)}&Limit=${limit}`); },

    // Users (admin)
    getUsers() { return this.get('/Users'); },
    getMe() { return this.get('/Users/Me'); },
    createUser(name, password) { return this.post('/Users/New', { Name: name, Password: password }); },
    deleteUser(id) { return this.post(`/Users/${id}/Delete`); },
    changePassword(id, current, newPw) { return this.post(`/Users/${id}/Password`, { CurrentPw: current, NewPw: newPw }); },
    updatePolicy(id, policy) { return this.post(`/Users/${id}/Policy`, policy); },

    // Sessions
    getSessions() { return this.get('/Sessions'); },

    // Collections & Playlists
    getCollections() { return this.get('/Collections'); },
    createCollection(name, ids) { return this.post('/Collections', { Name: name, Ids: ids }); },
    createPlaylist(name, ids) { return this.post('/Playlists', { Name: name, ItemIds: ids }); },
    getPlaylistItems(id) { return this.get(`/Playlists/${id}/Items`); },

    // Scheduled Tasks
    getTasks() { return this.get('/ScheduledTasks'); },

    // Plugins
    getPlugins() { return this.get('/Plugins'); },

    // Genres, Artists, Studios, Persons
    getGenres() { return this.get('/Genres'); },
    getArtists() { return this.get('/Artists'); },
    getStudios() { return this.get('/Studios'); },
    getPersons() { return this.get('/Persons'); },
    getPerson(id) { return this.get(`/Persons/${id}`); },
};
