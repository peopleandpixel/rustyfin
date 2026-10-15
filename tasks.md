# Rustyfin – Tasks bis zur Jellyfin-Kompatibilität

## Phase 1: Critical for Client Connection

### MediaSources & MediaStreams (Highest Priority)
- [x] Add `MediaSources` array to `MediaItem` DTO (container, protocol, bitrate, codecs, paths)
- [x] Add `MediaStreams` array to each media source (video, audio, subtitle tracks with codec, language, channels)
- [x] Implement FFprobe-based media analysis during library scanning
- [x] Add `MediaSources` and `MediaStreams` database tables
- [x] Probe media files to extract codec, bitrate, resolution, duration, stream info, chapter markers
- [x] Add media source detection for multiple versions of the same title

### HLS Transcoding Fixes
- [x] Fix HLS URL pattern: `/Videos/{id}/master.m3u8` → `/Videos/{id}/main.m3u8` (Jellyfin standard)
- [x] Implement Jellyfin HLS URL pattern: `/Videos/{id}/hls/{playbackId}/{type}/{segmentId}.{ext}`
- [x] Add `playbackId` generation and tracking
- [x] Generate proper HLS playlist with `#EXT-X-MEDIA` tags for multi-audio/subtitle groups
- [x] Implement multi-audio stream HLS (audio_stream_index support)
- [x] Implement multi-subtitle stream HLS (subtitle stream index support)
- [x] Implement adaptive bitrate with multiple quality levels in HLS (profile flag added)
- [x] Implement audio-only transcoding

### Audio Streaming
- [x] Implement `GET /Audio/{id}/universal` endpoint (primary music playback endpoint)
- [x] Return `StreamingResponse` DTO with direct play URL, transcode URL, media streams
- [x] Implement audio probing: extract album, artist, genre, track number from metadata
- [x] Implement music library structure: Artist → Album → Track hierarchy
- [x] Extract and embed album art from audio files
- [x] Implement `GET /Audio/{id}/PlaybackInfo` endpoint

### Video Streaming
- [x] Implement `GET /Videos/{id}/progressive.stream` endpoint (direct progressive download)
- [x] Implement `PlaybackInfo` endpoint: `GET /Audio/{id}/PlaybackInfo`
- [x] Implement `PlaybackInfo` endpoint: `GET /Videos/{id}/PlaybackInfo`
- [x] Implement direct play detection based on client codec support

### MediaItem DTO Fields
- [x] Add `UserData` field to every item response (per-user playback state embedded in items)
- [x] Add `ImageTags` map (image type to tag/hash for caching)
- [x] Add `BackdropImageTags` array
- [x] Add `ParentBackdropImageTags` array
- [x] Add `PrimaryImageAspectRatio` field
- [x] Add `CanDelete` and `CanDownload` permission flags
- [x] Add `Container`, `Bitrate`, `Video3D` fields
- [x] Add `PremiereDate` (full datetime)
- [x] Add `EndDate` (for series/seasons)
- [x] Add `SeriesName`, `SeriesId`, `SeasonName` (for episodes)
- [x] Add `Status` (series status: Continuing, Ended, etc.)
- [x] Add `OfficialRating`
- [x] Add `CustomRating`, `CriticRating`, `AudienceRating`
- [x] Add `Tags` (user-defined tags)
- [x] Add `People` array (cast/crew with roles)
- [x] Add `Studios` array
- [x] Add `GenreItems` (genre objects with IDs)
- [x] Add `ForcedSortName`, `Width`, `Height`, `AspectRatio`
- [x] Add `ChapterCount`, `MediaSourceCount`
- [x] Add `LocationType` (`FileSystem`, `Virtual`, `Remote`)
- [x] Add `IntroStartTimeTicks`, `IntroEndTimeTicks` (intro detection)
- [x] Add `IsHD`, `Is4K` flags
- [x] Add `VideoType` (`VideoFile`, `Iso`, `Ifo`)

### Items Query Parameters
- [x] Implement `Fields` query parameter for lazy loading (e.g., `PrimaryImageAspectRatio,BasicSyncInfo,CanDelete,MediaSourceCount`)
- [x] Implement `Recursive` query parameter
- [x] Implement `ImageTypesLimit` query parameter
- [x] Implement `IsFavorite`, `IsUnplayed`, `IsPlayed` filters
- [x] Implement `IsMovie`, `IsSeries`, `IsPeople` filters
- [x] Implement `HasSubtitles`, `MediaTypes` filters
- [x] Implement `GenreIds`, `StudioIds`, `ArtistIds`, `AlbumIds` filters
- [x] Implement `Tags`, `Features`, `OfficialRatings`, `Years` filters
- [x] Implement `StartDate`, `EndDate`, `PremiereDate`, `DateCreated`, `Runtime` filters
- [x] Implement `VideoCodecs`, `AudioCodecs`, `AudioBitrate`, `AudioChannels` filters
- [x] Implement `HasThemeSong`, `HasSpecialFeature`, `SubtitleLanguage` filters
- [x] Implement `EnableAutoHide`, `CollapseBoxSetItems`, `GroupProjects` flags
- [x] Complete `GET /Items/Resume` with real data (currently returns empty array)
- [x] Complete `GET /Users/{id}/Items/Resume` with real data (currently returns empty array)

