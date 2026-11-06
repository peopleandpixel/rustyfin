# Coverage Report

Overall coverage: 3231/13390 (24.13% -14.69%)

## File Coverage

<details>
<summary>Show file coverage details</summary>

| File | Coverage | Percentage | Change |
|------|----------|------------|--------|
| src/cache.rs | 10/142 | 7.04% | +7.04% |
| src/error.rs | 16/36 | 44.44% | +44.44% |
| src/lib.rs | 0/0 | 0.00% | - |
| src/log_stream.rs | 0/66 | 0.00% | - |
| src/middleware.rs | 50/68 | 73.53% | +73.53% |
| src/routes/activity.rs | 4/15 | 26.67% | +26.67% |
| src/routes/additional.rs | 18/221 | 8.14% | +8.14% |
| src/routes/auth.rs | 45/45 | 100.00% | +100.00% |
| src/routes/backup.rs | 5/64 | 7.81% | +7.81% |
| src/routes/branding.rs | 0/0 | 0.00% | - |
| src/routes/collections.rs | 19/151 | 12.58% | +12.58% |
| src/routes/configuration.rs | 7/24 | 29.17% | +29.17% |
| src/routes/devices.rs | 9/61 | 14.75% | +14.75% |
| src/routes/display_preferences.rs | 3/24 | 12.50% | +12.50% |
| src/routes/dlna.rs | 74/181 | 40.88% | +40.88% |
| src/routes/dlna_streaming.rs | 4/76 | 5.26% | +5.26% |
| src/routes/dynamic_hls.rs | 3/18 | 16.67% | +16.67% |
| src/routes/encoding_manager.rs | 5/41 | 12.20% | +12.20% |
| src/routes/images.rs | 36/128 | 28.12% | +28.12% |
| src/routes/items.rs | 167/1200 | 13.92% | +13.92% |
| src/routes/library.rs | 81/309 | 26.21% | +26.21% |
| src/routes/live_tv.rs | 44/500 | 8.80% | +8.80% |
| src/routes/localization.rs | 5/86 | 5.81% | +5.81% |
| src/routes/media_segments.rs | 2/17 | 11.76% | +11.76% |
| src/routes/mod.rs | 72/89 | 80.90% | +80.90% |
| src/routes/packages.rs | 17/78 | 21.79% | +21.79% |
| src/routes/persons.rs | 11/34 | 32.35% | +32.35% |
| src/routes/playback.rs | 15/119 | 12.61% | +12.61% |
| src/routes/plugins.rs | 6/16 | 37.50% | +37.50% |
| src/routes/quick_connect.rs | 7/61 | 11.48% | +11.48% |
| src/routes/scheduled_tasks.rs | 9/27 | 33.33% | +33.33% |
| src/routes/search.rs | 21/82 | 25.61% | +25.61% |
| src/routes/session.rs | 21/138 | 15.22% | +15.22% |
| src/routes/shows.rs | 6/66 | 9.09% | +9.09% |
| src/routes/startup.rs | 21/83 | 25.30% | +25.30% |
| src/routes/streaming.rs | 54/468 | 11.54% | +11.54% |
| src/routes/subtitle.rs | 10/83 | 12.05% | +12.05% |
| src/routes/sync_play.rs | 14/131 | 10.69% | +10.69% |
| src/routes/system.rs | 101/317 | 31.86% | +31.86% |
| src/routes/transcode.rs | 6/107 | 5.61% | +5.61% |
| src/routes/users.rs | 32/244 | 13.11% | +13.11% |
| src/routes/websocket.rs | 2/94 | 2.13% | +2.13% |
| src/state.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-auth/src/lib.rs | 65/65 | 100.00% | +100.00% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-cli/src/main.rs | 0/301 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/dlna.rs | 0/133 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/image_proxy.rs | 22/86 | 25.58% | +25.58% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/lib.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/local_images.rs | 0/90 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/mdns.rs | 0/129 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/media_segments.rs | 53/260 | 20.38% | +20.38% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/metadata.rs | 56/303 | 18.48% | +18.48% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/nfo_parser.rs | 0/61 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/plugin_manager.rs | 9/60 | 15.00% | +15.00% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/probe.rs | 44/329 | 13.37% | +13.37% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/provider_fanart.rs | 0/92 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/provider_omdb.rs | 0/57 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/provider_tmdb.rs | 0/312 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/scanner.rs | 24/518 | 4.63% | +4.63% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/scheduler.rs | 91/388 | 23.45% | +23.45% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/subtitle.rs | 9/155 | 5.81% | +5.81% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/transcode.rs | 82/231 | 35.50% | +35.50% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/trickplay.rs | 0/93 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-core/src/udp_discovery.rs | 0/40 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/lib.rs | 44/55 | 80.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_activity.rs | 33/33 | 100.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_chapter.rs | 76/76 | 100.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_collection.rs | 80/90 | 88.89% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_device.rs | 3/43 | 6.98% | +6.98% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_image.rs | 32/104 | 30.77% | +30.77% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_library.rs | 38/46 | 82.61% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_live_tv.rs | 7/377 | 1.86% | +1.86% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_media.rs | 312/1034 | 30.17% | +0.48% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_media_segment.rs | 35/35 | 100.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_media_source.rs | 207/347 | 59.65% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_misc.rs | 64/84 | 76.19% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_notification.rs | 14/21 | 66.67% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_package.rs | 4/43 | 9.30% | +9.30% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_person.rs | 8/61 | 13.11% | +13.11% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_playback.rs | 179/211 | 84.83% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_playlist.rs | 95/112 | 84.82% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_search.rs | 26/72 | 36.11% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_session.rs | 96/141 | 68.09% | +9.93% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_sync_play.rs | 49/56 | 87.50% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_trickplay.rs | 69/70 | 98.57% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-db/src/repo_user.rs | 131/243 | 53.91% | +23.46% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/activity.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/auth.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/chapter.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/collection.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/configuration.rs | 0/61 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/dlna_profile.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/environment.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/image.rs | 58/64 | 90.62% | +90.62% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/lib.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/library.rs | 14/14 | 100.00% | +100.00% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/media.rs | 42/221 | 19.00% | +17.65% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/media_segment.rs | 6/6 | 100.00% | +100.00% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/media_source.rs | 0/2 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/metadata.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/notification.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/person.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/playback.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/playback_info.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/playlist.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/plugin.rs | 0/6 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/quick_connect.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/scheduled_task.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/search.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/server_info.rs | 16/20 | 80.00% | +80.00% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/session.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/setup.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/trickplay.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/user.rs | 6/9 | 66.67% | +66.67% |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-model/src/ws_event.rs | 0/0 | 0.00% | - |
| /home/jens/RustroverProjects/rustyfin/crates/rustyfin-server/src/main.rs | 0/220 | 0.00% | - |

</details>

