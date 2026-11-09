# Arr webhook fixtures: Sonarr 4.0.20.3014, Radarr 6.4.4.10685, Lidarr 3.1.6.5079
40 files (sonarr/ 13, radarr/ 12, lidarr/ 15), one per event or variant, used by the webhook tests in `api_tests.rs`. Hand-built from the C# sources of the apps at those release tags, NOT captured from running instances.
Checked only by a scratch script: JSON parses; keys and enum values match the payload classes; all 30 Radarr ?h= hashes = SHA-256 prefix of remoteUrl. No replay.
Ids, titles, hosts and health texts are invented. Files are independent snapshots; only the old file in download-upgrade*.json equals *file-delete-upgrade.json.
Source paths below are relative to src/NzbDrone.Core/Notifications/Webhook/ unless noted. "dir" = folder, "file" = file. Paths are as the *arr process sees them (container mounts), so a consumer may need a path map.

## Transport (WebhookProxy.cs is byte-identical in all three apps)
- Method POST (default, WebhookSettings.cs :23; Lidarr :24) or PUT. WebhookMethod POST=1, PUT=2. WebhookProxy.cs :31-36.
- Content-Type application/json (:38), Accept application/json (:28), body = body.ToJson() (:39). No signature or HMAC.
- Auth is optional and uses WebhookSettings fields Url (:28; Lidarr :29), Method (:31/:32), Username (:34/:35), Password (:37/:38), Headers (:40/:41).
  URL: sent as typed, so a token in the query string works. Headers: key/value list added verbatim (WebhookProxy.cs :46-49).
  Basic: if Username or Password is non-blank (:41-44), sent preemptively as Authorization: Basic base64(ISO-8859-1 "user:pass")
  (NzbDrone.Common/Http/Dispatchers/ManagedHttpDispatcher.cs: Sonarr :83-88, Radarr :81-86, Lidarr :83-88).
- Saving an enabled connection in the UI first sends a Test payload (Api ProviderControllerBase.cs :85-88, forceSave=false; same lines in all three).

## Serializer (NzbDrone.Common/Serializer/Newtonsoft.Json/Json.cs: Sonarr+Radarr settings :25-29, enum converter :32, ToJson :119; Lidarr :24-28, :31, :69)
- Null omitted; 0/false/enum-0 always written; Indented, 2 spaces, Environment.NewLine (LF on Linux/Docker, CRLF on Windows); dates UTC ISO-8601, fraction trimmed.
- camelCase keys: MBId -> mbId, TvdbId -> tvdbId, TvMazeId -> tvMazeId. Enums are camelCase strings (manual, upgrade, warning, seasonPack),
  except eventType, which stays PascalCase (WebhookEventType.cs :8, DefaultNamingStrategy). Order: subclass keys first, then eventType, instanceName, applicationUrl.
- applicationUrl is omitted by these builders in WebhookBase.cs: Sonarr Health :159, HealthRestored :172, ApplicationUpdate :185; Radarr HealthRestored :158; Lidarr HealthRestored :190.
  Test payloads send applicationUrl "" when none is set, plus the Windows path C:\testpath (JSON "C:\\testpath": Sonarr :231, Radarr :218, Lidarr :227) and a dummy item.
- images[].url is app-relative, not a path: /MediaCover/[Albums/]<id>/<type>.<ext>, plus ?lastWrite=<ticks> (Sonarr, Lidarr) only if the cached file exists; Radarr uses
  ?h=<first 20 hex of SHA-256(remoteUrl)> (MediaCoverService.cs :92-97 Radarr, :85-127 Lidarr; read from GitHub at the tag).