### Network Discovery
- [x] Implement UDP discovery on port 7359 for local network auto-discovery
- [x] Implement mDNS/Bonjour `_jellyfin._tcp` service discovery (optional)

### WebSocket
- [x] Require authentication token for WebSocket connection
- [x] Implement event types: `PlaybackStart`, `PlaybackStop`, `PlaybackProgress`
- [x] Implement event types: `LibraryChanged`, `ScheduledTaskStarted`, `ScheduledTaskCompleted`
- [x] Implement event types: `SessionChanged`, `UserUpdated`
- [x] Push remote commands to active sessions via WebSocket
- [x] Complete `POST /Sessions/{id}/Command` to deliver commands via WebSocket (currently logs only)

## Phase 2: Web UI Compatibility

### Playstate Controller
- [x] Implement `POST /Playstate/PinEpisode` – pin current episode for a series
- [x] Implement `POST /Playstate/LastActivityDate` – update last activity date for items
- [x] Implement `PUT /Users/{userId}/Items/{itemId}/Rating` – set user rating for an item
- [x] Add `user_ratings` database table for per-user item ratings (1-5 stars)

### Configuration Controller
- [x] Implement `GET /Configuration/MediaEncoding` – transcoding paths, hardware acceleration settings
- [x] Implement `PUT /Configuration/MediaEncoding` – save encoding config
- [x] Implement `GET /Configuration/Networking` – published server URI, remote network bind addresses
- [x] Implement `PUT /Configuration/Networking` – save networking config
- [x] Implement `GET /Configuration/ServerRecoding` – server recording config
- [x] Implement `GET /Configuration/Branding` – branding configuration (via `/Branding/Configuration`)
- [x] Implement `GET /Configuration/Display` – display configuration
- [x] Implement `GET /Configuration/ParentalControls` – parental controls config
- [x] Add `configuration` database table for server configuration

### Encoding Manager Controller
- [x] Implement `GET /System/MediaEncoder/Path` – get FFmpeg path
- [x] Implement `PUT /System/MediaEncoder/Path` – set FFmpeg path
- [x] Implement `POST /System/MediaEncoder/Path/Validate` – validate FFmpeg path
- [x] Implement `POST /System/MediaEncoder/Test` – test FFmpeg with sample video
- [x] Add `media_encoder` database table

### Shows Endpoints
- [x] Complete `GET /Shows/NextUp` with real data (playback history logic)
- [x] Complete `GET /Shows/Upcoming` with real data (air date logic)

### Search
- [x] Implement `GET /Search` – full search returning items (not just hints)
- [x] Add search with `IncludeItemTypes`, `Limit`, `StartDate`, `EndDate` params

### Persons
- [x] Complete `GET /Persons` with real data (currently returns empty array)
- [x] Complete `GET /Persons/{id}` with real data (currently returns hardcoded "Unknown")
- [x] Implement `GET /Persons/{id}/Items` – items related to a person
- [x] Add `people` and `item_people` database tables
- [x] Download TMDb person photos (actor/director headshots)

### Collections & Playlists
- [x] Implement `GET /Collections/{id}` – get single collection
- [x] Implement `DELETE /Collections/{id}` – delete collection
- [x] Implement `GET /Collections/{id}/Items` – get collection items
- [x] Implement `POST /Collections/{id}/Items/{itemId}` – add item to collection
- [x] Implement `DELETE /Collections/{id}/Items/{itemId}` – remove item from collection
- [x] Implement `GET /Playlists` – list playlists
- [x] Implement `GET /Playlists/{id}` – get single playlist
- [x] Implement `DELETE /Playlists/{id}` – delete playlist

### System Info
- [x] Add `RemoteEndPoint`, `LocalAddresses`, `StartupWizardCompleted` to `GET /System/Info`
- [x] Implement `GET /System/Log` – get log content
- [x] Implement `GET /System/keys` – public keys endpoint
- [x] Add `PackageVersion` field to server info

### Users
- [x] Implement `GET /Users/{id}/Images` – user avatar images
- [x] Implement `GET /Users/{id}/Images/{imageType}` – get/set user images
- [x] Implement `GET /Users/{id}/SecurityInfo` – user security information
- [x] Implement `PUT /Users/{id}/Update` – update user name
- [x] Implement `POST /Users/{id}/EasyPassword` – easy PIN password
- [x] Implement `GET /Users/{id}/Preferences` – user preferences
- [x] Implement `GET /Users/{id}/Periods` – parental control periods
- [x] Add `display_preferences` database table

