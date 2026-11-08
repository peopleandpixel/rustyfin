/**
 * Rustyfin API Client
 * High-performance async client for Jellyfin REST API
 */
const API = {
    base: window.location.origin,
    token: localStorage.getItem('rustyfin_token'),
    userId: localStorage.getItem('rustyfin_user_id'),
    userName: localStorage.getItem('rustyfin_user_name'),
    isAdmin: localStorage.getItem('rustyfin_is_admin') === 'true',

    // ── Request Cache ──
    // In-memory cache for API responses to avoid redundant network requests
    _cache: new Map(),
    _cacheTTL: 30000, // 30 seconds default TTL
    _pendingRequests: new Map(), // Request deduplication: merge identical in-flight requests
    _cacheGeneration: 0, // Bumped when admin rights are lost, so responses already in flight are not cached
    _adminCheck: null, // The running /Users/Me re-read, shared by concurrent callers
    onAdminLost: null, // Set by app.js; called when this account loses administrator rights

    /**
     * Get cached response or fetch from server with deduplication.
     * Automatically merges identical in-flight requests.
     */
    async cachedGet(path, ttlMs = this._cacheTTL) {
        // Check cache first
        const cached = this._cache.get(path);
        if (cached && Date.now() - cached.time < ttlMs) {
            return cached.data;
        }

        // Deduplicate in-flight requests
        if (this._pendingRequests.has(path)) {
            return this._pendingRequests.get(path);
        }

        // Fetch and cache
        const generation = this._cacheGeneration;
        const promise = this.get(path).then(data => {
            if (this._cacheGeneration === generation) {
                this._cache.set(path, { data, time: Date.now() });
            }
            if (this._pendingRequests.get(path) === promise) this._pendingRequests.delete(path);
            return data;
        }).catch(err => {
            if (this._pendingRequests.get(path) === promise) this._pendingRequests.delete(path);
            throw err;
        });

        this._pendingRequests.set(path, promise);
        return promise;
    },

    /**
     * Invalidate cache entries matching a prefix.
     */
    invalidateCache(prefix = '') {
        if (!prefix) {
            this._cache.clear();
            return;
        }
        for (const key of this._cache.keys()) {
            if (key.startsWith(prefix)) {
                this._cache.delete(key);
            }
        }
    },

    /**
     * Create a debounced version of a function.
     */
    debounce(fn, delayMs = 300) {
        let timer = null;
        return function (...args) {
            clearTimeout(timer);
            return new Promise(resolve => {
                timer = setTimeout(() => {
                    fn.apply(this, args).then(resolve);
                }, delayMs);
            });
        };
    },

    headers() {
        const h = { 'Content-Type': 'application/json' };
        if (this.token) {
            h['Authorization'] = `Bearer ${this.token}`;
            h['X-Emby-Token'] = this.token;
        }
        return h;
    },

    authHeader() {
        return 'MediaBrowser Client="Rustyfin Web", Device="Browser", DeviceId="web-' + (localStorage.getItem('rustyfin_device_id') || this._initDeviceId()) + '", Version="12.0.0"';
    },

    _initDeviceId() {
        const id = 'rf-' + Date.now() + '-' + Math.random().toString(36).slice(2, 8);
        localStorage.setItem('rustyfin_device_id', id);
        return id;
    },

    // Builds the Error for a failed response: the JSON body's Detail or Title if present, else "METHOD path: status".
    async _requestError(method, path, res) {
        const fallback = `${method} ${path}: ${res.status}`;
        let message = fallback;
        try {
            const text = await res.text();
            if (text.startsWith('{')) {
                const body = JSON.parse(text);
                message = body.Detail || body.Title || fallback;
            }
        } catch (err) {
            console.warn(`Could not read the error response of ${method} ${path}:`, err);
        }
        const error = new Error(message);
        error.status = res.status;
        return error;
    },

    async get(path) {
        const res = await fetch(this.base + path, { headers: this.headers() });
        if (!res.ok) {
            if (res.status === 401 && this.token) {
                this.logout();
                window.location.hash = 'login';
            }
            if (res.status === 403 && this.isAdmin) this.recheckAdmin();
            throw await this._requestError('GET', path, res);
        }
        return res.json();
    },

    async post(path, body) {
        const res = await fetch(this.base + path, {
            method: 'POST',
            headers: this.headers(),
            body: body ? JSON.stringify(body) : undefined
        });
        if (!res.ok) {
            if (res.status === 403 && this.isAdmin) this.recheckAdmin();
            throw await this._requestError('POST', path, res);
        }
        const text = await res.text();
        return text ? JSON.parse(text) : {};
    },

    async put(path, body) {
        const res = await fetch(this.base + path, {
            method: 'PUT',
            headers: this.headers(),
            body: body ? JSON.stringify(body) : undefined
        });
        if (!res.ok) {
            if (res.status === 403 && this.isAdmin) this.recheckAdmin();
            throw await this._requestError('PUT', path, res);
        }
        const text = await res.text();
        return text ? JSON.parse(text) : {};
    },

    async del(path) {
        const res = await fetch(this.base + path, {
            method: 'DELETE',
            headers: this.headers()
        });
        if (!res.ok) {
            if (res.status === 403 && this.isAdmin) this.recheckAdmin();
            throw await this._requestError('DELETE', path, res);
        }
        const text = await res.text();
        return text ? JSON.parse(text) : {};
    },

    // Re-reads /Users/Me without the cache; drops administrator rights if the account no longer has them.
    recheckAdmin() {
        if (this._adminCheck) return this._adminCheck;
        this._adminCheck = (async () => {
            try {
                const res = await fetch(this.base + '/Users/Me', { headers: this.headers(), cache: 'no-store' });
                if (res.ok) {
                    const user = await res.json();
                    const isAdmin = Boolean(
                        user.IsAdministrator ??
                        user.is_administrator ??
                        user.Policy?.IsAdministrator ??
                        user.policy?.is_administrator ??
                        false
                    );
                    if (!isAdmin && this.isAdmin) this._loseAdmin();
                }
            } catch (err) {
                console.warn('Admin re-check failed:', err);
            }
            return this.isAdmin;
        })().finally(() => { this._adminCheck = null; });
        return this._adminCheck;
    },

    // Drops administrator rights locally and forgets cached and in-flight responses, then tells the UI.
    _loseAdmin() {
        this.isAdmin = false;
        localStorage.setItem('rustyfin_is_admin', 'false');
        this._cacheGeneration++;
        this._cache.clear();
        this._pendingRequests.clear();
        if (this.onAdminLost) this.onAdminLost();
    },

    // Auth
    async login(username, password) {
        const res = await fetch(this.base + '/Users/AuthenticateByName', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-Emby-Authorization': this.authHeader()
            },
            body: JSON.stringify({ Username: username, Pw: password })
        });
        if (!res.ok) {
            const err = await res.json().catch(() => ({}));
            const error = new Error(err.Message || err.message || t('login.failed'));
            error.status = res.status;
            error.setupRequired = res.status === 401 && res.headers.get('X-Rustyfin-Setup-Required') === 'true';
            throw error;
        }
        const data = await res.json();
        this._storeSession(data, username);
        return data;
    },

    // Keeps the session that an AuthenticateByName-style answer carries (password, setup-code or OpenID Connect sign-in).
    _storeSession(data, fallbackName = '') {
        this.token = data.AccessToken || data.access_token;
        const userObj = data.User || data.user || {};
        this.userId = userObj.Id || userObj.id || '';
        this.userName = userObj.Name || userObj.name || fallbackName;
        this.isAdmin = Boolean(
            userObj.IsAdministrator ??
            userObj.is_administrator ??
            userObj.Policy?.IsAdministrator ??
            userObj.policy?.is_administrator ??
            false
        );

        localStorage.setItem('rustyfin_token', this.token);
        localStorage.setItem('rustyfin_user_id', this.userId);
        localStorage.setItem('rustyfin_user_name', this.userName);
        localStorage.setItem('rustyfin_is_admin', this.isAdmin.toString());
    },

    // Ends an OpenID Connect sign-in. The flow cookie from oidcStart() picks the flow. The device header is sent as login() sends it, so the token belongs to this browser.
    async oidcComplete() {
        const res = await fetch(this.base + '/Auth/Oidc/Complete', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-Emby-Authorization': this.authHeader()
            }
        });
        if (!res.ok) throw await this._requestError('POST', '/Auth/Oidc/Complete', res);
        this._storeSession(await res.json());
    },

    // Signs in an imported user with the one-time code and the password chosen now.
    async loginWithSetupCode(username, code, newPw) {
        const res = await fetch(this.base + '/Users/AuthenticateWithSetupCode', {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
                'X-Emby-Authorization': this.authHeader()
            },
            body: JSON.stringify({ Username: username, Code: code, NewPw: newPw })
        });
        if (!res.ok) throw await this._requestError('POST', '/Users/AuthenticateWithSetupCode', res);
        const data = await res.json();
        this._storeSession(data, username);
        return data;
    },

    logout() {
        if (this.token) {
            this.post('/Sessions/Logout').catch(() => {});
        }
        this.token = null;
        this.userId = null;
        this.userName = null;
        this.isAdmin = false;
        ['rustyfin_token', 'rustyfin_user_id', 'rustyfin_user_name', 'rustyfin_is_admin'].forEach(k => localStorage.removeItem(k));
    },

    isLoggedIn() {
        return !!this.token;
    },

    // OpenID Connect. Start and Link answer with the provider's AuthorizationUrl.
    oidcInfo() { return this.get('/Auth/Oidc/Info'); },
    oidcStart() { return this.post('/Auth/Oidc/Start'); },
    oidcLink() { return this.post('/Auth/Oidc/Link'); },
    oidcAccount(userId) { return this.get(`/Auth/Oidc/Accounts/${encodeURIComponent(userId)}`); },
    oidcAccounts() { return this.get('/Auth/Oidc/Accounts'); },
    oidcUnlink(userId) { return this.del(`/Auth/Oidc/Accounts/${encodeURIComponent(userId)}/Link`); },
    oidcSettings() { return this.get('/Auth/Oidc/Settings'); },
    saveOidcSettings(body) { return this.post('/Auth/Oidc/Settings', body); },

    // System
    getPublicInfo() { return this.get('/System/Info/Public'); },
    getFullInfo() { return this.get('/System/Info'); },
    getConfig() { return this.get('/System/Configuration'); },
    updateConfig(cfg) { return this.post('/System/Configuration', cfg); },
    getLogs() { return this.get('/System/Logs'); },
    getActivityLog(start = 0, limit = 50) {
        return this.get(`/System/ActivityLog/Entries?StartIndex=${start}&Limit=${limit}`);
    },
    shutdown() { return this.post('/System/Shutdown'); },
    restart() { return this.post('/System/Restart'); },

    // Startup / Setup Wizard
    getStartupConfig() { return this.get('/Startup/Configuration'); },
    updateStartupConfig(cfg) { return this.post('/Startup/Configuration', cfg); },
    getStartupUser() { return this.get('/Startup/User'); },
    createStartupUser(name, password) { return this.post('/Startup/User', { Name: name, Password: password }); },
    completeStartup() { return this.post('/Startup/Complete'); },

    // Libraries
    getLibraries() {
        if (this.isAdmin) return this.get('/Library/VirtualFolders');
        // /Library/VirtualFolders is for administrators; other users get the same libraries from their views.
        const types = { movies: 'Movies', tvshows: 'TvShows', music: 'Music', homevideos: 'Photos' };
        return this.get('/UserViews').then(views => (views.Items || []).map(v => ({
            id: v.Id,
            name: v.Name,
            content_type: types[v.CollectionType] || 'Mixed',
        })));
    },
    createLibrary(name, contentType, paths, libraryOptions) {
        const body = { Name: name, ContentType: contentType, Paths: paths };
        if (libraryOptions) body.LibraryOptions = libraryOptions;
        return this.post('/Library/VirtualFolders', body);
    },
    deleteLibrary(id) { return this.del(`/Library/VirtualFolders/${id}`); },
    getLibraryOptions(id) { return this.get(`/Library/${id}/Options`); },
    saveLibraryOptions(id, options) { return this.post(`/Library/${id}/Options`, options); },
    scanLibrary(id) { 
        this.invalidateCache('/Items');
        return this.post(`/Library/VirtualFolders/${id}/Refresh`); 
    },
    scanAllLibraries() { 
        this.invalidateCache('/Items');
        return this.post('/Library/Refresh'); 
    },
    browseDirectories(parent) {
        const q = parent ? `?parent=${encodeURIComponent(parent)}` : '';
        return this.get(`/Library/BrowseDirectories${q}`);
    },

    // Items
    getItems(params = {}) {
        const q = new URLSearchParams();
        Object.entries(params).forEach(([k, v]) => {
            if (v !== undefined && v !== null && v !== '') q.set(k, v);
        });
        return this.cachedGet('/Items?' + q.toString());
    },
    getItem(id) { return this.cachedGet(`/Items/${id}`); },
    getChildren(id, params = {}) {
        const q = new URLSearchParams(params);
        return this.cachedGet(`/Items/${id}/Children?${q.toString()}`);
    },
    getSimilar(id) { return this.cachedGet(`/Items/${id}/Similar`); },
    getLatest(limit = 20) { return this.cachedGet(`/Items/Latest?Limit=${limit}`); },
    getResume(limit = 12) { return this.cachedGet(`/Items/Resume?Limit=${limit}`); },
    getNextUp(limit = 12) { return this.cachedGet(`/Shows/NextUp?Limit=${limit}`); },
    getRecommendations() { return this.cachedGet('/Movies/Recommendations'); },
    getFilters() { return this.cachedGet('/Items/Filters', 60000); }, // 60s TTL for filters
    deleteItem(id) { return this.del(`/Items/${id}`); },
    refreshItem(id) { return this.post(`/Items/${id}/Refresh`); },

    // User Items
    getUserItems(params = {}) {
        const q = new URLSearchParams(params);
        return this.get(`/Users/${this.userId}/Items?${q.toString()}`);
    },
    getUserLatest(limit = 20) { return this.get(`/Users/${this.userId}/Items/Latest?Limit=${limit}`); },
    getUserResume(limit = 12) { return this.get(`/Users/${this.userId}/Items/Resume?Limit=${limit}`); },

    // Images
    imageUrl(itemId, type = 'Primary', opts = {}) {
        const q = new URLSearchParams(opts);
        if (this.token) q.set('api_key', this.token);
        return `${this.base}/Items/${itemId}/Images/${type}?${q.toString()}`;
    },

    // Playback
    reportPlaying(itemId, positionTicks = 0, fields = {}) {
        return this.post('/Sessions/Playing', { ItemId: itemId, PositionTicks: positionTicks, ...fields });
    },
    reportProgress(itemId, positionTicks, isPaused = false, fields = {}) {
        return this.post('/Sessions/Playing/Progress', { ItemId: itemId, PositionTicks: positionTicks, IsPaused: isPaused, ...fields });
    },
    reportStopped(itemId, positionTicks, fields = {}) {
        return this.post('/Sessions/Playing/Stopped', { ItemId: itemId, PositionTicks: positionTicks, ...fields });
    },
    markPlayed(itemId) { return this.post(`/Users/${this.userId}/PlayedItems/${itemId}`); },
    markUnplayed(itemId) { return this.del(`/Users/${this.userId}/PlayedItems/${itemId}`); },
    setFavorite(itemId) { return this.post(`/Users/${this.userId}/FavoriteItems/${itemId}`); },
    unsetFavorite(itemId) { return this.del(`/Users/${this.userId}/FavoriteItems/${itemId}`); },
    setRating(itemId, rating) { return this.put(`/Users/${this.userId}/Items/${itemId}/Rating`, { Rating: rating }); },

    // Streaming & Playback info
    streamUrl(itemId, type = 'Videos') {
        const q = new URLSearchParams();
        if (this.token) q.set('api_key', this.token);
        return `${this.base}/${type}/${itemId}/stream?${q.toString()}`;
    },
    getPlaybackInfo(itemId, type = 'Videos') { return this.get(`/${type}/${itemId}/PlaybackInfo`); },
    // Codecs this browser can play, probed once: MediaSource or a <video> element saying yes counts
    _playableCodecs: null,
    playableCodecs() {
        if (this._playableCodecs) return this._playableCodecs;
        const video = document.createElement('video');
        const canPlay = mime => (typeof MediaSource !== 'undefined' && MediaSource.isTypeSupported(mime))
            || video.canPlayType(mime) !== '';
        const probe = mimes => Object.keys(mimes).filter(name => canPlay(mimes[name]));
        this._playableCodecs = {
            video: probe({
                h264: 'video/mp4; codecs="avc1.640028"',
                hevc: 'video/mp4; codecs="hvc1.1.6.L120.90"',
                vp8: 'video/webm; codecs="vp8"',
                vp9: 'video/webm; codecs="vp09.00.10.08"',
                av1: 'video/mp4; codecs="av01.0.08M.08"',
            }),
            audio: probe({
                aac: 'audio/mp4; codecs="mp4a.40.2"',
                mp3: 'audio/mpeg',
                opus: 'audio/webm; codecs="opus"',
                flac: 'audio/flac',
                ac3: 'audio/mp4; codecs="ac-3"',
                eac3: 'audio/mp4; codecs="ec-3"',
            }),
        };
        return this._playableCodecs;
    },
    // Universal endpoint returns the optimal playback URL
    async getUniversalVideoUrl(itemId, params = {}) {
        const q = new URLSearchParams({
            MaxStreamingBitrate: params.maxBitrate || '100000000',
            EnableDirectPlay: params.enableDirectPlay !== false ? 'true' : 'false',
            EnableDirectStream: params.enableDirectStream !== false ? 'true' : 'false',
            PlaySessionId: 'web-' + (localStorage.getItem('rustyfin_device_id') || 'browser'),
        });
        const codecs = this.playableCodecs();
        q.set('VideoCodecs', codecs.video.join(','));
        q.set('AudioCodecs', codecs.audio.join(','));
        if (params.startTimeTicks > 0) q.set('StartTimeTicks', params.startTimeTicks);
        if (params.mediaSourceId) q.set('MediaSourceId', params.mediaSourceId);
        if (params.audioStreamIndex != null) q.set('AudioStreamIndex', params.audioStreamIndex);
        if (params.subtitleStreamIndex != null) q.set('SubtitleStreamIndex', params.subtitleStreamIndex);
        const data = await this.get(`/Videos/${itemId}/Universal?${q.toString()}`);
        // The player reads `url`; the server sends `Url`
        if (data && !data.url) data.url = data.Url;
        return data;
    },

    // Stops the transcode of one play session; keepalive lets the request outlive the page
    async stopActiveEncodings(playSessionId) {
        const path = `/Videos/ActiveEncodings?PlaySessionId=${encodeURIComponent(playSessionId)}`;
        const res = await fetch(this.base + path, { method: 'DELETE', headers: this.headers(), keepalive: true });
        if (!res.ok) throw await this._requestError('DELETE', path, res);
    },

    // Search
    search(term, limit = 30) {
        return this.get(`/Search/Hints?SearchTerm=${encodeURIComponent(term)}&Limit=${limit}`);
    },

    // Users (admin)
    getUsers() { return this.get('/Users'); },
    getMe() { return this.get('/Users/Me'); },
    createUser(name, password, isAdministrator = false) {
        return this.post('/Users/New', { Name: name, Password: password, IsAdministrator: isAdministrator });
    },
    deleteUser(id) { return this.post(`/Users/${id}/Delete`); },
    changePassword(id, current, newPw) { return this.post(`/Users/${id}/Password`, { CurrentPw: current, NewPw: newPw }); },
    updatePolicy(id, policy) { return this.post(`/Users/${id}/Policy`, policy); },

    // Sessions
    getSessions() { return this.get('/Sessions'); },

    // Collections & Playlists
    getCollections() { return this.get('/Collections'); },
    createCollection(name, ids) { return this.post('/Collections', { Name: name, Ids: ids }); },
    getPlaylists() { return this.get('/Playlists'); },
    createPlaylist(name, ids) { return this.post('/Playlists', { Name: name, ItemIds: ids }); },
    getPlaylistItems(id) { return this.get(`/Playlists/${id}/Items`); },

    // Scheduled Tasks
    getTasks() { return this.get('/ScheduledTasks'); },
    startTask(id) { return this.post(`/ScheduledTasks/Running/${id}`); },
    stopTask(id) { return this.del(`/ScheduledTasks/Running/${id}`); },

    // Plugins
    getPlugins() { return this.get('/Plugins'); },

    // Genres, Artists, Studios, Persons
    getGenres() { return this.get('/Genres'); },
    getArtists() { return this.get('/Artists'); },
    getStudios() { return this.get('/Studios'); },
    getPersons() { return this.get('/Persons'); },
    getPerson(id) { return this.get(`/Persons/${id}`); },
    getPersonItems(id) { return this.get(`/Persons/${id}/Items`); },

    // Live Log
    getLogLevel() { return this.get('/System/LogLevel'); },
    setLogLevel(level) { return this.post('/System/LogLevel', { LogLevel: level }); },

    // Jellyfin import (B20)
    jellyfinImportPreview(req) { return this.post('/JellyfinImport/Preview', req); },
    jellyfinImportRun(req) { return this.post('/JellyfinImport/Run', req); },
    jellyfinImportStatus() { return this.get('/JellyfinImport/Status'); },
    jellyfinImportPendingUsers() { return this.get('/JellyfinImport/PendingUsers'); },
    jellyfinImportNewCode(userId) { return this.post('/JellyfinImport/Users/' + encodeURIComponent(userId) + '/SetupCode'); }
};

