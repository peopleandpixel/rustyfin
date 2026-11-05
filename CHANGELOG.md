# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Web client translations**: The web client is localized with i18next, with English and German translations in `crates/rustyfin-web/src/translations/` (one JSON file per language, English as the source). The display language follows the browser and can be changed in the setup wizard and in the user settings; picking it in the wizard switches the UI immediately. Previously the UI was German only.
- **User roles in the dashboard**: The Users tab has an Admin and a Disabled checkbox for each user. `POST /Users/{id}/Policy` also takes `IsDisabled`. A disabled user can't sign in. Disabling deletes the user's tokens, sessions and devices at once, so they leave the sessions list, an open websocket closes within 5 s, and none of them comes back when the account is enabled again. As in Jellyfin, an administrator can't be disabled, not even in the request that removes the rights: remove them first, then disable. A disabled account becomes an administrator only in the request that enables it, and the last enabled administrator stays one. The owner's role and access can't be changed, and no administrator can demote or disable their own account. The dashboard locks every box the server would refuse, with the reason. A change takes effect on the user's next request, and an open websocket follows within 5 s. User answers carry `IsOwner` and `IsDisabled`.

### Changed
- **User policy and public users**: `POST /Users/{id}/Policy` answers 404 for an unknown user whatever the body; before, a body without `IsAdministrator` got 200. `GET /Users/Public` leaves out disabled users, as Jellyfin does. When no enabled administrator is left, the startup log's recovery hint now sends `{"IsAdministrator": true, "IsDisabled": false}` to the Policy endpoint, which can enable a disabled account too, instead of sending operators to the database for that.
- **User names in any letter case**: Password sign-in (`POST /Users/AuthenticateByName`), `/Users/ForgotPassword` and resetting a password by PIN find an account by its name in any letter case and Unicode compatibility form, as Jellyfin does: the exact name first, else the one account whose name matches. When several accounts match and none exactly, the sign-in fails as with a wrong password. `POST /Users/New` and renames refuse a name that matches another account's with 400 "user already exists"; users may still change the case of their own name. Redeeming a reset PIN removes its file, whatever spelling the reset was asked with. Before, names matched only exactly, and a rename to another user's exact name answered 500. Accounts whose names already differ only in case keep signing in with their exact names.
- **pnpm for the web client**: `crates/rustyfin-web` uses pnpm instead of npm. `package.json` pins the version in `packageManager` (Corepack and `pnpm/action-setup` read it), and `pnpm-lock.yaml` replaces `package-lock.json` with the same dependency versions. The Dockerfile, the release workflow, `build.sh`, `start.sh` and the README use pnpm.
- **DLNA server off by default**: The DLNA server, its `/dlna/*` endpoints and its SSDP announcements, runs only when `EnableDlnaServer` is on in the server configuration. Before, it always ran and listed the libraries to anyone on the network without a sign-in. An administrator turns it on with `POST /System/Configuration`; the `/dlna/*` endpoints follow the setting at once, and SSDP starts at the next server start.
- **Letter rail in the library**: The library page's letter bar is now a vertical rail at the right edge, like Jellyfin's alphabet picker. It stays in view while the list scrolls and highlights the letter of the first title at the top of the view. A click scrolls to that letter's first title and filters nothing; the first entry (`#`, or `Z` in Name Z-A, where the rail runs from Z to A) goes back to the top. It shows for the name sorts, as the bar did. Before, the bar scrolled away with the list and never marked the current letter.
- **Access tokens survive restarts**: Access tokens are now opaque values stored server-side, one per device, instead of self-contained JWTs signed with a per-start secret. A restart no longer logs anyone out, and the server id is stable across restarts (persisted to `<data>/server_id`). `RUSTYFIN_SECRET` is now ignored (with a one-time startup notice). **Upgrade note:** tokens issued by an older build stop working once, at the upgrade — every client signs in again after updating.
- **Stricter login rate limit**: Only the login, forgot-password and reset endpoints are rate-limited. Before, one budget of 500 requests per 60 s per client IP covered every route, so a remote client scrolling a library grid got 429 for its thumbnails within seconds. These endpoints now allow 10 requests per 60 seconds per client IP by default, tunable with `RUSTYFIN_RATE_LIMIT` and `RUSTYFIN_RATE_WINDOW` (`0` disables). `X-Forwarded-For` is trusted only from a proxy listed in the new `RUSTYFIN_KNOWN_PROXIES` (comma-separated IPs or CIDRs; loopback only when listed); otherwise the connecting peer address is used, so a spoofed header can no longer claim the loopback exemption or rotate past the limit.