### Quick Connect
- [x] Add WebSocket notification when code is authorized
- [x] Return token in Connect response when authenticated

### Subtitles
- [x] Complete remote subtitle search (currently returns empty array)
- [x] Implement `GET /Videos/{id}/Subtitles/{index}/Stream` – subtitle stream endpoint
- [x] Implement subtitle download and embedding

### Media Segments
- [x] Complete intro/outro detection (currently returns empty)
  - Database table `media_segments` with segment type, start/end ticks
  - FFmpeg-based detection using `silencedetect` and `blackdetect` filters
  - API endpoint `GET /MediaSegments/{itemId}` returns real data
  - Scheduled task for batch detection across all episodes
  - Unit tests for parsing and detection logic

### Session Management
- [x] Create session on user login (sessions not created when users authenticate)
- [x] Implement `GET /Sessions/Playing/NowPlayingItem` – get now playing item for a session
- [x] Update session's `now_playing_item` in real-time during playback progress

## Phase 3: Feature Parity

### Chapter Controller
- [x] Implement `GET /Videos/{id}/Chapters` – chapter navigation
- [x] Add `chapters` database table
- [x] Extract chapter markers from media files via FFprobe

### Trickplay
- [x] Implement `GET /Items/{id}/Trickplay/Preview/{width}/{seekTicks}` – preview thumbnails for scrubbing
- [x] Add `trickplay` database table
- [x] Generate trickplay thumbnail strips

### DLNA Profile Controller
- [x] Implement `GET /Dlna/Profile` – DLNA profile management
- [x] Implement `GET /Dlna/Profile/DeviceDetection` – device detection
- [x] Implement `GET /Dlna/Profile/Device` – device-specific profiles
- [x] Implement device detection to identify connecting devices
- [x] Implement profile-based transcoding based on device capabilities

### Environment Controller
- [x] Implement `GET /Environment/NetworkShares` – network share browsing
- [x] Implement `GET /Environment/Drives` – drive listing

### Metadata Enhancements
- [x] Download backdrop images from TMDb
- [x] Parse local Kodi-compatible NFO files for metadata
- [x] Implement theme song detection
- [x] Implement intro/outro detection

### Configuration
- [x] Implement parental controls: rating levels, blocked words, access schedules
- [x] Implement library options: per-library metadata providers, image providers, skip subfolders

### Authentication
- [x] Implement device registration on first login
- [x] Implement JWT token refresh mechanism
- [x] Complete password reset PIN flow (currently stub)

## Phase 4: Polish & Advanced Features

### LiveTV (Stub Endpoints)
- [x] Implement `GET /LiveTv/Channels` – live TV channels (stub, returns empty)
- [x] Implement `GET /LiveTv/Recordings` – recordings (stub, returns empty)
- [x] Implement `GET /LiveTv/Guides` – guide data (stub, returns empty)
- [x] Implement `GET /LiveTv/SeriesTimers` – series timers (stub, returns empty)

### Additional Controllers
- [x] Implement `GET /Movies/Recommendations` – movie recommendations
- [x] Implement `GET /Movies/Featured` – featured movies
- [x] Implement `GET /Users/{id}/Items/{id}/InstantMix` – instant mix (uses similar items)
- [x] Implement `GET /Genres/Music` – music genres
- [x] Implement `POST /Devices/{id}/Command` – remote commands via HTTP

### API Compatibility
- [x] Ensure all responses use `QueryResult` with `StartIndex` where Jellyfin expects it (LiveTV, additional controllers)
- [x] Use Jellyfin's exact error response format with `Error` object (Code, Message, StackTrace) (already implemented)
- [x] Ensure all dates use ISO 8601 with `Z` suffix (chrono default)
- [x] Ensure UUID serialization matches Jellyfin GUID format (uuid crate default)
- [x] Ensure ALL JSON fields are consistently PascalCase (verified)

### Server Bootstrap
- [x] Validate FFmpeg exists and is accessible at startup
- [x] Create required data directories (cache, logs, transcodes, trickplay) at startup
- [x] Handle SIGHUP for config reload (Unix-only)

### Performance & Security Optimizations (Prioritized by Impact)

> **Status:** P1, P2, P3, P4, P5, P6, P7, P15, P16 completed. Remaining items still valid.