## eventType (WebhookEventType.cs :11-21; Lidarr :11-23)
- Sonarr: Test, Grab, Download, Rename, SeriesAdd, SeriesDelete, EpisodeFileDelete, Health, ApplicationUpdate, HealthRestored, ManualInteractionRequired.
- Radarr: Test, Grab, Download, Rename, MovieDelete, MovieFileDelete, Health, ApplicationUpdate, MovieAdded, HealthRestored, ManualInteractionRequired.
- Lidarr: Test, Grab, Download, DownloadFailure, ImportFailure, Rename, ArtistAdd, ArtistDelete, AlbumDelete, Health, Retag, ApplicationUpdate, HealthRestored.
- Sonarr import-complete reuses "Download" (WebhookBase.cs :87-108; OnImportComplete flag, NotificationService.cs :241): plural episodeFiles[], fileCount, sourcePath, destinationPath, no isUpgrade.
- Gating (NotificationService.cs): Download only for NewDownload imports (Sonarr :173, Radarr :131, Lidarr :160); upgrades (non-empty deletedFiles) also need OnUpgrade
  (:197/:155/:183). A file-delete with reason upgrade needs the delete-for-upgrade flag (Sonarr :411, Radarr :319). Per-connection tag filters apply.

## Path fields
Sonarr
- series.path (dir): Grab, Download, Rename, SeriesAdd, SeriesDelete, EpisodeFileDelete, ManualInteractionRequired, Test. Health/HealthRestored/ApplicationUpdate: no path.
- Download: episodeFile.path (file = series.path + relativePath, WebhookEpisodeFile.cs :18), .relativePath, .sourcePath (file in the download dir, outside the library, WebhookBase.cs :65);
  deletedFiles[].path/.relativePath (replaced files, :79) and .recycleBinPath (file outside the library; omitted without a recycle bin).
- import-complete: episodeFiles[].path/.relativePath, sourcePath (download-client output, dir or file), destinationPath (dir).
- Rename: renamedEpisodeFiles[].previousPath/.previousRelativePath (old) + .path/.relativePath (new). EpisodeFileDelete: episodeFile.path/.relativePath.
- SeriesDelete: series.path + deletedFiles (bool) only. Grab, SeriesAdd: no file paths.
Radarr
- movie.folderPath (dir): Grab, Download, Rename, MovieAdded, MovieDelete, MovieFileDelete, ManualInteractionRequired, Test. movie.filePath is never sent: its ctor (WebhookMovie.cs :46-50) has no caller; WebhookBase.cs :250 uses the other.
- Download: movieFile.path/.relativePath/.sourcePath and deletedFiles[].path/.relativePath/.recycleBinPath, same rules as Sonarr (WebhookBase.cs :65, :80-81).
  Rename: renamedMovieFiles[] as Sonarr. MovieFileDelete: movieFile.path/.relativePath.
- MovieDelete: folderPath + deletedFiles + movieFolderSize, which is the movie FILE size and 0 unless files were deleted (WebhookBase.cs :124-127). No file path.
Lidarr
- artist.path (dir) wherever an artist is sent: Grab, Download, Rename, Retag, ArtistAdd, ArtistDelete, AlbumDelete, Test. Albums have no path (WebhookAlbum.cs :10-19).
- File entries carry only an absolute path (WebhookTrackFile.cs :15/:25): trackFiles[] (Download), deletedFiles[], renamedTrackFiles[].path + .previousPath
  (WebhookRenamedTrackFile.cs :10/:13), trackFile (Retag). No relativePath, sourcePath or recycleBinPath, and no file-delete event.
- Missing paths: DownloadFailure has no artist, album or paths; ImportFailure is an empty shell: tracks [], trackFiles [], isUpgrade false (WebhookBase.cs :90-114;
  NotificationService.cs :373-397 sets only Message, which is not serialized).
- REFUTED: "Lidarr delete events carry only a deletedFiles flag". ArtistDelete = artist (with path) + deletedFiles (WebhookBase.cs :151-161, WebhookArtistDeletePayload.cs :5-6);
  AlbumDelete = artist (with path) + album (no path) + deletedFiles (:163-174, WebhookAlbumDeletePayload.cs :5-7). The flag is the only word on files: no list, no album folder.

## Variants beyond the minimum set
*-same-path (old and new file share one path), *-files-kept (deletedFiles false), import-complete (Sonarr), retag, download-failure, import-failure.