### Fixed
- **New administrators**: `POST /Users/New` honors `IsAdministrator`, and a user created in the dashboard with the Administrator box ticked is an administrator. Before, the server ignored the field and the web client dropped the box, so every new user was created without administrator rights.
- **Played and favorite marks of another user**: `POST` and `DELETE /Users/{userId}/PlayedItems/{itemId}` and `/Users/{userId}/FavoriteItems/{itemId}` act on `{userId}`. Before, they ignored it and changed the caller's own marks. An administrator can set them for any user; other users get 403 for anyone but themselves.
- **Startup wizard metadata settings**: The wizard now saves the chosen metadata language and country. It sent them as `preferred_metadata_language` and `metadata_country_code`, which `POST /Startup/Configuration` ignores because it expects PascalCase fields.
- **Setup wizard owner account**: The server no longer creates an `admin`/`admin` account at start. The setup wizard creates the first administrator, the server owner, with the typed name and password. Before, the wizard only changed the default admin's password, ignored the typed name and hid sign-in failures. The `/Startup/*` endpoints also stayed anonymous, so anyone could reset the administrator's password. Now:
  - `POST /Startup/User` creates the owner only on a server without users and never changes an existing account. The `/Startup/*` endpoints are open only while the server has no users and need an administrator afterwards.
  - `StartupWizardCompleted` means that the server has users.
  - The owner cannot be deleted or demoted.
  - On upgrade, the earliest enabled administrator becomes the owner at the first start.
  - The wizard shows every failed step and resumes there on retry.
  - `start.sh smoke` creates a throwaway owner in a temporary data directory.