#### P1 — Batch Media Sources & Streams Query (5-10x browse speedup) 🔴 CRITICAL
- [x] Replace N+1 `populate_media_sources` loop in `items.rs` with a single batched `IN (...)` query
- [x] Fetch all sources and streams for all items in one round-trip, then assemble in memory
- [x] **Nested N+1:** `get_media_sources` in `repo_media_source.rs` also loops per-source to fetch streams — batch this too
- [x] Affected: all API endpoints returning item lists (browse, search, latest, children, resume, similar)
- [x] A page with 50 items now fires **1 query** instead of 100-150 sequential DB round-trips

#### P2 — In-Memory Path Set for Scanner (10-50x scan speedup) 🔴 CRITICAL
- [x] Before scanning, load all existing paths into a `HashSet<String>` in memory via `get_library_paths()`
- [x] Replace per-file DB existence check (`get_item_by_path`) with `set.contains(&file_path)` in all 3 scanners
- [x] **Also apply to:** TV library scanner (`scan_tv_library`) and music library scanner (`scan_music_library`) — both have the same pattern
- [x] Series/season/artist/album lookups also use the in-memory set first, falling back to DB only when needed
- [x] A library with 10,000 files now fires **1 query** to load paths instead of 10,000 sequential existence checks

#### P3 — In-Memory Response Cache (5-20x repeated reads speedup) 🟠 HIGH
- [x] Add `moka` crate for async LRU caching — `ApiCache` + `ImageCache` in `cache.rs`
- [x] Cache library views (change only on scan) — 128-entry LRU, 60s TTL
- [x] Cache item details (change only on metadata refresh) — 1024-entry LRU, 30s TTL
- [x] Cache filter lists (change only on scan) — 128-entry LRU, 120s TTL
- [x] Cache search results with short TTL — 256-entry LRU, 30s TTL + 10s idle
- [x] Share `ApiCache` + `ImageCache` across request handlers via `AppState`
- [x] **Also cache:** image proxy results in memory — 512-entry LRU, 5min TTL
- [x] Cache invalidation on scan (`invalidate_on_scan`) and metadata refresh (`invalidate_item`)

#### P4 — Native Enum/Date Types — Eliminate JSON Serialization (2-3x all queries) 🟠 HIGH
- [x] Store `media_type` as plain TEXT enum (`"Movie"`, `"Episode"`) — uses `MediaType::item_kind()` for storage, `FromStr` for parsing
- [x] Store genres as delimited string (`"Action|Sci-Fi"`) — `join("|")` on write, `split('|')` on read
- [x] DateTime fields use RFC3339 strings (already the standard SQLite date format — no change needed)
- [x] Removed all `serde_json::to_string` / `from_str` calls from `repo_media.rs` and `scanner.rs`
- [x] Added `FromStr` and `Display` impls to `MediaType` enum
- [x] Created migration `013_native_types.sql` to convert existing data from JSON to native format
- [x] Updated all SQL queries: `get_next_up`, `get_next_up_for_series`, `get_upcoming`, `get_upcoming_for_series`, `get_recommendations`, `get_featured_movies`, `get_music_genres`, `get_filters`
- [x] Updated `get_filters` and `get_music_genres` to parse delimited genres instead of JSON arrays

#### P5 — HTTP Response Compression (2-5x network latency) 🟠 HIGH
- [x] Add `tower_http::CompressionLayer` to Axum middleware stack in `main.rs`
- [x] JSON responses compress 5-10x with gzip/zstd
- [x] **Confirmed:** `CompressionLayer` added before `CorsLayer` and `TraceLayer`

#### P6 — Batch Scanner Inserts in Transaction (10-100x scan writes) 🟠 HIGH
- [x] Added `batch_upsert_media_items()` to `repo_media.rs` — single transaction with batched INSERT OR REPLACE
- [x] Modified `scan_flat_library` to collect items in Vec, then batch-insert in one transaction
- [x] Modified `scan_tv_library` to collect series/seasons/episodes, then batch-insert in one transaction per path
- [x] Modified `scan_music_library` to collect artists/albums/tracks, then batch-insert in one transaction per path
- [x] Added `insert_media_source_with_streams()` to `repo_media_source.rs` — single transaction for source + all streams
- [x] Updated `probe_and_add_source_internal` to use `insert_media_source_with_streams` instead of separate INSERT calls
- [x] Reduces WAL contention and speeds up bulk inserts dramatically

#### P7 — Replace `ORDER BY RANDOM()` (10-100x similar items) 🟡 MEDIUM
- [x] In `get_similar_items`, replaced `ORDER BY RANDOM()` with ROWID-based random offset
- [x] Uses `fastrand` to generate random offset, then `ORDER BY ROWID LIMIT ? OFFSET ?`
- [x] **Also in:** `get_items_filtered` — `sort_by = "Random"` now uses ROWID with random offset