if (typeof window !== 'undefined') {
    window.API = API;
}

// ── WebSocket Live Log Client ──
const LiveLog = {
    ws: null,
    messages: [],
    maxMessages: 500,
    callbacks: [],
    connected: false,

    connect() {
        if (this.ws && this.ws.readyState <= 1) return;
        const token = localStorage.getItem('rustyfin_token');
        if (!token) return;

        const proto = window.location.protocol === 'https:' ? 'wss:' : 'ws:';
        const url = `${proto}//${window.location.host}/socket?authorization=Bearer ${token}`;

        try {
            this.ws = new WebSocket(url);

            this.ws.onopen = () => {
                this.connected = true;
            };

            this.ws.onmessage = (evt) => {
                try {
                    const data = JSON.parse(evt.data);
                    if (data.MessageType === 'LogMessage' && data.Data) {
                        this.messages.push(data.Data);
                        if (this.messages.length > this.maxMessages) {
                            this.messages.shift();
                        }
                        this.callbacks.forEach(cb => cb(data.Data));
                    }
                } catch {}
            };

            this.ws.onclose = () => {
                this.connected = false;
                // Reconnect after 5 seconds
                setTimeout(() => this.connect(), 5000);
            };

            this.ws.onerror = () => {
                this.connected = false;
            };
        } catch {}
    },

    onMessage(cb) {
        this.callbacks.push(cb);
    },

    disconnect() {
        if (this.ws) {
            this.ws.close();
            this.ws = null;
            this.connected = false;
        }
    }
};

if (typeof window !== 'undefined') {
    window.LiveLog = LiveLog;
}