- **Library folders**: Creating a library now checks its folders. Before, `POST /Library/VirtualFolders` stored any path, so the setup wizard and the library dialog accepted a folder that does not exist, and the library's scan found nothing. Now each path must be a full path to a folder that exists on the server and that the server can list. Otherwise the server stores nothing and answers 400 with a message that names each bad path and the reason, for example `The folder "/media/films" was not found on the server.` In Docker, the paths are the ones inside the container.
- **TMDb client**: All TMDb requests share one client with rate limiting (about 9 per second, 4 in flight), retries with backoff, `Retry-After` handling for 429 and the `Rustyfin/<version>` User-Agent. The API key no longer appears in logs or errors. New variables `RUSTYFIN_TMDB_API_KEY`, `RUSTYFIN_TMDB_BASE_URL` and `RUSTYFIN_TMDB_IMAGE_BASE_URL`; the env key takes precedence over the config key and is never shown in or saved to the server configuration.
- **Titles with quotes in the web client**: Play buttons, the sidebar Scan button and other actions now work for titles and library names with an apostrophe, such as *World's Greatest Dad*. The web client built inline `onclick` JavaScript from server values without escaping `'`, so the click failed with a syntax error, and a crafted file name could run script in the page. Actions now read their values from `data-*` attributes through one delegated click handler, and server text is escaped.
- **Resume playback**: Closing the player keeps the position, and playing again resumes there, as in Jellyfin. Before, the web player reported position 0 at start and when it closed, which erased the resume point, and it always started at 0. The detail page offers Resume and Play from beginning; a point in the last 10 % of the runtime counts as watched.
- **Scan progress in the web client**: The scan progress connection now opens. The client used the wrong path (`/websocket` instead of `/socket`), sent no token and read the wrong fields, so the scan banner and the refresh after a scan never worked, and it retried about every second without logging anything. It now refreshes the open view when a scan ends, logs one warning per outage, retries with backoff up to 30 s and closes on logout.
- **Scan and Stop errors**: Every Scan button and Scan All now show an error toast when the scan can't start, for example "library not found". The task Stop button sent no request (`this.delete is not a function`); it now sends it and shows the server's error, for example when the task has already finished.
- **Setup wizard library scan**: The library created in the setup wizard is scanned right away. Before, it stayed empty until the next scheduled scan, up to 12 hours later.
- **Symlinked folders in the folder picker**: `GET /Library/BrowseDirectories` now lists a symlink that points to a folder, under the link's own path, since the library scan follows such links too. Before, the library dialog's folder picker left these folders out, so they could only be typed.
- **Scan notice in the web client**: Starting a scan now shows one toast as the request is sent, and the scan banner is gone. Before, the banner showed alongside the toast, and the toast came only when the scan had ended, since the scan request answers then. The setup wizard now also shows a toast when it starts its scan. A scan that can't start, for example for a deleted library, shows only the error, and one that fails later shows "Scan failed".
- **Scan finished notice in the web client**: When a library scan ends, the web client now shows a toast that names the library, such as "Scan finished: Films", in every open tab, whoever started the scan. Scan All shows one for each library. A failed scan shows only its error. Scans the server runs on its own schedule don't show it yet, since they send no scan events.
- **Resume or start over**: Starting a video that has a resume point from a card, the home page's Play button or an episode now asks whether to resume from the saved time or to start from the beginning, as in Jellyfin. Before, only the detail page offered the choice, and everything else resumed without asking. Escape or a click outside the menu cancels. A video without a resume point, or one in its last 10 %, starts at once.
- **Folder picker in the library dialog**: The picker now works like Jellyfin's. It shows the open folder's path in a field you can edit, where Enter opens the typed folder; a click on a folder opens it; and "Select this folder" adds the open folder to the library's paths. "Create library" also takes the folder the open picker shows, so browsing to a folder and pressing Create works. Before, the picker hid the path field, only a small ✓ next to each folder added a path and the open folder could not be chosen, so Create failed with "Name and at least one path are required".
- **Password hashing no longer blocks the server (M-15)**: argon2 hashing and verification run on a blocking thread pool, so a burst of logins no longer stalls unrelated requests.
- **Login response reflects the current login (M-16)**: `AuthenticateByName` now returns the user with the `LastLoginDate` set by that login, not the previous one.
- **Next Up and Upcoming parameters**: `GET /Shows/NextUp` and `GET /Shows/Upcoming` accept `Limit`, `StartIndex`, `UserId`, `SeriesId` and `ParentId` as well as their camelCase forms, as Jellyfin does. Before, the capitalized forms were ignored, so the web client's Next Up row asked for 12 episodes and got up to 20.

### Security
- **Administrator endpoints**: Endpoints that change the server or show its internals need an administrator, following Jellyfin's rule for each endpoint. They cover configuration changes, shutdown and restart, logs, scheduled tasks, plugins, packages, backups, devices, the activity log and API keys. They also cover creating and deleting users and setting their policy, library and item refreshes, deleting items, uploading and deleting images, subtitle management, collections, Live TV tuners and listing providers, DLNA profiles, localization providers and the media encoder settings. Before, any signed-in user could call them, and the DLNA profile endpoints needed no sign-in. A request without a valid token gets 401; a signed-in user without the right gets 403, which also replaces the 401 for another user's account settings.
- **Library setup endpoints**: `/Environment/Drives`, `/Environment/NetworkShares`, `/Library/BrowseDirectories`, `/Library/VirtualFolders` and `/Library/{id}/Options` are open only while the server has no users, and need an administrator afterwards, as in Jellyfin. Before, the Environment endpoints listed the server's drives to anyone without a sign-in, and every user could browse the server's folders and change the libraries.
- **Files outside their folders**: `GET /System/Log/{name}` returns only a log file that `GET /System/Logs` lists, and the HLS segment endpoints return only a segment in the item's transcode folder. Other names get 404. Before, a signed-in user could read any file the server could read, with a name such as `..%2Fconfig.json` or `%2Fetc%2Fhostname`.
- **Media encoder path**: `PUT /System/MediaEncoder/Path` and `POST /System/MediaEncoder/Path/Validate` are removed. They ran the program at the path in the request, so any signed-in user could run any program on the server. As in Jellyfin, the encoder path cannot be changed through the API. `POST /System/MediaEncoder/Test` stops FFmpeg after 10 seconds.
- **Easy PIN**: `POST /Users/{userId}/EasyPassword` stores the PIN as an Argon2 hash, and no response returns it. Before, it replaced the user's configuration with the PIN in plain text, and `GET /Users` showed it to every user. The upgrade removes PINs stored the old way; set them again.
- **Configuration secrets**: For users who are not administrators, `GET /System/Configuration` and `GET /System/Configuration/{key}` return `TmdbApiKey`, `OmdbApiKey`, `FanartApiKey` and `SmtpPassword` as null. Before, every signed-in user could read them.
- **Empty API key and secret**: An empty `RUSTYFIN_API_KEY` (for example `RUSTYFIN_API_KEY=` in compose or an empty field in a NAS template) gave administrator access to any request with an empty token, such as `?api_key=`. An empty `RUSTYFIN_SECRET` let anyone sign their own administrator tokens. `RUSTYFIN_API_KEY` is now trimmed, an empty value counts as unset, and the server logs a warning. `RUSTYFIN_SECRET` is no longer used, since access tokens are stored server-side. An empty token never authenticates. The `/socket` websocket now also accepts user tokens when an API key is set, so the web client's scan progress works on such servers. The docs use placeholders instead of example keys.
- **Another user's watch data**: `GET /Users/{userId}/Items/Resume`, `GET /Notifications/{userId}` and `DELETE /Users/{userId}/Items/{itemId}/Rating` need the caller to be `{userId}` or an administrator, and so does a `userId` given to `GET /Shows/NextUp` and `GET /Shows/Upcoming`. Other users get 403; an administrator naming an unknown user gets 404. Before, any signed-in user could read another user's resume list, Next Up, Upcoming and notifications, and clear their ratings. Without `userId`, Next Up and Upcoming now follow the caller's own watch history, as in Jellyfin; before, they ignored it and also listed episodes the caller had already watched.
- **Remote control of other users' sessions**: `POST /Sessions/{sessionId}/Command`, `/Sessions/{sessionId}/Command/{command}`, `/Sessions/{sessionId}/Message` and `/Devices/{deviceId}/Command` need the caller to own the session or device, or to be an administrator. Other users get 403, also for an id that does not exist; an administrator gets 404 for one. `GET /Sessions` lists only the caller's own sessions unless the caller is an administrator. Before, any signed-in user could send commands and messages to everyone's sessions and see every session on the server.
- **LAN admin takeover via the reset PIN (R1-01)**: The forgot-password flow now follows Jellyfin's. A reset PIN is issued only to a request from the local network and only for an existing user, written to a file in the data directory (mode 0600), and never returned or logged; the response is uniform, arrives after the same fixed delay (250 ms), and does not reveal whether a user exists. The PIN is random, stored only as a hash, valid for 30 minutes, single-use, and voided after five wrong attempts. The old biased PIN generator was removed (M-17).
- **Access tokens can be revoked (R1-08, R1-09)**: Logging out ends that device's token, changing a password ends the user's other tokens, and deleting a user ends all of theirs; a disabled user's existing tokens are rejected. Restarts keep valid tokens valid.
- **`admin`/`admin` after an upgrade**: At startup the server logs a warning naming any enabled administrator whose password is still `admin`.
- **Setup wizard on upgraded servers**: At the first start after an upgrade, a server that has users but no owner makes its earliest enabled administrator the owner, logs it and marks the setup wizard complete in `config.json`, even if the wizard was never finished. If no administrator is enabled, the server logs an error with the steps to make one, and the wizard stays closed. The `/Startup/*` and library setup endpoints are open only while the server has no users, and `POST /Startup/User` never changes an existing account. Before, a server whose wizard was never finished, or that had no enabled administrator, kept no owner, so anyone who could reach it could use the wizard without signing in to set a new password on any account and make it an administrator and the owner.
- **Proxy address handling**: `X-Forwarded-For` is read across all header lines, right to left, with `ip:port` and `[v6]:port` entries; loopback is trusted as a proxy only when listed in `RUSTYFIN_KNOWN_PROXIES`, and a malformed entry no longer resolves to a client-chosen address. The reset-PIN LAN check uses the same client address, IPv4-mapped IPv6 peers count as their IPv4 address, and IPv6 clients share one login budget per /64. A peer that is not listed but sends `X-Forwarded-For`, `X-Real-IP` or `Forwarded` is neither local nor exempt from the login limit, and all its clients share one budget. Before, a client behind a same-host proxy could claim the loopback exemption.
- **Reset PIN guessing bound**: a new forgot-password request no longer replaces a pending or voided reset, so each reset allows at most 5 wrong PINs; redemption is atomic and deletes the PIN. `ResetPassword` answers an unknown user like a wrong PIN.
- **Sessions end on device removal**: removing a device (`DELETE /Devices?id=` or `DELETE /Devices/{deviceId}`) signs it out.
- **Revoked sessions close their websocket**: an open websocket closes within about 5 seconds after its token is revoked, also when it is revoked while the socket opens.
- **Continue Watching, favorites and watched filters**: `GET /Items/Resume` lists only the caller's part-watched items, each once, and accepts `userId` under the same rules as Next Up; the API key without `userId` gets 404, as in Jellyfin. `IsFavorite`, `IsPlayed` and `IsUnplayed` match only the caller's favorites and watch history. Before, Continue Watching showed every user's items, an item two users had started appeared twice, and the filters matched any user's data. The last item of every list, and the only item of a one-item list, now carries the caller's `UserData`; before, it had none.
- **Live updates over `/socket`**: A user who is not an administrator now gets only their own session and playback events (`SessionChanged`, `PlaybackStart`, `PlaybackProgress`, `PlaybackStop`) and no server log lines (`LogMessage`); administrators and the API key still get every event. Remote-control commands and messages to a session or device now name its owner, so they reach that user and administrators only. Authorizing a Quick Connect code no longer sends the code's secret to every socket. Before, every signed-in user's socket received all users' session and playback events and the server's log lines.
- **Another user's now playing item and ratings**: `GET /Sessions/Playing/NowPlayingItem?SessionId=` needs the caller to own the session or to be an administrator. Other users get 403, also for a session that does not exist; an administrator gets 404 for one. `PUT /Users/{userId}/Items/{itemId}/Rating` sets `{userId}`'s rating and needs the caller to be `{userId}` or an administrator; other users get 403 and an administrator naming an unknown user gets 404. `DELETE` on the same path answers 404 for an unknown item. Before, any signed-in user could see what another user's session was playing, and setting a rating ignored `{userId}` and always changed the caller's own rating.
- **Long user names**: A user name is at most 255 UTF-16 code units, as in Jellyfin. `POST /Users/New`, `PUT /Users/{userId}` and `POST /Startup/User` refuse a longer name with 400. A longer name is never folded, so it matches only an exactly equal name, and an account from before the limit still signs in with its exact name. Before, a name of any length was accepted, and a 1.8 MB name was folded once per account on every sign-in and reset request.

## [0.14.0] - 2026-11-03

### Fixed
- **Clippy `collapsible_if` warnings**: Collapsed nested `if let` statements in `scanner.rs` and `items.rs` to comply with strict Clippy rules (`-D warnings`). Ensures CI passes with `cargo clippy --workspace --all-targets --all-features -- -D warnings`.

## [0.13.0] - 2026-11-03

### Changed
- **Additional performance optimizations**: Further improvements to reduce latency, eliminate redundant queries, and optimize hot paths.
  - **TV scanner version detection**: Pre-load all episodes into a HashMap for O(1) lookups, eliminating N+1 database queries during TV library scans (matching the movie scanner optimization).
  - **Episode enrichment**: Eliminated duplicate Season database query by loading Season once and reusing it for both series info and parent image tags (3→2 queries per episode).
  - **Filter query cache**: Replaced `std::sync::Mutex` with `parking_lot::Mutex` to avoid blocking Tokio worker threads under high concurrency.
  - **Blurhash HTTP client reuse**: `compute_blurhash` now accepts an optional shared HTTP client, eliminating per-request client creation overhead for remote images.
  - **`normalize_name` optimization**: Reduced string allocations from ~60 per call to ~2 by using a single buffer and processing tags in one word-based pass instead of 48x `.replace()` calls.
  - **L1 cache TTL**: Increased item details cache TTL from 30 seconds to 5 minutes, improving cache hit rate for frequently accessed items.
  - **Transcode manager**: Replaced `RwLock<HashMap>` with `DashMap` for lock-striping, reducing contention under concurrent transcoding workloads.

## [0.12.0] - 2026-11-03

### Changed
- **Performance optimizations**: Comprehensive improvements across all layers to reduce latency and eliminate bottlenecks.
  - **Scanner version detection**: Pre-load all existing movies into a HashMap for O(1) lookups, eliminating N+1 database queries during flat library scans.
  - **Image proxy**: Reuse shared HTTP client for remote image downloads instead of creating a new client per request.
  - **Item enrichment**: Parallelize 6 independent database queries (images, chapters, trickplay, user data, people, studios) using `tokio::join!` instead of running them sequentially.
  - **Rate limiter**: Replaced `Mutex<HashMap>` with `DashMap` for lock-striping, reducing contention under high concurrency.
  - **Batch inserts**: Disable filter triggers during batch media item inserts to avoid write amplification from per-row COUNT(*) queries.
  - **Metadata refresh**: Increased concurrent API task semaphore from 5 to 10, better utilizing TMDb's 20-40 req/s rate limit.
  - **Blurhash pre-computation**: Added `blurhash` column to `item_images` table; blurhash is now computed during metadata refresh and stored in the database, avoiding on-demand computation on every item detail request.
  - **Transcode cache cleanup**: Added a periodic background task (every 15 minutes) that automatically removes stale transcode sessions (older than 1 hour) and cleans up orphaned cache directories.
  - **Search performance**: Replaced slow LIKE substring queries with FTS5 full-text search across name, overview, and genres. LIKE fallback now uses prefix matching (`query%`) so B-tree indexes can be used when FTS5 is unavailable. Added `idx_media_items_name` and `idx_media_items_type_name` indexes.

## [0.8.0] - 2026-11-03

### Changed
- **Database migrations**: Replaced manual migration execution with SQLx's built-in `Migrator` for cleaner, tracked database migrations.
  - SQLx now tracks applied migrations in the `sqlx_migrations` table, ensuring each migration runs exactly once.
  - Renamed migration files to have unique sequential numbers (001–027), fixing the duplicate `002_` prefix.
  - Reduced `lib.rs` from 245 to 115 lines by replacing 26 manual `include_str!()` calls with a single `MIGRATOR.run()` call.

## [0.7.0] - 2026-10-29

### Added
- **Comprehensive test suite**: Added 200+ new tests across the workspace, bringing total test count to 318.
  - **API integration tests** (57 tests): Auth, library, search, system, DLNA, items, sessions, playback, users, Live TV, and more.
  - **Unit tests for route helpers** (91 tests): DLNA XML escaping, UPnP classes, device detection, auth header parsing, library views, range parsing, MIME type detection.
  - **Model tests** (88 tests): MediaType, LibraryContentType, ImageType, SegmentType, ProviderIds, and more.
  - **Core library tests** (19 tests): Subtitle SRT-to-VTT conversion and metadata filename cleaning.
  - **Auth tests** (16 tests): Password hashing, JWT token creation/validation, refresh tokens, PIN generation.
- **Test coverage improved**: Overall line coverage increased from 14.88% to 24.14%.

### Changed
- **`libraries_to_views` visibility**: Changed to `pub(crate)` in `library.rs` to allow unit testing.

## [0.6.0] - 2026-10-29

### Added
- **Cross-platform binary releases**: GitHub Actions workflow builds and packages binaries for 5 platforms on every push to `master` and on published releases.

### Changed
- **TLS implementation**: Switched `reqwest` from `native-tls` (OpenSSL) to `rustls-tls` for reliable cross-compilation without system OpenSSL dependencies.
- **Disk usage API**: Made `disk_usage()` cross-platform compatible with `#[cfg(target_family = "unix")]` guard and non-Unix fallback.
- **Windows cross-compilation**: Added `cargo-xwin` support for building Windows binaries from Linux CI runners.
- **macOS builds**: Native builds on macOS GitHub runners (x86_64 and aarch64), as `cross` cannot cross-compile to macOS.

### Fixed
- **aarch64-unknown-linux-gnu build**: Resolved OpenSSL version mismatch by using `rustls` instead of `native-tls`.
- **x86_64-pc-windows-msvc build**: Configured `cargo-xwin` with required LLVM/clang dependencies for Windows cross-compilation.
- **macOS aarch64 build**: Moved from `cross` fallback to native macOS runner builds.

## [0.5.0] - 2026-10-29

### Added
- **Cross-platform release builds**: GitHub Actions workflow (`release.yml`) uses `cross` to build binaries for 5 platforms (Linux x86_64/aarch64, Windows x86_64, macOS x86_64/aarch64)
- **Release artifacts**: Each binary is packaged with the embedded web client for standalone distribution

### Changed
- **Code formatting**: Applied `cargo fmt` to fix line-length violations across API and DB crates

### Other
- **Build script** (`build.sh`): Automated build pipeline with Quality Gates (fmt, clippy, test), WebUI build, and release build
- **systemd service** (`rustyfin.service`): Production-ready service file with hardening (ProtectHome, ProtectSystem, NoNewPrivileges)
- **One-command install**: `sudo ./build.sh --install` installs binary, creates system user, and sets up systemd service

## [0.4.0] - 2026-10-28

### Added
- **Extended metadata from TMDb**: Movie and TV details now include status, original language, tagline, production countries, budget, revenue, homepage, vote count, and popularity
- **Studios from TMDb**: Studios are now fetched and stored in `studios`/`item_studios` tables
- **People for all media types**: Cast and crew data is now loaded for movies, episodes, and series (not just audio)
- **People batch query**: Batch enrichment now loads all people in a single query instead of N+1
- **Service Worker**: Offline caching for static web assets (JS, CSS, images)
- **Infinite Scroll**: Automatic loading when scrolling near the bottom, replacing "Load More" buttons
- **Virtual Scrolling**: Only visible cards are rendered in the library view (optimized for 1500+ items)
- **Vite build system**: Hot-reload development server (`npm run dev`) and optimized production builds

### Changed
- **WebUI performance optimizations**:
  - HLS.js loaded locally instead of from CDN (~200-500ms faster startup)
  - Image requests optimized: full-resolution images loaded on hover only
  - DOM rendering uses `DocumentFragment` instead of `innerHTML`
  - Template caching for item cards (~30% faster rendering)
  - Search input debounced (300ms delay)
  - CSS `backdrop-filter` only on desktop (mobile performance)
  - Hardware-accelerated CSS animations with `will-change`
- **Cache version automation**: MD5 hash of all assets is automatically computed on build and injected into Service Worker and HTML
- **Migration 025**: New DB columns for extended metadata (status, budget, revenue, homepage, vote_count, popularity)

### Fixed
- **Movie detail page**: Missing backdrop image, cast, and studio data
- **Similar recommendations**: Inconsistent card sizes (mixed movies and episodes in same row)
- **Test compilation**: `hash_combine` and `filter_cache_key` now accessible from test module
- **API tests**: Missing `http_client` field in test `AppState`
- **Migration 025**: Now registered in `lib.rs` so test databases include new columns

### Removed
- **Manual build script**: `build.js` replaced by Vite configuration (`vite.config.mjs`)

### Other
- **Code quality**: All `cargo fmt`, `cargo clippy`, and `cargo test` gates pass with zero warnings
- **Clippy allowances**: Added `#[allow(dead_code)]` for TMDb deserialization structs and `#[allow(clippy::too_many_arguments)]` for SQLx query functions