#### P8 — TV Episodes Single-Query Fetch (3-5x episode lists) 🟡 MEDIUM
- [x] Added `get_episodes_for_series()` to `repo_media.rs` — single query using subquery: `WHERE parent_id IN (SELECT id FROM media_items WHERE parent_id = ? AND media_type = 'Season')`
- [x] Updated `get_episodes` in `shows.rs` to use the single-query function instead of the N+1 loop
- [x] Supports optional `season_number` filter to fetch episodes for a specific season
- [x] Eliminates N+1 round-trips: a series with 8 seasons now fires 1 query instead of 9

#### P9 — UUID as BLOB(16) or Integer (1.5-2x all lookups) 🟡 MEDIUM — SKIPPED
- [x] **SKIPPED:** Massive effort for marginal gain — SQLite TEXT comparison on 36-char UUIDs is already fast with WAL mode
- [x] **SKIPPED:** Jellyfin API contract expects UUIDs; switching internally would require a translation layer
- [x] **SKIPPED:** BLOB(16) only matters at millions of rows; media servers typically have ~50k items

#### P10 — Bounded ffprobe Concurrency (2-4x probe phase) 🟡 MEDIUM
- [x] Added global `Semaphore` in `probe.rs` with max 4 concurrent probes
- [x] Created `spawn_probe_and_save()` and `spawn_probe_and_add_source()` wrapper functions
- [x] Updated all 5 scanner.rs locations to use bounded spawn functions instead of raw `tokio::spawn`
- [x] Prevents process spawn overload during large library scans — excess probes queue behind the semaphore

#### P11 — Dynamic SQL Query Plan Caching 🟡 MEDIUM
- [x] Rewrote `get_items_filtered` — single-value params use `(? IS NULL OR ? = '' OR col = ?)` static pattern; array params use a query plan cache keyed by filter array sizes
- [x] Added `FILTER_QUERY_CACHE` (`LazyLock<Mutex<HashMap>>`) that maps filter shape to pre-built SQL strings — same array sizes reuse the same plan
- [x] Removed `BindParam` enum — all binds are now direct `sqlx::Query` chains
- [x] Rewrote `get_latest_items` — static SQL with `(? IS NULL OR ? = '' OR col = ?)` pattern, no string concatenation
- [x] Rewrote `get_filters` — static SQL for genres, years, and ratings queries, no if/else string concatenation
- [x] Eliminates per-request SQL re-parsing overhead for the most frequently called query functions

#### P12 — Genres Filter Optimization 🟡 MEDIUM
- [x] Replaced `LIKE %genre%` with exact delimited matching — checks 4 boundary positions: exact match, start (`genre|%`), middle (`%|genre`), end (`%|genre|%`)
- [x] Applied the same fix to the tags filter, eliminating false positives (e.g., "Action" no longer matches "Action-Adventure")
- [x] Added 4 new indexes in migration 014: composite `(library_id, genres)`, `(library_id, tags)`, and single-column `genres`, `tags`
- [x] Updated both count and data query bind parameters to pass 4 values per genre/tag
- [x] Eliminates full table scans on genre/tags filters — indexes narrow results before pattern matching

#### P13 — Async Directory Traversal 🟢 LOW
- [x] Replaced synchronous `WalkDir` in all 3 scanner functions with `tokio::fs::read_dir` using an iterative work-queue approach
- [x] Removed the `walkdir` dependency from `Cargo.toml`
- [x] Eliminates blocking the Tokio runtime during large library scans

#### P14 — Read-Only Connection Pool 🟢 LOW
- [x] Added `init_read_only_db()` function in `rustyfin-db` with `PRAGMA query_only = ON`
- [x] Added `db_ro: SqlitePool` field to `AppState`
- [x] Initialized the read-only pool in `main.rs` alongside the writable pool
- [x] Updated all read-only GET endpoints across 17+ route files to use `state.db_ro`
- [x] Write operations (POST/PUT/DELETE) continue to use `state.db`
- [x] Eliminates write-lock overhead on query-heavy operations like item browsing, search, and library views

#### P15 — SQL Injection Vulnerability Fix 🔴 CRITICAL SECURITY
- [x] In `repo_media.rs` `get_items_filtered`, genres, search_term, and tags are now **parameterized** with `bind()` calls
- [x] Date filters (`start_date`, `end_date`, `premiere_date`, `date_created`) are now parameterized
- [x] Replaced all string interpolation with proper parameterized `bind()` calls
- [x] **Security vulnerability fixed**

#### P16 — Concurrent Metadata Refresh (5-10x scan+metadata speedup) 🔴 CRITICAL
- [x] In `scanner.rs` `scan_library_with_metadata`, metadata refresh now uses **bounded concurrency** (semaphore, max 5)
- [x] Each refresh runs concurrently with semaphore limiting to 5 simultaneous HTTP requests
- [x] All tasks awaited after spawning to ensure completion before returning

#### P17 — Batch Image Existence Checks (3-5x metadata speedup) 🟠 HIGH
- [x] In `metadata.rs` `store_provider_images`, each image is checked via `get_image` before inserting — N+1 pattern
- [x] In `metadata.rs` NFO image handling, same pattern: check then insert per image
- [x] Batch-check all existing images for an item in one query, then batch-insert missing ones
- [x] Single query: `SELECT item_id, image_type FROM item_images WHERE item_id = ?` then check in-memory

#### P18 — Scanner Series/Season Lookup Optimization (3-10x TV scan speedup) 🟠 HIGH
- [x] In `scan_tv_library`, series and season existence checks use `get_item_by_path` per-file during WalkDir iteration
- [x] While `series_map`/`season_map` HashMaps cache in-memory, the initial DB lookup for each new series/season is still a per-call query
- [x] Before scanning, pre-load all existing TV series and season paths into a `HashMap<String, Uuid>`
- [x] Same pattern exists in `scan_music_library` for artist/album lookups

#### P19 — In-Memory Image Cache (2-3x image serve speedup) 🟡 MEDIUM
- [x] Image proxy (`image_proxy.rs`) currently only disk-caches resized images
- [x] Frequently accessed images (posters, backdrops) are read from disk on every request
- [x] Add in-memory LRU cache for small images (<1MB) with TTL
- [x] Share cache across requests via `AppState`

---

## SQLite Evaluation — Is It Still the Best Database?

**Verdict: Yes, SQLite remains the best choice for this project.** After re-evaluating the current codebase:

### Current Workload Profile
- **12 tables** with ~40 columns total across media_streams
- **Read-heavy:** browsing, search, filtering dominate (90%+ reads)
- **Write patterns:** periodic bulk writes during library scans, occasional single writes (playback state, favorites)
- **Concurrency:** single-server deployment, 10 connection pool max
- **Query patterns:** paginated queries, parent-child hierarchies, FTS5 full-text search, simple key-lookups by UUID
- **Deployment:** single Docker container, single-file DB

### Why SQLite Is Still Optimal
1. **WAL mode is already enabled** — provides excellent concurrent read performance (multiple readers + single writer)
2. **Optimizations from migration 007** — 17 targeted composite indexes cover the most common query patterns
3. **Connection pool tuned** — 10 max connections, 64MB cache per connection, shared cache, memory-mapped I/O
4. **Single-container deployment** — no separate database process to manage, simplifies Docker setup
5. **Media server scale** — libraries up to ~50,000 items work comfortably with SQLite (Jellyfin/Emby historically used it)
6. **Zero network overhead** — all queries are local file I/O, not network round-trips

### When to Consider PostgreSQL
- Library grows beyond **50k-100k items**
- Noticeable **write contention** during library scans with concurrent user activity
- Need **native JSONB** to eliminate all `serde_json` serialization overhead (P4)
- Need **superior full-text search** (PostgreSQL's tsvector > FTS5)
- Plan for **multi-server/distributed** deployment

### Quick Wins Regardless of Database
The top optimizations (P1-P3, P5, P15-P18) will provide **massive speedups regardless of database choice**. The N+1 query problems, missing compression, SQL injection, and sequential metadata refresh are application-level issues that would exist with any database.

**Recommendation:** Focus on implementing P1-P19 first. Only reconsider PostgreSQL after those are done and you still hit performance walls.

### Client Compatibility Testing
- [ ] Test with Jellyfin Android App
- [ ] Test with Jellyfin iOS App
- [ ] Test with Jellyfin TV (Android TV, Fire TV)
- [ ] Test with Swiftfin (Apple TV)
- [ ] Test with Findroid (Android)
- [ ] Test with Sailfin
- [ ] Implement client capability reporting

---

## Completed Items

### Phase 1: API-Kompatibilität (Grundlagen) ✓

#### System & Server
- [x] `GET /System/Info/Public` – Öffentliche Server-Infos
- [x] `GET /System/Info` – Vollständige Server-Infos (authentifiziert)
- [x] `POST /System/Shutdown` – Server herunterfahren
- [x] `POST /System/Restart` – Server neustarten
- [x] `GET /System/Configuration` – Server-Konfiguration abrufen
- [x] `POST /System/Configuration` – Server-Konfiguration ändern
- [x] `GET /System/Logs` – Server-Logs abrufen
- [x] `GET /System/Ping` – Ping-Endpoint

#### Authentifizierung & Benutzer
- [x] `POST /Users/AuthenticateByName` – Login
- [x] `GET /Users` – Benutzerliste
- [x] `GET /Users/Me` – Aktueller Benutzer
- [x] `GET /Users/{id}` – Benutzer nach ID
- [x] `POST /Users/New` – Benutzer erstellen
- [x] `POST /Users/{id}/Delete` – Benutzer löschen
- [x] `POST /Users/{id}/Password` – Passwort ändern
- [x] `POST /Users/{id}/Policy` – Benutzerrichtlinien setzen (IsAdmin, etc.)
- [x] `POST /Users/{id}/Configuration` – Benutzereinstellungen
- [x] `GET /Users/Public` – Öffentliche Benutzerliste (Login-Screen)
- [x] `POST /Users/ForgotPassword` – Passwort vergessen (Stub)
- [x] API-Key-Authentifizierung (neben JWT, via RUSTYFIN_API_KEY)
- [x] Quick Connect Support (`/QuickConnect/Enabled`, `/Initiate`, `/Connect`, `/Authorize`)

#### Bibliothek & Medien
- [x] `GET /Library/VirtualFolders` – Bibliotheken auflisten
- [x] `POST /Library/VirtualFolders` – Bibliothek erstellen
- [x] `DELETE /Library/VirtualFolders` – Bibliothek löschen
- [x] `POST /Library/VirtualFolders/LibraryOptions` – Bibliothek-Scan
- [x] `POST /Library/Refresh` – Alle Bibliotheken neu scannen
- [x] `GET /Items` – Medien abfragen (Paginierung, Sortierung, Filter)
- [x] `GET /Items/{id}` – Einzelnes Medium abrufen
- [x] `GET /Items/{id}/Similar` – Ähnliche Medien
- [x] `DELETE /Items/{id}` – Medium löschen
- [x] `GET /Items/Latest` – Zuletzt hinzugefügte Medien
- [x] `GET /Items/Resume` – Weiterschauen-Liste
- [x] `GET /Items/Filters` – Verfügbare Filter (Genres, Jahre, etc.)
- [x] `GET /Users/{id}/Items` – Medien mit Benutzer-spezifischem Status
- [x] `GET /Users/{id}/Items/Latest` – Zuletzt hinzugefügt pro Benutzer
- [x] `GET /Users/{id}/Items/Resume` – Weiterschauen pro Benutzer
- [x] Rekursive Ordner-Struktur (Serien → Staffeln → Episoden)
- [x] Medientyp-Erkennung (Film vs. Serie vs. Musik vs. Foto)

#### Metadaten
- [x] `GET /Items/{id}/Images` – Bilder eines Mediums
- [x] `GET /Items/{id}/Images/{type}` – Bestimmtes Bild abrufen (Primary, Backdrop, etc.)
- [x] `POST /Items/{id}/Images/{type}` – Bild hochladen
- [x] Image-Proxy mit Caching und Resize
- [x] TMDb-Provider (Filme & Serien)
- [x] OMDb-Provider
- [x] Lokale NFO-Datei-Parser
- [x] Lokale Bilder erkennen (poster.jpg, fanart.jpg, etc.)
- [x] `POST /Items/{id}/Refresh` – Metadaten neu laden
- [x] Automatischer Metadaten-Abruf beim Library-Scan

### Phase 2: Streaming & Wiedergabe ✓

#### Direct Play / Direct Stream
- [x] `GET /Videos/{id}/stream` – Video-Datei direkt streamen
- [x] `GET /Audio/{id}/stream` – Audio-Datei direkt streamen
- [x] Range-Request-Support (Seeking)
- [x] `HEAD`-Requests für Medien-Streams
- [x] Content-Type-Erkennung (video/mp4, audio/flac, etc.)

#### Transkodierung
- [x] `GET /Videos/{id}/master.m3u8` – HLS-Transkodierung
- [x] FFmpeg-Prozess-Management (Start, Stop, Cleanup)
- [x] Transkodierungs-Profile (Bitrate, Codec, Auflösung)
- [x] Hardware-Beschleunigung (VAAPI, NVENC, QSV)
- [x] Untertitel-Einbrennung (Burn-in)
- [x] Segment-basiertes Streaming (HLS-Segmente)
- [x] Transkodierungs-Cache-Management

#### Wiedergabe-Status
- [x] `POST /Sessions/Playing` – Wiedergabe gestartet
- [x] `POST /Sessions/Playing/Progress` – Fortschritt melden
- [x] `POST /Sessions/Playing/Stopped` – Wiedergabe gestoppt
- [x] `POST /Users/{id}/PlayedItems/{itemId}` – Als gesehen markieren
- [x] `DELETE /Users/{id}/PlayedItems/{itemId}` – Als ungesehen markieren
- [x] `POST /Users/{id}/FavoriteItems/{itemId}` – Favorit setzen
- [x] Wiedergabe-Position speichern & wiederherstellen

#### Untertitel
- [x] `GET /Videos/{id}/Subtitles/{index}` – Untertitel abrufen
- [x] Externe Untertitel-Dateien erkennen (.srt, .ass, .vtt)
- [x] Untertitel-Format-Konvertierung (SRT → VTT)
- [x] `GET /Items/{id}/RemoteSearch/Subtitles` – Untertitel suchen (OpenSubtitles)

### Phase 3: Session & Geräte-Management ✓

#### Sessions
- [x] `GET /Sessions` – Aktive Sessions auflisten
- [x] `POST /Sessions/Logout` – Session beenden
- [x] Session-Timeout & automatische Bereinigung
- [x] WebSocket-Verbindung für Echtzeit-Updates
- [x] `POST /Sessions/{id}/Command` – Remote-Steuerung

#### Geräte
- [x] `GET /Devices` – Registrierte Geräte
- [x] `DELETE /Devices` – Gerät entfernen
- [x] Device-ID-Tracking pro Client

### Phase 4: Erweiterte Features ✓

#### Suche
- [x] `GET /Search/Hints` – Suchvorschläge
- [x] Volltextsuche über Titel, Beschreibung, Personen
- [x] Fuzzy-Search / Tippfehler-Toleranz

#### Sammlungen & Playlists
- [x] `GET /Collections` – Sammlungen
- [x] `POST /Collections` – Sammlung erstellen
- [x] `GET /Playlists/{id}/Items` – Playlist-Inhalte
- [x] `POST /Playlists` – Playlist erstellen

#### Personen & Genres
- [x] `GET /Persons` – Schauspieler/Regisseure
- [x] `GET /Persons/{id}` – Person-Details
- [x] `GET /Genres` – Genre-Liste
- [x] `GET /Studios` – Studio-Liste
- [x] `GET /Artists` – Künstler (Musik)

#### Aktivitätslog
- [x] `GET /System/ActivityLog/Entries` – Aktivitätsprotokoll
- [x] Login-Versuche loggen
- [x] Medien-Wiedergabe loggen

#### Benachrichtigungen
- [x] `GET /Notifications/{userId}` – Benachrichtigungen
- [x] Webhook-Support

### Phase 5: Netzwerk & Discovery ✓

#### DLNA
- [x] UPnP-Discovery (SSDP)
- [x] DLNA-Medienserver
- [x] DLNA-Content-Directory-Service

#### Netzwerk
- [x] `GET /System/Endpoint` – Netzwerk-Endpoint-Info
- [x] Automatische Port-Erkennung
- [x] HTTPS/TLS-Support (rustls)
- [x] Reverse-Proxy-Kompatibilität (X-Forwarded-For, etc.)
- [x] IPv6-Support

### Phase 6: Setup & Administration ✓

#### Ersteinrichtung (Setup Wizard)
- [x] `GET /Startup/Configuration` – Setup-Status
- [x] `POST /Startup/Configuration` – Setup abschließen
- [x] `GET /Startup/User` – Initialen Admin-User abfragen
- [x] `POST /Startup/User` – Admin-User erstellen
- [x] `POST /Startup/Complete` – Setup abschließen
- [x] Wizard-Flow für Ersteinrichtung

#### Scheduled Tasks
- [x] `GET /ScheduledTasks` – Geplante Aufgaben
- [x] `POST /ScheduledTasks/{id}/Triggers` – Task-Trigger setzen
- [x] Automatischer Library-Scan (Intervall-basiert)
- [x] Automatische Metadaten-Aktualisierung
- [x] Transkodierungs-Cache-Bereinigung

#### Plugin-System
- [x] Plugin-API definieren (Trait-basiert)
- [x] Plugin-Laden (dynamisch via `libloading` oder WASM)
- [x] `GET /Plugins` – Installierte Plugins
- [x] Plugin-Konfiguration

### Phase 7: Client-Kompatibilität ✓

#### Jellyfin Web Client
- [x] Statische Web-UI ausliefern (jellyfin-web)
- [x] WebSocket-Endpoint (`/socket`)
- [x] Alle benötigten API-Responses im exakten Jellyfin-Format
- [x] CORS-Konfiguration für Web-Client

### Phase 8: Qualität & Betrieb ✓

#### Testing
- [x] Unit-Tests für alle Repository-Funktionen
- [x] Unit-Tests für Auth-Logik
- [x] Integrationstests für API-Endpoints
- [x] End-to-End-Tests mit echtem Client

#### Dokumentation
- [x] README mit Setup-Anleitung
- [x] API-Dokumentation (OpenAPI/Swagger)
- [x] Konfigurationsdokumentation
- [x] Docker-Image & Dockerfile
- [x] docker-compose.yml Beispiel

#### Performance & Stabilität
- [x] Graceful Shutdown
- [x] Connection Pooling (DB)
- [x] Rate Limiting
- [x] Request-Logging (strukturiert)
- [x] Fehlerbehandlung & Error-Responses im Jellyfin-Format
- [x] Speicher-Optimierung für große Bibliotheken (>100k Items)
