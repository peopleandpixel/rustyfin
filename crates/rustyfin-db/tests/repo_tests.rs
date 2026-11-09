use chrono::Utc;
use sqlx::SqlitePool;
use std::path::PathBuf;
use uuid::Uuid;

use rustyfin_model::library::{Library, LibraryContentType};
use rustyfin_model::media::{LocationType, MediaItem, MediaType};
use rustyfin_model::user::User;

/// Create a temporary file-based SQLite database for proper test isolation.
/// Returns (pool, temp_dir) — the temp dir is deleted when dropped.
struct TempDb {
    pool: SqlitePool,
    _dir: PathBuf,
}

impl TempDb {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("rustyfin_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).ok();
        let db_path = dir.join("test.db");
        let db_url = db_path.to_str().unwrap().to_string();
        let pool = rustyfin_db::init_db(&db_url).await.unwrap();
        Self { pool, _dir: dir }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

/// Create an in-memory SQLite database with all migrations applied.
async fn test_pool() -> TempDb {
    TempDb::new().await
}

fn make_user(name: &str, is_admin: bool) -> User {
    User {
        id: Uuid::new_v4(),
        name: name.to_string(),
        password_hash: "$argon2id$v=19$m=19456,t=2,p=1$fake$fakehash".to_string(),
        is_administrator: is_admin,
        created_at: Utc::now(),
        last_login_date: None,
        configuration: serde_json::json!({}),
    }
}

fn make_library(name: &str, ct: LibraryContentType) -> Library {
    Library {
        id: Uuid::new_v4(),
        name: name.to_string(),
        content_type: ct,
        paths: vec!["/media/movies".to_string()],
    }
}

// ── User Repository Tests ──

#[tokio::test]
async fn test_create_and_get_user_by_name() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("alice", true);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let found = rustyfin_db::repo_user::get_user_by_name(&pool, "alice")
        .await
        .unwrap();
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.name, "alice");
    assert!(found.is_administrator);
    assert_eq!(found.id, user.id);
}

#[tokio::test]
async fn test_get_user_by_id() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("bob", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap();
    assert!(found.is_some());
    assert_eq!(found.unwrap().name, "bob");
}

#[tokio::test]
async fn test_get_user_not_found() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let found = rustyfin_db::repo_user::get_user_by_name(&pool, "nonexistent")
        .await
        .unwrap();
    assert!(found.is_none());

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, Uuid::new_v4())
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_get_all_users() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    rustyfin_db::repo_user::create_user(&pool, &make_user("u1", false))
        .await
        .unwrap();
    rustyfin_db::repo_user::create_user(&pool, &make_user("u2", true))
        .await
        .unwrap();

    let users = rustyfin_db::repo_user::get_all_users(&pool).await.unwrap();
    assert_eq!(users.len(), 2);
}

#[tokio::test]
async fn test_delete_user() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("deleteme", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let deleted = rustyfin_db::repo_user::delete_user(&pool, user.id)
        .await
        .unwrap();
    assert!(deleted);

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap();
    assert!(found.is_none());

    // Deleting again returns false
    let deleted = rustyfin_db::repo_user::delete_user(&pool, user.id)
        .await
        .unwrap();
    assert!(!deleted);
}

#[tokio::test]
async fn test_update_password() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("pwuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let updated = rustyfin_db::repo_user::update_password(&pool, user.id, "newhash")
        .await
        .unwrap();
    assert!(updated);

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.password_hash, "newhash");
}

#[tokio::test]
async fn test_update_policy() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("policyuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();
    assert!(!user.is_administrator);

    rustyfin_db::repo_user::update_policy(&pool, user.id, true)
        .await
        .unwrap();
    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(found.is_administrator);
}

#[tokio::test]
async fn test_update_last_login() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("loginuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_user::update_last_login(&pool, user.id)
        .await
        .unwrap();
    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert!(found.last_login_date.is_some());
}

#[tokio::test]
async fn test_update_configuration() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("confuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    let new_config = serde_json::json!({"theme": "dark"});
    rustyfin_db::repo_user::update_configuration(&pool, user.id, &new_config)
        .await
        .unwrap();

    let found = rustyfin_db::repo_user::get_user_by_id(&pool, user.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.configuration["theme"], "dark");
}

// ── Library Repository Tests ──

#[tokio::test]
async fn test_create_and_get_library() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let found = rustyfin_db::repo_library::get_library_by_id(&pool, lib.id)
        .await
        .unwrap();
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.name, "Movies");
    assert_eq!(found.content_type, LibraryContentType::Movies);
    assert_eq!(found.paths, vec!["/media/movies".to_string()]);
}

#[tokio::test]
async fn test_get_all_libraries() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    rustyfin_db::repo_library::create_library(
        &pool,
        &make_library("Movies", LibraryContentType::Movies),
    )
    .await
    .unwrap();
    rustyfin_db::repo_library::create_library(
        &pool,
        &make_library("TV", LibraryContentType::TvShows),
    )
    .await
    .unwrap();

    let libs = rustyfin_db::repo_library::get_all_libraries(&pool)
        .await
        .unwrap();
    assert_eq!(libs.len(), 2);
}

#[tokio::test]
async fn test_delete_library() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("ToDelete", LibraryContentType::Mixed);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    rustyfin_db::repo_library::delete_library(&pool, lib.id)
        .await
        .unwrap();
    let found = rustyfin_db::repo_library::get_library_by_id(&pool, lib.id)
        .await
        .unwrap();
    assert!(found.is_none());
}

// ── Session Repository Tests ──

#[tokio::test]
async fn test_upsert_and_get_sessions() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("sessuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_session(
        &pool, "sess1", user.id, "dev1", "MyDevice", "TestApp", "1.0",
    )
    .await
    .unwrap();

    let sessions = rustyfin_db::repo_session::get_active_sessions(&pool)
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].device_name, "MyDevice");
}

#[tokio::test]
async fn test_delete_session() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("sessuser2", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_session(
        &pool, "sess2", user.id, "dev2", "Device", "App", "1.0",
    )
    .await
    .unwrap();

    rustyfin_db::repo_session::delete_session(&pool, "sess2")
        .await
        .unwrap();
    let sessions = rustyfin_db::repo_session::get_active_sessions(&pool)
        .await
        .unwrap();
    assert!(sessions.is_empty());
}

#[tokio::test]
async fn test_cleanup_expired_sessions() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("expuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_session(&pool, "old_sess", user.id, "dev", "D", "A", "1")
        .await
        .unwrap();

    // Manually set last_activity to the past so cleanup finds it
    sqlx::query("UPDATE sessions SET last_activity = datetime('now', '-10 minutes')")
        .execute(&pool)
        .await
        .unwrap();

    let cleaned = rustyfin_db::repo_session::cleanup_expired_sessions(&pool, 5)
        .await
        .unwrap();
    assert!(cleaned >= 1);
}

// ── Device Repository Tests ──

#[tokio::test]
async fn test_upsert_and_get_devices() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("devuser", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_device(
        &pool,
        "device1",
        "Living Room TV",
        "Jellyfin",
        "10.8",
        user.id,
    )
    .await
    .unwrap();

    let devices = rustyfin_db::repo_session::get_all_devices(&pool)
        .await
        .unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Living Room TV");
}

#[tokio::test]
async fn test_delete_device() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("devuser2", false);
    rustyfin_db::repo_user::create_user(&pool, &user)
        .await
        .unwrap();

    rustyfin_db::repo_session::upsert_device(&pool, "device2", "Phone", "App", "1.0", user.id)
        .await
        .unwrap();

    rustyfin_db::repo_session::delete_device(&pool, "device2")
        .await
        .unwrap();
    let devices = rustyfin_db::repo_session::get_all_devices(&pool)
        .await
        .unwrap();
    assert!(devices.is_empty());
}

// ── Media Source & Stream Tests ──

#[tokio::test]
async fn test_insert_media_source_with_streams() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();

    // Create a library and media item
    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let item_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(item_id.to_string())
    .bind(lib.id.to_string())
    .bind("Sample Movie")
    .bind("Movie")
    .bind("/media/movies/sample.mkv")
    .execute(&pool)
    .await
    .unwrap();

    let source = rustyfin_model::media_source::MediaSourceResponse {
        id: Uuid::new_v4().to_string(),
        uid: Uuid::new_v4().to_string(),
        container: Some("mkv".to_string()),
        protocol: "File".to_string(),
        path: "/media/movies/sample.mkv".to_string(),
        r#type: "Default".to_string(),
        run_time_ticks: Some(72000000000),
        size: Some(1024 * 1024 * 500),
        bitrate: Some(2500000),
        video_bitrate: Some(2000000),
        audio_bitrate: Some(500000),
        width: Some(1920),
        height: Some(1080),
        aspect_ratio: Some("16/9".to_string()),
        is_remote: false,
        media_state: None,
        media_streams: vec![
            rustyfin_model::media_source::MediaStream {
                index: 0,
                codec: Some("h264".to_string()),
                codec_type: "Video".to_string(),
                language: Some("eng".to_string()),
                time_base: Some("1/1000".to_string()),
                codec_time_base: None,
                codec_tag: None,
                is_external: false,
                path: None,
                width: Some(1920),
                height: Some(1080),
                average_frame_rate: Some("24".to_string()),
                real_frame_rate: Some("24".to_string()),
                bit_rate: Some(2000000),
                bit_depth: Some(8),
                channels: None,
                channel_layout: None,
                sample_rate: None,
                profile: Some("High".to_string()),
                r#type: None,
                aspect_ratio_text: None,
                aspect_ratio: None,
                title: Some("Main Video".to_string()),
                color_space: None,
                color_transfer: None,
                color_primaries: None,
                color_range: None,
                localized_uri: None,
                is_default: true,
                is_forced: false,
                is_interlaced: false,
                embedded_bitmaps: false,
            },
            rustyfin_model::media_source::MediaStream {
                index: 1,
                codec: Some("aac".to_string()),
                codec_type: "Audio".to_string(),
                language: Some("ger".to_string()),
                time_base: Some("1/1000".to_string()),
                codec_time_base: None,
                codec_tag: None,
                is_external: false,
                path: None,
                width: None,
                height: None,
                average_frame_rate: None,
                real_frame_rate: None,
                bit_rate: Some(500000),
                bit_depth: None,
                channels: Some(6),
                channel_layout: Some("5.1".to_string()),
                sample_rate: Some(48000),
                profile: None,
                r#type: None,
                aspect_ratio_text: None,
                aspect_ratio: None,
                title: Some("German 5.1".to_string()),
                color_space: None,
                color_transfer: None,
                color_primaries: None,
                color_range: None,
                localized_uri: None,
                is_default: true,
                is_forced: false,
                is_interlaced: false,
                embedded_bitmaps: false,
            },
        ],
        supports_transcoding: true,
        supports_direct_stream: true,
        supports_direct_play: true,
    };

    let source_id =
        rustyfin_db::repo_media_source::insert_media_source_with_streams(&pool, &item_id, &source)
            .await
            .unwrap();

    let fetched = rustyfin_db::repo_media_source::get_media_sources(&pool, &item_id)
        .await
        .unwrap();
    assert_eq!(fetched.len(), 1);
    assert_eq!(fetched[0].id, source_id.to_string());
    assert_eq!(fetched[0].media_streams.len(), 2);
    assert_eq!(fetched[0].media_streams[0].codec.as_deref(), Some("h264"));
    assert_eq!(fetched[0].media_streams[1].channels, Some(6));
}

#[tokio::test]
async fn test_get_items_filtered_no_overflow() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();

    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let item_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(item_id.to_string())
    .bind(lib.id.to_string())
    .bind("Sample Movie")
    .bind("Movie")
    .bind("/media/movies/sample.mkv")
    .execute(&pool)
    .await
    .unwrap();

    let (items, count) = rustyfin_db::repo_media::get_items_filtered(
        &pool,
        None,
        Some(lib.id),
        Some(&["Movie".to_string()]),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        0,
        10,
        true,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(count, 1);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id, item_id);
}

#[tokio::test]
async fn test_get_media_sources_batch_and_ordering() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();

    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let item_id_1 = Uuid::new_v4();
    let item_id_2 = Uuid::new_v4();

    for (item_id, name) in [(item_id_1, "Movie One"), (item_id_2, "Movie Two")] {
        sqlx::query(
            "INSERT INTO media_items (id, library_id, name, media_type, path) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(item_id.to_string())
        .bind(lib.id.to_string())
        .bind(name)
        .bind("Movie")
        .bind(format!("/media/{}.mkv", name))
        .execute(&pool)
        .await
        .unwrap();
    }

    // Source for item 1 with 2 streams
    let source_1 = rustyfin_model::media_source::MediaSourceResponse {
        id: Uuid::new_v4().to_string(),
        uid: "uid-1".to_string(),
        container: Some("mkv".to_string()),
        protocol: "File".to_string(),
        path: "/media/Movie One.mkv".to_string(),
        r#type: "Default".to_string(),
        run_time_ticks: Some(36000000000),
        size: Some(1024000),
        bitrate: Some(2000000),
        video_bitrate: Some(1500000),
        audio_bitrate: Some(500000),
        width: Some(1920),
        height: Some(1080),
        aspect_ratio: Some("16:9".to_string()),
        is_remote: false,
        media_state: None,
        media_streams: vec![
            rustyfin_model::media_source::MediaStream {
                index: 0,
                codec: Some("h264".to_string()),
                codec_type: "Video".to_string(),
                language: None,
                time_base: Some("1/1000".to_string()),
                codec_time_base: None,
                codec_tag: None,
                is_external: false,
                path: None,
                width: Some(1920),
                height: Some(1080),
                average_frame_rate: Some("24".to_string()),
                real_frame_rate: Some("24".to_string()),
                bit_rate: Some(1500000),
                bit_depth: Some(8),
                channels: None,
                channel_layout: None,
                sample_rate: None,
                profile: Some("High".to_string()),
                r#type: None,
                aspect_ratio_text: Some("16:9".to_string()),
                aspect_ratio: Some(1.777),
                title: None,
                color_space: None,
                color_transfer: None,
                color_primaries: None,
                color_range: None,
                localized_uri: None,
                is_default: true,
                is_forced: false,
                is_interlaced: false,
                embedded_bitmaps: false,
            },
            rustyfin_model::media_source::MediaStream {
                index: 1,
                codec: Some("eac3".to_string()),
                codec_type: "Audio".to_string(),
                language: Some("deu".to_string()),
                time_base: Some("1/1000".to_string()),
                codec_time_base: None,
                codec_tag: None,
                is_external: false,
                path: None,
                width: None,
                height: None,
                average_frame_rate: None,
                real_frame_rate: None,
                bit_rate: Some(256000),
                bit_depth: None,
                channels: Some(6),
                channel_layout: Some("5.1".to_string()),
                sample_rate: Some(48000),
                profile: None,
                r#type: None,
                aspect_ratio_text: None,
                aspect_ratio: None,
                title: Some("German 5.1".to_string()),
                color_space: None,
                color_transfer: None,
                color_primaries: None,
                color_range: None,
                localized_uri: None,
                is_default: true,
                is_forced: false,
                is_interlaced: false,
                embedded_bitmaps: false,
            },
        ],
        supports_transcoding: true,
        supports_direct_stream: true,
        supports_direct_play: true,
    };

    // Source for item 2 without streams
    let source_2 = rustyfin_model::media_source::MediaSourceResponse {
        id: Uuid::new_v4().to_string(),
        uid: "uid-2".to_string(),
        container: Some("avi".to_string()),
        protocol: "File".to_string(),
        path: "/media/Movie Two.avi".to_string(),
        r#type: "Default".to_string(),
        run_time_ticks: Some(18000000000),
        size: Some(512000),
        bitrate: Some(1000000),
        video_bitrate: None,
        audio_bitrate: None,
        width: None,
        height: None,
        aspect_ratio: None,
        is_remote: false,
        media_state: None,
        media_streams: vec![],
        supports_transcoding: true,
        supports_direct_stream: true,
        supports_direct_play: true,
    };

    rustyfin_db::repo_media_source::insert_media_source_with_streams(&pool, &item_id_1, &source_1)
        .await
        .unwrap();

    rustyfin_db::repo_media_source::insert_media_source_with_streams(&pool, &item_id_2, &source_2)
        .await
        .unwrap();

    let batch =
        rustyfin_db::repo_media_source::get_media_sources_batch(&pool, &[item_id_1, item_id_2])
            .await
            .unwrap();

    assert_eq!(batch.len(), 2);
    let sources_1 = batch.get(&item_id_1.to_string()).unwrap();
    assert_eq!(sources_1.len(), 1);
    assert_eq!(sources_1[0].media_streams.len(), 2);
    assert_eq!(sources_1[0].media_streams[0].index, 0);
    assert_eq!(sources_1[0].media_streams[1].index, 1);
    assert_eq!(sources_1[0].media_streams[0].codec.as_deref(), Some("h264"));
    assert_eq!(sources_1[0].media_streams[1].codec.as_deref(), Some("eac3"));

    let sources_2 = batch.get(&item_id_2.to_string()).unwrap();
    assert_eq!(sources_2.len(), 1);
    assert_eq!(sources_2[0].media_streams.len(), 0);
}

#[tokio::test]
async fn test_get_media_sources_batch_large() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("LargeLib", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    // Create 450 items (exceeding chunk size of 200)
    let mut item_ids = Vec::new();
    for i in 0..450 {
        let item_id = Uuid::new_v4();
        let path = format!("/media/movie_{}.mkv", i);
        sqlx::query(
            "INSERT INTO media_items (id, library_id, name, media_type, path) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(item_id.to_string())
        .bind(lib.id.to_string())
        .bind(format!("Movie {}", i))
        .bind("Movie")
        .bind(&path)
        .execute(&pool)
        .await
        .unwrap();

        item_ids.push(item_id);

        if i % 50 == 0 {
            let source = rustyfin_model::media_source::MediaSourceResponse {
                id: Uuid::new_v4().to_string(),
                uid: format!("uid-{}", i),
                container: Some("mkv".to_string()),
                protocol: "File".to_string(),
                path: path.clone(),
                r#type: "Default".to_string(),
                run_time_ticks: Some(36000000000),
                size: Some(1024000),
                bitrate: Some(2000000),
                video_bitrate: None,
                audio_bitrate: None,
                width: None,
                height: None,
                aspect_ratio: None,
                is_remote: false,
                media_state: None,
                media_streams: vec![],
                supports_transcoding: true,
                supports_direct_stream: true,
                supports_direct_play: true,
            };
            rustyfin_db::repo_media_source::insert_media_source_with_streams(
                &pool, &item_id, &source,
            )
            .await
            .unwrap();
        }
    }

    let batch = rustyfin_db::repo_media_source::get_media_sources_batch(&pool, &item_ids)
        .await
        .unwrap();

    // 450 items total, 9 of them (0, 50, 100, 150, 200, 250, 300, 350, 400) have sources
    assert_eq!(batch.len(), 9);
    for i in (0..450).step_by(50) {
        let id_str = item_ids[i].to_string();
        assert!(batch.contains_key(&id_str));
    }
}

#[tokio::test]
async fn test_filtered_query_with_genres_and_tags() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("FilterLib", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let id1 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path, genres) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id1.to_string())
    .bind(lib.id.to_string())
    .bind("Sci-Fi Thriller")
    .bind("Movie")
    .bind("/media/scifi.mkv")
    .bind("Sci-Fi|Thriller")
    .execute(&pool)
    .await
    .unwrap();

    let id2 = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path, genres) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(id2.to_string())
    .bind(lib.id.to_string())
    .bind("Action Comedy")
    .bind("Movie")
    .bind("/media/action.mkv")
    .bind("Action|Comedy")
    .execute(&pool)
    .await
    .unwrap();

    // Test genre filtering
    let sci_fi_genre = vec!["Sci-Fi".to_string()];
    let (items, total) = rustyfin_db::repo_media::get_items_filtered(
        &pool,
        None,
        Some(lib.id),
        None,
        None,
        Some(&sci_fi_genre),
        None,
        None,
        None,
        None,
        None,
        None,
        0,
        10,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(total, 1);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "Sci-Fi Thriller");
}

#[tokio::test]
async fn test_filtered_query_with_name_starts_with() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("AlphaLib", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let items_data = [
        (Uuid::new_v4(), "Alien", "Alien"),
        (Uuid::new_v4(), "Avatar", "Avatar"),
        (Uuid::new_v4(), "Blade Runner", "Blade Runner"),
        (Uuid::new_v4(), "The Matrix", "Matrix, The"),
        (
            Uuid::new_v4(),
            "2001: A Space Odyssey",
            "2001: A Space Odyssey",
        ),
    ];

    for (id, name, sort_name) in items_data {
        sqlx::query(
            "INSERT INTO media_items (id, library_id, name, sort_name, media_type, path) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(id.to_string())
        .bind(lib.id.to_string())
        .bind(name)
        .bind(sort_name)
        .bind("Movie")
        .bind(format!("/media/{}.mkv", name))
        .execute(&pool)
        .await
        .unwrap();
    }

    // Filter by 'A'
    let (items_a, total_a) = rustyfin_db::repo_media::get_items_filtered(
        &pool,
        None,
        Some(lib.id),
        None,
        None,
        None,
        None,
        None,
        Some("A"),
        None,
        None,
        None,
        0,
        100,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(total_a, 2);
    assert_eq!(items_a.len(), 2);

    // Filter by 'M' (matches "The Matrix" via sort_name)
    let (items_m, total_m) = rustyfin_db::repo_media::get_items_filtered(
        &pool,
        None,
        Some(lib.id),
        None,
        None,
        None,
        None,
        None,
        Some("M"),
        None,
        None,
        None,
        0,
        100,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(total_m, 1);
    assert_eq!(items_m[0].name, "The Matrix");

    // Filter by '#' (matches numbers / symbols like "2001: A Space Odyssey")
    let (items_hash, total_hash) = rustyfin_db::repo_media::get_items_filtered(
        &pool,
        None,
        Some(lib.id),
        None,
        None,
        None,
        None,
        None,
        Some("#"),
        None,
        None,
        None,
        0,
        100,
        false,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(total_hash, 1);
    assert_eq!(items_hash[0].name, "2001: A Space Odyssey");
}

// ── J5: genres, studios, years ──

use rustyfin_db::repo_by_name::{self, ByNameEntry, ByNameFilter, ByNameKind};

/// Creates a library of the given content type and returns its id.
async fn j5_library(pool: &SqlitePool, name: &str, ct: LibraryContentType) -> Uuid {
    let lib = make_library(name, ct);
    rustyfin_db::repo_library::create_library(pool, &lib)
        .await
        .unwrap();
    lib.id
}

/// Inserts an item the way the scanner stores it since migration 014 and returns its id.
async fn j5_item(
    pool: &SqlitePool,
    library_id: Uuid,
    parent_id: Option<Uuid>,
    name: &str,
    media_type: &str,
    genres: &str,
    year: Option<i64>,
) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, parent_id, name, media_type, path, genres, production_year) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(id.to_string())
    .bind(library_id.to_string())
    .bind(parent_id.map(|p| p.to_string()))
    .bind(name)
    .bind(media_type)
    .bind(format!("/media/j5/{id}"))
    .bind(genres)
    .bind(year)
    .execute(pool)
    .await
    .unwrap();
    id
}

/// Links an item to a studio, adding the studio the way TMDb metadata stores it.
async fn j5_studio(pool: &SqlitePool, item_id: Uuid, studio: &str) {
    let studio_id = format!("studio-{}", studio.trim().to_lowercase().replace(' ', "-"));
    sqlx::query("INSERT OR IGNORE INTO studios (id, name) VALUES (?, ?)")
        .bind(&studio_id)
        .bind(studio)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO item_studios (item_id, studio_id) VALUES (?, ?)")
        .bind(item_id.to_string())
        .bind(&studio_id)
        .execute(pool)
        .await
        .unwrap();
}

/// The names of the entries, in order.
fn j5_names(entries: &[ByNameEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

/// Lists `kind` under `filter` as `"<total>: <names in order>"`.
async fn j5_list(pool: &SqlitePool, kind: ByNameKind, filter: &ByNameFilter) -> String {
    let page = repo_by_name::list(pool, kind, filter).await.unwrap();
    format!("{}: {}", page.total, j5_names(&page.items).join(", "))
}

/// Looks `name` up as `kind`, checks the entry's id, and returns its kind, name and item count.
async fn j5_lookup(
    pool: &SqlitePool,
    kind: ByNameKind,
    name: &str,
) -> Option<(ByNameKind, String, i64)> {
    let entry = repo_by_name::get_by_name(pool, kind, name).await.unwrap()?;
    assert_eq!(entry.id, repo_by_name::by_name_id(kind, &entry.name));
    Some((entry.kind, entry.name, entry.counts.items))
}

#[test]
fn j5_ids_are_uuid_v5_of_the_kind_and_the_folded_name() {
    use ByNameKind::{Genre, MusicGenre, Studio, Year};
    let id = |kind: ByNameKind, name: &str| repo_by_name::by_name_id(kind, name).to_string();
    assert_eq!(
        repo_by_name::BY_NAME_NAMESPACE.to_string(),
        "bba0cef5-b539-4903-ad04-cdc4497f4ca3"
    );
    assert_eq!(id(Genre, "Action"), "195d1858-f1fc-5450-9330-73e695447ff1");
    assert_eq!(id(Genre, "Sci-Fi"), "a3577062-d445-5df8-a91e-9f6fdcc7770f");
    assert_eq!(id(Genre, "Rock"), "c7e58572-d517-5ddd-b6cd-71fd7705a75d");
    assert_eq!(
        id(MusicGenre, "Rock"),
        "5682964f-dff2-51db-8651-c91285edb0a9"
    );
    assert_eq!(id(Studio, "Pixar"), "01aa62ee-902a-5a32-9c32-aca3fdee5582");
    assert_eq!(id(Year, "1999"), "3b638efc-32dc-5eee-8ba2-c24d7c3bf319");
    // Accents fold away, so an accented name and its NFD form share the plain name's id.
    assert_eq!(id(Genre, "AÇÃO"), "ee352a6b-1d4c-581e-bbbf-ebde19828365");
    assert_eq!(id(Genre, "Acao"), "ee352a6b-1d4c-581e-bbbf-ebde19828365");
    assert_eq!(
        id(Genre, "Ac\u{327}a\u{303}o"),
        "ee352a6b-1d4c-581e-bbbf-ebde19828365"
    );
    assert_eq!(id(Genre, "Comédia"), "9cbc059f-cf43-5355-9eae-90f8875b2c67");
    assert_eq!(
        id(Genre, "Come\u{301}dia"),
        "9cbc059f-cf43-5355-9eae-90f8875b2c67"
    );
    // Case and outer spaces don't change the id; other characters do.
    assert_eq!(id(Genre, "  aCTION "), id(Genre, "Action"));
    assert_ne!(id(Genre, "Sci Fi"), id(Genre, "Sci-Fi"));
    // Accents fold as Jellyfin's tables fold them, also on letters with no decomposition such as
    // ø and ł; the kana voicing mark, which the tables leave alone, stays.
    assert_ne!(id(Genre, "ガ"), id(Genre, "カ"));
    assert_eq!(repo_by_name::by_name_id(Genre, "x").get_version_num(), 5);
    assert_eq!(repo_by_name::fold_name("  AÇÃO "), "acao");
    assert_eq!(repo_by_name::fold_name("Come\u{301}dia"), "comedia");
    assert_eq!(repo_by_name::fold_name("Bjørk ŁÓDŹ"), "bjork lodz");
    assert_eq!(repo_by_name::fold_name("ガ"), "ガ");
    // Every "ς" folds to "σ", as .NET's ToLowerInvariant lowercases "Σ" to "σ" at the end of a word
    // too, so "ΔΡΆΜΑΣ" and "δράμας" are one name and "ΜΑΣ" is found in "Μασκαράδα".
    assert_eq!(id(Genre, "ΔΡΆΜΑΣ"), "46602e53-1af1-504d-823c-c1113b8387da");
    assert_eq!(id(Genre, "δράμας"), "46602e53-1af1-504d-823c-c1113b8387da");
    assert_eq!(repo_by_name::fold_name("ΜΑΣ"), "μασ");
    // U+FFFD, "¡", "¿" and "⸘" are deleted, as Jellyfin deletes them, before the name is trimmed.
    assert_eq!(
        id(Genre, "¡Acción!"),
        "e85dbba9-8b4e-58ef-9479-5aab1317bfba"
    );
    assert_eq!(repo_by_name::fold_name("¡ Acción!"), "accion!");
    assert_eq!(repo_by_name::fold_name("¿Qué? ⸘Qué?!"), "que? que?!");
    assert_eq!(repo_by_name::fold_name("Rock\u{fffd}"), "rock");
    let kinds: Vec<_> = [Genre, MusicGenre, Studio, Year]
        .iter()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(kinds, ["Genre", "MusicGenre", "Studio", "Year"]);
}

#[test]
fn j5_split_genres_reads_pipes_and_old_json_and_drops_blank_and_repeated_names() {
    let split = repo_by_name::split_genres;
    assert_eq!(split("Action|Sci-Fi"), ["Action", "Sci-Fi"]);
    assert_eq!(split("A||B"), ["A", "B"]);
    assert_eq!(split(" Action | action |Sci-Fi "), ["Action", "Sci-Fi"]);
    assert_eq!(split(r#"["Drama", " War ","drama"]"#), ["Drama", "War"]);
    assert_eq!(split("Ação|Comédia"), ["Ação", "Comédia"]);
    // Migration 014 left compact JSON as one comma-joined name. Commas don't split.
    assert_eq!(split("Action,Sci-Fi"), ["Action,Sci-Fi"]);
    for blank in ["", "[]", " [ ] ", "|| |", "   "] {
        assert!(split(blank).is_empty(), "{blank:?}");
    }
}

#[tokio::test]
async fn j5_genres_list_each_folded_name_once_sorted_with_counts() {
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    j5_item(pool, films, None, "Alpha", "Movie", "Action|Sci-Fi", None).await;
    j5_item(pool, films, None, "Beta", "Movie", "action", None).await;
    let gamma = j5_item(pool, films, None, "Gamma", "Series", "Drama| sci-fi ", None).await;
    j5_item(pool, films, Some(gamma), "Gamma 1", "Episode", "", None).await;
    j5_item(pool, films, None, "Song", "Audio", "Rock", None).await;

    let page = repo_by_name::list(pool, ByNameKind::Genre, &ByNameFilter::default())
        .await
        .unwrap();
    assert_eq!(j5_names(&page.items), ["Action", "Drama", "Sci-Fi"]);
    assert_eq!(page.total, 3);
    for entry in &page.items {
        assert_eq!(entry.kind, ByNameKind::Genre);
        assert_eq!(
            entry.id,
            repo_by_name::by_name_id(ByNameKind::Genre, &entry.name)
        );
    }
    let counts: Vec<_> = page
        .items
        .iter()
        .map(|e| (e.counts.items, e.counts.movies, e.counts.series))
        .collect();
    assert_eq!(counts, [(2, 2, 0), (1, 0, 1), (2, 1, 1)]);
}

#[tokio::test]
async fn j5_genre_lists_honour_parent_types_names_order_and_paging() {
    use ByNameKind::Genre;
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    let shows = j5_library(pool, "J5 Shows", LibraryContentType::TvShows).await;
    j5_item(pool, films, None, "Alpha", "Movie", "Action|Sci-Fi", None).await;
    j5_item(pool, films, None, "Beta", "Movie", "Comedy", None).await;
    let show = j5_item(pool, shows, None, "Show", "Series", "Drama|Action", None).await;
    let season = j5_item(pool, shows, Some(show), "Season 1", "Season", "", None).await;
    j5_item(
        pool,
        shows,
        Some(season),
        "Pilot",
        "Episode",
        "Thriller",
        None,
    )
    .await;

    let all = "5: Action, Comedy, Drama, Sci-Fi, Thriller";
    assert_eq!(j5_list(pool, Genre, &ByNameFilter::default()).await, all);

    // A library, the items below an item (not the item itself), an unknown id.
    let under = |id| ByNameFilter {
        parent_id: Some(id),
        ..Default::default()
    };
    assert_eq!(
        j5_list(pool, Genre, &under(films)).await,
        "3: Action, Comedy, Sci-Fi"
    );
    assert_eq!(
        j5_list(pool, Genre, &under(shows)).await,
        "3: Action, Drama, Thriller"
    );
    assert_eq!(j5_list(pool, Genre, &under(show)).await, "1: Thriller");
    assert_eq!(j5_list(pool, Genre, &under(Uuid::new_v4())).await, "0: ");

    // Item types are canonical kind names, which the API's reader gives; a kind that no item has
    // matches nothing.
    let types = |include: &[&'static str], exclude: &[&'static str]| ByNameFilter {
        include_item_types: include.to_vec(),
        exclude_item_types: exclude.to_vec(),
        ..Default::default()
    };
    assert_eq!(
        j5_list(pool, Genre, &types(&["Series"], &[])).await,
        "2: Action, Drama"
    );
    assert_eq!(
        j5_list(pool, Genre, &types(&["Movie", "Episode"], &[])).await,
        "4: Action, Comedy, Sci-Fi, Thriller"
    );
    assert_eq!(
        j5_list(pool, Genre, &types(&[], &["Movie"])).await,
        "3: Action, Drama, Thriller"
    );
    assert_eq!(j5_list(pool, Genre, &types(&["BoxSet"], &[])).await, "0: ");

    // Name filters compare folded names; a blank one filters nothing.
    let search = ByNameFilter {
        search_term: Some("SCI".into()),
        ..Default::default()
    };
    assert_eq!(j5_list(pool, Genre, &search).await, "1: Sci-Fi");
    let blank = ByNameFilter {
        search_term: Some("  ".into()),
        ..Default::default()
    };
    assert_eq!(j5_list(pool, Genre, &blank).await, all);
    let starts = ByNameFilter {
        name_starts_with: Some("c".into()),
        ..Default::default()
    };
    assert_eq!(j5_list(pool, Genre, &starts).await, "1: Comedy");
    let from = ByNameFilter {
        name_starts_with_or_greater: Some("D".into()),
        ..Default::default()
    };
    assert_eq!(
        j5_list(pool, Genre, &from).await,
        "3: Drama, Sci-Fi, Thriller"
    );
    let below = ByNameFilter {
        name_less_than: Some("comedy".into()),
        ..Default::default()
    };
    assert_eq!(j5_list(pool, Genre, &below).await, "1: Action");

    // Order and paging; the total counts every entry.
    let page = |start_index, limit, descending| ByNameFilter {
        start_index,
        limit,
        descending,
        ..Default::default()
    };
    assert_eq!(
        j5_list(pool, Genre, &page(1, Some(2), false)).await,
        "5: Comedy, Drama"
    );
    assert_eq!(
        j5_list(pool, Genre, &page(0, Some(2), true)).await,
        "5: Thriller, Sci-Fi"
    );
    assert_eq!(
        j5_list(pool, Genre, &page(3, None, true)).await,
        "5: Comedy, Action"
    );
    assert_eq!(j5_list(pool, Genre, &page(0, Some(0), false)).await, "5: ");
    assert_eq!(j5_list(pool, Genre, &page(9, Some(2), false)).await, "5: ");
}

#[tokio::test]
async fn j5_music_genres_come_from_albums_and_songs_only() {
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    let tunes = j5_library(pool, "J5 Tunes", LibraryContentType::Music).await;
    let album = j5_item(
        pool,
        tunes,
        None,
        "Kind of Blue",
        "MusicAlbum",
        "Jazz|Rock",
        None,
    )
    .await;
    j5_item(
        pool,
        tunes,
        Some(album),
        "So What",
        "Audio",
        "Rock| Modal",
        None,
    )
    .await;
    j5_item(
        pool,
        films,
        None,
        "School of Rock",
        "Movie",
        "Rock|Comedy",
        None,
    )
    .await;

    let music = repo_by_name::list(pool, ByNameKind::MusicGenre, &ByNameFilter::default())
        .await
        .unwrap();
    assert_eq!(j5_names(&music.items), ["Jazz", "Modal", "Rock"]);
    let rock = &music.items[2];
    assert_eq!(rock.kind, ByNameKind::MusicGenre);
    assert_eq!(rock.id.to_string(), "5682964f-dff2-51db-8651-c91285edb0a9");
    assert_eq!(
        (rock.counts.items, rock.counts.albums, rock.counts.songs),
        (2, 1, 1)
    );
    let in_album = ByNameFilter {
        parent_id: Some(album),
        ..Default::default()
    };
    assert_eq!(
        j5_list(pool, ByNameKind::MusicGenre, &in_album).await,
        "2: Modal, Rock"
    );

    let genres = repo_by_name::list(pool, ByNameKind::Genre, &ByNameFilter::default())
        .await
        .unwrap();
    assert_eq!(j5_names(&genres.items), ["Comedy", "Rock"]);
    assert_eq!(
        genres.items[1].id.to_string(),
        "c7e58572-d517-5ddd-b6cd-71fd7705a75d"
    );
    assert_eq!(genres.items[1].counts.items, 1);
}

#[tokio::test]
async fn j5_years_are_positive_production_years_and_studios_come_from_item_studios() {
    use ByNameKind::{Studio, Year};
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    let shows = j5_library(pool, "J5 Shows", LibraryContentType::TvShows).await;
    let up = j5_item(pool, films, None, "Up", "Movie", "", Some(2009)).await;
    let coco = j5_item(pool, films, None, "Coco", "Movie", "", Some(2017)).await;
    j5_item(pool, films, None, "Matrix", "Movie", "", Some(1999)).await;
    j5_item(pool, films, None, "Beowulf", "Movie", "", Some(800)).await;
    j5_item(pool, films, None, "Undated", "Movie", "", None).await;
    j5_item(pool, films, None, "Zero", "Movie", "", Some(0)).await;
    let show = j5_item(pool, shows, None, "Show", "Series", "", Some(1999)).await;
    j5_studio(pool, up, "Pixar").await;
    j5_studio(pool, coco, "Pixar").await;
    j5_studio(pool, coco, "  ").await;
    j5_studio(pool, show, "A24").await;

    // Years sort as numbers.
    let years = repo_by_name::list(pool, Year, &ByNameFilter::default())
        .await
        .unwrap();
    assert_eq!(j5_names(&years.items), ["800", "1999", "2009", "2017"]);
    assert_eq!(years.total, 4);
    let y1999 = &years.items[1];
    assert_eq!(y1999.kind, Year);
    assert_eq!(y1999.id.to_string(), "3b638efc-32dc-5eee-8ba2-c24d7c3bf319");
    assert_eq!(
        (y1999.counts.items, y1999.counts.movies, y1999.counts.series),
        (2, 1, 1)
    );
    let newest_first = ByNameFilter {
        descending: true,
        ..Default::default()
    };
    assert_eq!(
        j5_list(pool, Year, &newest_first).await,
        "4: 2017, 2009, 1999, 800"
    );
    let series = ByNameFilter {
        include_item_types: vec!["Series"],
        ..Default::default()
    };
    assert_eq!(j5_list(pool, Year, &series).await, "1: 1999");
    // A series is no Video, so 1999 counts only the movie.
    let video = ByNameFilter {
        media_types: vec!["Video"],
        ..Default::default()
    };
    let videos = repo_by_name::list(pool, Year, &video).await.unwrap();
    assert_eq!(j5_names(&videos.items), ["800", "1999", "2009", "2017"]);
    assert_eq!(videos.items[1].counts.items, 1);

    // Studios, without the blank name.
    let studios = repo_by_name::list(pool, Studio, &ByNameFilter::default())
        .await
        .unwrap();
    assert_eq!(j5_names(&studios.items), ["A24", "Pixar"]);
    assert_eq!(
        studios.items[1].id.to_string(),
        "01aa62ee-902a-5a32-9c32-aca3fdee5582"
    );
    assert_eq!(studios.items[1].kind, Studio);
    assert_eq!(
        (
            studios.items[1].counts.items,
            studios.items[1].counts.movies
        ),
        (2, 2)
    );
    assert_eq!(studios.items[0].counts.series, 1);
    let in_films = ByNameFilter {
        parent_id: Some(films),
        ..Default::default()
    };
    assert_eq!(j5_list(pool, Studio, &in_films).await, "1: Pixar");
}

#[tokio::test]
async fn j5_single_lookups_fold_the_name_follow_slugs_and_resolve_ids() {
    use ByNameKind::{Genre, MusicGenre, Studio, Year};
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    let tunes = j5_library(pool, "J5 Tunes", LibraryContentType::Music).await;
    let genres = "Sci-Fi|Action & Adventure|Ação";
    let up = j5_item(pool, films, None, "Up", "Movie", genres, Some(1999)).await;
    j5_studio(pool, up, "Pixar").await;
    j5_item(pool, tunes, None, "Song", "Audio", "Rock", None).await;

    let found = |kind: ByNameKind, name: &str| Some((kind, name.to_string(), 1_i64));
    assert_eq!(
        j5_lookup(pool, Genre, "sci-fi").await,
        found(Genre, "Sci-Fi")
    );
    assert_eq!(j5_lookup(pool, Genre, " AÇÃO ").await, found(Genre, "Ação"));
    // jellyfin-web writes `&`, `/` and `?` in names as `-`.
    assert_eq!(
        j5_lookup(pool, Genre, "Action - Adventure").await,
        found(Genre, "Action & Adventure")
    );
    assert_eq!(j5_lookup(pool, Genre, "Western").await, None);
    assert_eq!(j5_lookup(pool, Genre, "Rock").await, None);
    assert_eq!(
        j5_lookup(pool, MusicGenre, "ROCK").await,
        found(MusicGenre, "Rock")
    );
    assert_eq!(
        j5_lookup(pool, Studio, "pixar").await,
        found(Studio, "Pixar")
    );
    assert_eq!(j5_lookup(pool, Year, "1999").await, found(Year, "1999"));
    for missing in ["2000", "0", "-5", "abc", ""] {
        assert_eq!(j5_lookup(pool, Year, missing).await, None, "{missing:?}");
    }

    let known = [
        (Genre, "Sci-Fi"),
        (Genre, "Ação"),
        (MusicGenre, "Rock"),
        (Studio, "Pixar"),
        (Year, "1999"),
    ];
    for (kind, name) in known {
        let id = repo_by_name::by_name_id(kind, name);
        let entry = repo_by_name::get_by_id(pool, id)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{kind:?} {name}"));
        assert_eq!(
            (entry.kind, entry.name.as_str(), entry.id),
            (kind, name, id)
        );
    }
    let unknown = repo_by_name::get_by_id(pool, Uuid::new_v4()).await.unwrap();
    assert_eq!(unknown, None);
    // "Rock" is only a music genre here, so its Genre id names nothing.
    let rock = repo_by_name::by_name_id(Genre, "Rock");
    assert_eq!(repo_by_name::get_by_id(pool, rock).await.unwrap(), None);
}

#[tokio::test]
async fn j5_old_json_rows_and_odd_separators_list_without_error() {
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    j5_item(pool, films, None, "Pipes", "Movie", "A||B", None).await;
    j5_item(
        pool,
        films,
        None,
        "Spaces",
        "Movie",
        "  Drama  | |War ",
        None,
    )
    .await;
    // Rows from before migration 014 keep JSON in both columns.
    let old = r#"["Western", "Noir"]"#;
    j5_item(pool, films, None, "Old", "\"Movie\"", old, None).await;
    j5_item(pool, films, None, "Empty", "Movie", "[]", None).await;
    j5_item(pool, films, None, "Accents", "Movie", "Ação|Comédia", None).await;

    let all = "8: A, Ação, B, Comédia, Drama, Noir, War, Western";
    let genres = j5_list(pool, ByNameKind::Genre, &ByNameFilter::default()).await;
    assert_eq!(genres, all);
    let movies = ByNameFilter {
        include_item_types: vec!["Movie"],
        ..Default::default()
    };
    assert_eq!(j5_list(pool, ByNameKind::Genre, &movies).await, all);
    let years = j5_list(pool, ByNameKind::Year, &ByNameFilter::default()).await;
    assert_eq!(years, "0: ");
}

#[tokio::test]
async fn j5_entries_are_the_same_after_the_items_are_stored_again() {
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    let rows = [
        ("Alpha", "Action|Sci-Fi", 2001),
        ("Beta", "sci-fi|Drama", 2002),
    ];
    for (name, genres, year) in rows {
        j5_item(pool, films, None, name, "Movie", genres, Some(year)).await;
    }
    let before = j5_genre_and_year_entries(pool).await;
    assert_eq!(before.len(), 5);

    // A rescan that stores every item again, under new ids and in another order.
    sqlx::query("DELETE FROM media_items WHERE library_id = ?")
        .bind(films.to_string())
        .execute(pool)
        .await
        .unwrap();
    for (name, genres, year) in rows.into_iter().rev() {
        j5_item(pool, films, None, name, "Movie", genres, Some(year)).await;
    }
    assert_eq!(j5_genre_and_year_entries(pool).await, before);
}

/// Every genre entry, then every year entry.
async fn j5_genre_and_year_entries(pool: &SqlitePool) -> Vec<ByNameEntry> {
    let mut all = Vec::new();
    for kind in [ByNameKind::Genre, ByNameKind::Year] {
        let page = repo_by_name::list(pool, kind, &ByNameFilter::default())
            .await
            .unwrap();
        all.extend(page.items);
    }
    all
}

#[tokio::test]
async fn j5_an_entry_keeps_its_name_under_every_scope_and_type_filter() {
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    let docs = j5_library(pool, "J5 Docs", LibraryContentType::Movies).await;
    let tunes = j5_library(pool, "J5 Tunes", LibraryContentType::Music).await;
    j5_item(pool, films, None, "Alpha", "Movie", "Action", None).await;
    j5_item(pool, docs, None, "Bravo", "Movie", " action |Drama", None).await;
    let album = j5_item(pool, tunes, None, "Blue", "MusicAlbum", "Rock|Jazz", None).await;
    j5_item(pool, tunes, Some(album), "One", "Audio", "rock", None).await;

    // Only Bravo counts in J5 Docs, but its genre keeps the name it has in every other list.
    let in_docs = ByNameFilter {
        parent_id: Some(docs),
        ..Default::default()
    };
    let page = repo_by_name::list(pool, ByNameKind::Genre, &in_docs)
        .await
        .unwrap();
    assert_eq!(j5_names(&page.items), ["Action", "Drama"]);
    assert_eq!(page.items[0].counts.items, 1);
    assert_eq!(
        page.items[0].id.to_string(),
        "195d1858-f1fc-5450-9330-73e695447ff1"
    );

    // The same under a type filter. Only the song counts, so Jazz, which only the album has, is left out.
    let songs = ByNameFilter {
        include_item_types: vec!["Audio"],
        ..Default::default()
    };
    let page = repo_by_name::list(pool, ByNameKind::MusicGenre, &songs)
        .await
        .unwrap();
    assert_eq!(j5_names(&page.items), ["Rock"]);
    let rock = &page.items[0];
    assert_eq!(
        (rock.counts.items, rock.counts.albums, rock.counts.songs),
        (1, 0, 1)
    );
    let found = repo_by_name::get_by_id(pool, rock.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        (found.kind, found.name.as_str()),
        (ByNameKind::MusicGenre, "Rock")
    );
}

#[tokio::test]
async fn j5_accented_and_nfd_spellings_are_one_entry() {
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    for (name, genres) in [
        ("A", "Acao|Comedia"),
        ("B", "Acao|Comedia"),
        ("C", "Ação|Comédia"),
        ("D", "Ac\u{327}a\u{303}o|Come\u{301}dia"),
    ] {
        j5_item(pool, films, None, name, "Movie", genres, None).await;
    }

    let page = repo_by_name::list(pool, ByNameKind::Genre, &ByNameFilter::default())
        .await
        .unwrap();
    assert_eq!(j5_names(&page.items), ["Acao", "Comedia"]);
    for (entry, accented) in page.items.iter().zip(["Ação", "Comédia"]) {
        assert_eq!(entry.counts.items, 4, "{}", entry.name);
        assert_eq!(
            entry.id,
            repo_by_name::by_name_id(ByNameKind::Genre, accented)
        );
    }
    for name in ["AÇÃO", "ac\u{327}a\u{303}o", "comédia"] {
        let found = j5_lookup(pool, ByNameKind::Genre, name).await;
        assert_eq!(found.map(|f| f.2), Some(4), "{name}");
    }
    let search = ByNameFilter {
        search_term: Some("ÇÃ".to_string()),
        ..Default::default()
    };
    assert_eq!(j5_list(pool, ByNameKind::Genre, &search).await, "1: Acao");
}

#[tokio::test]
async fn j5_an_entry_is_named_by_its_most_common_spelling() {
    let db = test_pool().await;
    let pool = &db.pool;
    let films = j5_library(pool, "J5 Films", LibraryContentType::Movies).await;
    let odd = j5_library(pool, "J5 Odd", LibraryContentType::Movies).await;
    for name in ["A", "B", "C"] {
        j5_item(pool, films, None, name, "Movie", "Sci-Fi", None).await;
    }
    j5_item(pool, odd, None, "D", "Movie", "SCI-FI|DRAMA", None).await;
    j5_item(pool, films, None, "E", "Movie", "Drama", None).await;

    // Three Sci-Fi beat one SCI-FI, also where only the SCI-FI item counts. A tie goes to the
    // smallest spelling, so DRAMA beats Drama.
    assert_eq!(
        j5_list(pool, ByNameKind::Genre, &ByNameFilter::default()).await,
        "2: DRAMA, Sci-Fi"
    );
    let in_odd = ByNameFilter {
        parent_id: Some(odd),
        ..Default::default()
    };
    assert_eq!(
        j5_list(pool, ByNameKind::Genre, &in_odd).await,
        "2: DRAMA, Sci-Fi"
    );
    let found = j5_lookup(pool, ByNameKind::Genre, "sci-fi").await;
    assert_eq!(found, Some((ByNameKind::Genre, "Sci-Fi".to_string(), 4)));
}

// ── Media Item Repository Tests ──

fn make_movie(library_id: Uuid, name: &str) -> MediaItem {
    MediaItem {
        id: Uuid::new_v4(),
        name: name.to_string(),
        sort_name: Some(name.to_lowercase()),
        media_type: MediaType::Movie,
        path: format!("/media/movies/{name}.mkv"),
        library_id,
        production_year: Some(2020),
        popularity: Some(12.5),
        ..MediaItem::default_dto_fields()
    }
}

#[tokio::test]
async fn test_upsert_media_item_round_trip() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();

    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let movie = make_movie(lib.id, "Test Movie");
    rustyfin_db::repo_media::upsert_media_item(&pool, &movie)
        .await
        .unwrap();

    let stored = rustyfin_db::repo_media::get_item_by_id(&pool, movie.id)
        .await
        .unwrap()
        .expect("item should exist after upsert");
    assert_eq!(stored.name, "Test Movie");
    assert_eq!(stored.path, movie.path);
    assert_eq!(stored.production_year, Some(2020));
    assert_eq!(stored.popularity, Some(12.5));
}

#[tokio::test]
async fn test_batch_upsert_media_items_round_trip() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();

    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();

    let movies = vec![make_movie(lib.id, "First"), make_movie(lib.id, "Second")];
    rustyfin_db::repo_media::batch_upsert_media_items(&pool, &movies)
        .await
        .unwrap();

    for movie in &movies {
        let stored = rustyfin_db::repo_media::get_item_by_id(&pool, movie.id)
            .await
            .unwrap()
            .expect("item should exist after batch upsert");
        assert_eq!(stored.name, movie.name);
        assert_eq!(stored.popularity, Some(12.5));
    }
}

/// A batch that saves a stored item again updates it in place (B31): the rows that reference it
/// and its date added stay, and the year and rating counts follow its new values.
#[tokio::test]
async fn test_batch_upsert_updates_a_stored_item_in_place() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();

    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();
    let added: chrono::DateTime<Utc> = "2020-01-02T03:04:05Z".parse().unwrap();
    let movie = MediaItem {
        official_rating: Some("R".to_string()),
        date_created: added,
        ..make_movie(lib.id, "Stored")
    };
    let other = MediaItem {
        official_rating: Some("R".to_string()),
        ..make_movie(lib.id, "Other")
    };
    rustyfin_db::repo_media::batch_upsert_media_items(&pool, &[movie.clone(), other])
        .await
        .unwrap();
    sqlx::query("INSERT INTO item_images (item_id, image_type, path) VALUES (?1, 'Primary', ?2)")
        .bind(movie.id.to_string())
        .bind("/media/movies/Stored.jpg")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO media_sources (id, item_id, uid, path) VALUES (?1, ?2, ?1, ?3)")
        .bind(Uuid::new_v4().to_string())
        .bind(movie.id.to_string())
        .bind(&movie.path)
        .execute(&pool)
        .await
        .unwrap();

    let changed = MediaItem {
        name: "Renamed".to_string(),
        production_year: Some(2021),
        official_rating: Some("PG".to_string()),
        date_created: Utc::now(),
        ..movie.clone()
    };
    rustyfin_db::repo_media::batch_upsert_media_items(&pool, &[changed])
        .await
        .unwrap();

    let stored = rustyfin_db::repo_media::get_item_by_id(&pool, movie.id)
        .await
        .unwrap()
        .expect("the item is still stored under its id");
    assert_eq!(stored.name, "Renamed");
    let kept: (i64, i64) = sqlx::query_as(
        "SELECT (SELECT COUNT(*) FROM item_images WHERE item_id = ?1), \
                (SELECT COUNT(*) FROM media_sources WHERE item_id = ?1)",
    )
    .bind(movie.id.to_string())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kept, (1, 1), "the image and the media source stay");
    assert_eq!(stored.date_created, added);
    let years: Vec<(i64, i64)> =
        sqlx::query_as("SELECT year, item_count FROM filter_year_index ORDER BY year")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(years, [(2020, 1), (2021, 1)]);
    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT rating, item_count FROM filter_rating_index ORDER BY rating")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, [("PG".to_string(), 1), ("R".to_string(), 1)]);
}

/// The year filter's values with their item counts.
async fn filter_years(pool: &SqlitePool) -> Vec<(i64, i64)> {
    sqlx::query_as("SELECT year, item_count FROM filter_year_index ORDER BY year")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// The rating filter's values with their item counts.
async fn filter_ratings(pool: &SqlitePool) -> Vec<(String, i64)> {
    sqlx::query_as("SELECT rating, item_count FROM filter_rating_index ORDER BY rating")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Stores a 2020, R movie and a 2021, PG one, and returns the first.
async fn store_two_rated_movies(pool: &SqlitePool) -> MediaItem {
    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(pool, &lib)
        .await
        .unwrap();
    let movie = MediaItem {
        official_rating: Some("R".to_string()),
        ..make_movie(lib.id, "Last")
    };
    let other = MediaItem {
        production_year: Some(2021),
        official_rating: Some("PG".to_string()),
        ..make_movie(lib.id, "Other")
    };
    for item in [&movie, &other] {
        rustyfin_db::repo_media::upsert_media_item(pool, item)
            .await
            .unwrap();
    }
    assert_eq!(filter_years(pool).await, [(2020, 1), (2021, 1)]);
    assert_eq!(
        filter_ratings(pool).await,
        [("PG".to_string(), 1), ("R".to_string(), 1)]
    );
    movie
}

/// B31 review 1, finding 4: a year or rating leaves the filters once no item has it. Here the only
/// 2020, R movie is saved as 2022, PG-13.
#[tokio::test]
async fn test_filters_drop_a_value_when_its_last_item_moves_off_it() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();

    let movie = store_two_rated_movies(&pool).await;
    let moved = MediaItem {
        production_year: Some(2022),
        official_rating: Some("PG-13".to_string()),
        ..movie
    };
    rustyfin_db::repo_media::upsert_media_item(&pool, &moved)
        .await
        .unwrap();

    assert_eq!(
        (filter_years(&pool).await, filter_ratings(&pool).await),
        (
            vec![(2021, 1), (2022, 1)],
            vec![("PG".to_string(), 1), ("PG-13".to_string(), 1)]
        ),
        "no item has 2020 or R any more"
    );
}

/// B31 review 1, finding 4: the same when the only 2020, R movie is deleted.
#[tokio::test]
async fn test_filters_drop_a_value_when_its_last_item_is_deleted() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();

    let movie = store_two_rated_movies(&pool).await;
    assert!(
        rustyfin_db::repo_media::delete_item(&pool, movie.id)
            .await
            .unwrap()
    );

    assert_eq!(
        (filter_years(&pool).await, filter_ratings(&pool).await),
        (vec![(2021, 1)], vec![("PG".to_string(), 1)]),
        "no item has 2020 or R any more"
    );
}

/// B26 review 1, finding 2: `save_chapters` saves all of an item's chapters or none of them. A save
/// that fails part way, here at a second chapter with the first one's id, leaves the chapters that
/// the item had before.
#[tokio::test]
async fn test_save_chapters_keeps_the_old_ones_when_it_fails() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("Movies", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();
    let item_id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO media_items (id, library_id, name, media_type, path) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(item_id.to_string())
    .bind(lib.id.to_string())
    .bind("Sample Movie")
    .bind("Movie")
    .bind("/media/movies/sample.mkv")
    .execute(&pool)
    .await
    .unwrap();

    let chapter = |id: &str, number: i32| rustyfin_model::chapter::Chapter {
        id: id.to_string(),
        item_id: item_id.to_string(),
        chapter_number: number,
        name: format!("Chapter {number}"),
        start_position_ticks: i64::from(number - 1) * 10_000_000,
        end_position_ticks: i64::from(number) * 10_000_000,
        image_path: None,
    };
    rustyfin_db::repo_chapter::save_chapters(
        &pool,
        item_id,
        vec![chapter("c1", 1), chapter("c2", 2)],
    )
    .await
    .unwrap();

    let failed = rustyfin_db::repo_chapter::save_chapters(
        &pool,
        item_id,
        vec![chapter("c3", 1), chapter("c3", 2)],
    )
    .await;
    assert!(
        failed.is_err(),
        "two chapters with one id were saved: {failed:?}"
    );
    let kept: Vec<(String, String)> = rustyfin_db::repo_chapter::get_chapters(&pool, item_id)
        .await
        .unwrap()
        .into_iter()
        .map(|chapter| (chapter.id, chapter.name))
        .collect();
    assert_eq!(
        kept,
        [
            ("c1".to_string(), "Chapter 1".to_string()),
            ("c2".to_string(), "Chapter 2".to_string())
        ],
        "the failed save left part of its chapters"
    );
}

// ── B34: the filter trigger's rating recounts use an index ──

const B34_ITEMS: i64 = 20_000;
const B34_RATINGS: [&str; 8] = ["G", "PG", "PG-13", "R", "NC-17", "TV-14", "TV-MA", "NR"];

/// The details of `EXPLAIN QUERY PLAN <sql>` with `R` bound to `?1`.
async fn b34_plan(pool: &SqlitePool, sql: &'static str) -> Vec<String> {
    let rows: Vec<(i64, i64, i64, String)> =
        sqlx::query_as(sql).bind("R").fetch_all(pool).await.unwrap();
    rows.into_iter().map(|(_, _, _, detail)| detail).collect()
}

/// Stores `B34_ITEMS` movies with no year or rating, as a first import finds them. A helper index
/// keeps the insert trigger's recounts from scanning the table while they are stored, whatever the
/// migrations made; it is dropped again before anything is timed. Returns their ids.
async fn b34_items(pool: &SqlitePool) -> Vec<String> {
    let library = make_library("Film", LibraryContentType::Movies);
    rustyfin_db::repo_library::create_library(pool, &library)
        .await
        .unwrap();
    sqlx::query("CREATE INDEX b34_insert_helper ON media_items(official_rating)")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "WITH RECURSIVE n(i) AS (SELECT 0 UNION ALL SELECT i + 1 FROM n WHERE i + 1 < ?1) \
         INSERT INTO media_items \
         (id, name, sort_name, media_type, path, library_id, genres, production_countries) \
         SELECT printf('b34-%05d', i), printf('Film %05d', i), printf('film %05d', i), 'Movie', \
          printf('/media/film/%05d.mkv', i), ?2, '', '' FROM n",
    )
    .bind(B34_ITEMS)
    .bind(library.id.to_string())
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("DROP INDEX b34_insert_helper")
        .execute(pool)
        .await
        .unwrap();
    sqlx::query_scalar("SELECT id FROM media_items ORDER BY id")
        .fetch_all(pool)
        .await
        .unwrap()
}

/// Writes every item once with the Jellyfin import's UPDATE (`repo_jellyfin_import.rs`,
/// `write_metadata`) in one transaction, giving each a year and a rating. Stops once `limit` has
/// passed. Returns how many items it wrote and how long it took.
async fn b34_import_update(
    pool: &SqlitePool,
    ids: &[String],
    limit: std::time::Duration,
) -> (usize, std::time::Duration) {
    let start = std::time::Instant::now();
    let mut tx = pool.begin().await.unwrap();
    let mut written = 0;
    for (i, id) in ids.iter().enumerate() {
        if start.elapsed() > limit {
            break;
        }
        sqlx::query(
            "UPDATE media_items SET name = ?, sort_name = ?, overview = ?, tagline = ?, \
             official_rating = ?, community_rating = ?, premiere_date = ?, end_date = ?, \
             production_year = ?, genres = ?, tags = ?, tmdb_id = ?, tvdb_id = ? WHERE id = ?",
        )
        .bind(format!("Film {i:05}"))
        .bind(format!("film {i:05}"))
        .bind("Imported overview")
        .bind(None::<String>)
        .bind(B34_RATINGS[i % B34_RATINGS.len()])
        .bind(7.0_f64)
        .bind(None::<String>)
        .bind(None::<String>)
        .bind(1950 + (i % 70) as i64)
        .bind("Drama")
        .bind("")
        .bind(None::<i64>)
        .bind(None::<i64>)
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();
        written += 1;
    }
    tx.commit().await.unwrap();
    (written, start.elapsed())
}

/// The update trigger recounts the old and new rating of every item whose rating is written,
/// with these two statements. Both look the rating up in its index instead of scanning the table.
#[tokio::test]
async fn b34_rating_recounts_use_the_index() {
    let db = test_pool().await;
    for sql in [
        "EXPLAIN QUERY PLAN \
         SELECT ?1, COUNT(*) FROM media_items WHERE official_rating = ?1 GROUP BY ?1",
        "EXPLAIN QUERY PLAN SELECT 1 FROM media_items WHERE official_rating = ?1",
    ] {
        let plan = b34_plan(&db.pool, sql).await;
        assert!(
            plan.iter()
                .any(|step| step.contains("idx_media_items_official_rating")),
            "{sql}: {plan:?}"
        );
    }
}

/// An import that writes 20,000 items in one transaction holds the write lock for well under the
/// 30 s busy timeout other writers wait. Without the index each item's recounts scanned the table
/// twice, about 35-43 s in all (B20's re-check 9). It times the debug build's SQLite, and its time
/// grows several times over on a loaded machine, so like the timing without the index below it only
/// runs when asked; `b34_rating_recounts_use_the_index` guards the index on every run.
#[tokio::test]
#[ignore]
async fn b34_import_update_of_20000_items_is_quick() {
    let db = test_pool().await;
    let ids = b34_items(&db.pool).await;
    let limit = std::time::Duration::from_secs(15);

    let (written, took) = b34_import_update(&db.pool, &ids, limit).await;

    eprintln!("b34: import UPDATE of {written} items took {took:?}");
    assert_eq!(written, ids.len(), "stopped after {limit:?}");
    let ratings: i64 = sqlx::query_scalar("SELECT SUM(item_count) FROM filter_rating_index")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(ratings, B34_ITEMS, "every item counted under its rating");
}

/// The same import with the rating index dropped, for the before timing. It takes minutes. Run both
/// timings with `cargo test -p rustyfin-db --test repo_tests b34_import -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn b34_import_update_of_20000_items_without_the_index() {
    let db = test_pool().await;
    let ids = b34_items(&db.pool).await;
    sqlx::query("DROP INDEX IF EXISTS idx_media_items_official_rating")
        .execute(&db.pool)
        .await
        .unwrap();

    let (written, took) =
        b34_import_update(&db.pool, &ids, std::time::Duration::from_secs(1800)).await;

    eprintln!("b34: import UPDATE of {written} items without the index took {took:?}");
}

// ── B20: one-time setup codes ──

async fn b20_password(pool: &SqlitePool, user: Uuid) -> String {
    rustyfin_db::repo_user::get_user_by_id(pool, user)
        .await
        .unwrap()
        .unwrap()
        .password_hash
}

#[tokio::test]
async fn b20_redeem_needs_the_current_code() {
    use rustyfin_db::repo_setup_code::{code_hash, create_user_with_code, redeem, replace_code};
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let user = make_user("pending", false);
    create_user_with_code(&pool, &user, "hash-1").await.unwrap();
    assert_eq!(
        code_hash(&pool, user.id).await.unwrap().as_deref(),
        Some("hash-1")
    );
    assert_eq!(code_hash(&pool, Uuid::new_v4()).await.unwrap(), None);

    // A reissue replaces the hash; the old one then redeems nothing.
    assert!(
        replace_code(&pool, user.id, "hash-2", Utc::now())
            .await
            .unwrap()
    );
    assert!(!redeem(&pool, user.id, "hash-1", "pw-1").await.unwrap());
    assert_eq!(b20_password(&pool, user.id).await, user.password_hash);
    assert_eq!(
        code_hash(&pool, user.id).await.unwrap().as_deref(),
        Some("hash-2")
    );

    // The current hash sets the password and removes the code, once.
    assert!(redeem(&pool, user.id, "hash-2", "pw-2").await.unwrap());
    assert_eq!(b20_password(&pool, user.id).await, "pw-2");
    assert_eq!(code_hash(&pool, user.id).await.unwrap(), None);
    assert!(!redeem(&pool, user.id, "hash-2", "pw-3").await.unwrap());
    assert_eq!(b20_password(&pool, user.id).await, "pw-2");
    assert!(
        !replace_code(&pool, user.id, "hash-3", Utc::now())
            .await
            .unwrap()
    );
    assert_eq!(code_hash(&pool, user.id).await.unwrap(), None);
}

#[tokio::test]
async fn b20_pending_users_are_sorted_by_name() {
    use rustyfin_db::repo_setup_code::{PendingUser, create_user_with_code, pending_users};
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    assert!(pending_users(&pool).await.unwrap().is_empty());
    let carol = make_user("Carol", false);
    let bob = make_user("bob", false);
    let alice = make_user("alice", false);
    for user in [&carol, &bob, &alice] {
        create_user_with_code(&pool, user, "hash").await.unwrap();
    }
    let dave = make_user("dave", false);
    rustyfin_db::repo_user::create_user(&pool, &dave)
        .await
        .unwrap();

    let pending = |user: &User| PendingUser {
        user_id: user.id,
        name: user.name.clone(),
        created_at: user.created_at,
    };
    let expected = vec![pending(&alice), pending(&bob), pending(&carol)];
    assert_eq!(pending_users(&pool).await.unwrap(), expected);

    let later = Utc::now() + chrono::Duration::seconds(5);
    let replaced = rustyfin_db::repo_setup_code::replace_code(&pool, bob.id, "hash-2", later);
    assert!(replaced.await.unwrap());
    let listed = pending_users(&pool).await.unwrap();
    assert_eq!(listed[1].user_id, bob.id);
    assert_eq!(listed[1].created_at, later);
}

#[tokio::test]
async fn b20_password_writes_end_the_code() {
    use rustyfin_db::repo_setup_code::{code_hash, create_user_with_code};
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let bob = make_user("bob", false);
    let carol = make_user("carol", false);
    for user in [&bob, &carol] {
        create_user_with_code(&pool, user, "hash").await.unwrap();
    }
    let updated = rustyfin_db::repo_user::update_password(&pool, bob.id, "newhash");
    assert!(updated.await.unwrap());
    assert_eq!(b20_password(&pool, bob.id).await, "newhash");
    assert_eq!(code_hash(&pool, bob.id).await.unwrap(), None);
    assert_eq!(
        code_hash(&pool, carol.id).await.unwrap().as_deref(),
        Some("hash")
    );
    let unknown = rustyfin_db::repo_user::update_password(&pool, Uuid::new_v4(), "x");
    assert!(!unknown.await.unwrap());
    assert_eq!(
        code_hash(&pool, carol.id).await.unwrap().as_deref(),
        Some("hash")
    );
}

/// An episode of `season` with the numbers `index` to `end`.
fn b33_episode(library_id: Uuid, season: Uuid, index: i32, end: Option<i32>) -> MediaItem {
    MediaItem {
        id: Uuid::new_v4(),
        name: format!("Hotel Series - S01E{index:02}"),
        media_type: MediaType::Episode,
        path: format!("/media/tv/Hotel Series/Season 01/S01E{index:02}.mkv"),
        parent_id: Some(season),
        library_id,
        index_number: Some(index),
        parent_index_number: Some(1),
        index_number_end: end,
        ..MediaItem::default_dto_fields()
    }
}

#[tokio::test]
async fn b33_both_upserts_and_the_item_reads_keep_the_last_episode() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("TV", LibraryContentType::TvShows);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();
    let series = MediaItem {
        id: Uuid::new_v4(),
        name: "Hotel Series".to_string(),
        media_type: MediaType::Series,
        path: "/media/tv/Hotel Series".to_string(),
        library_id: lib.id,
        ..MediaItem::default_dto_fields()
    };
    let season = MediaItem {
        id: Uuid::new_v4(),
        name: "Season 01".to_string(),
        media_type: MediaType::Season,
        path: "/media/tv/Hotel Series/Season 01".to_string(),
        parent_id: Some(series.id),
        library_id: lib.id,
        index_number: Some(1),
        ..MediaItem::default_dto_fields()
    };
    for folder in [&series, &season] {
        rustyfin_db::repo_media::upsert_media_item(&pool, folder)
            .await
            .unwrap();
    }
    let double = b33_episode(lib.id, season.id, 3, Some(4));
    rustyfin_db::repo_media::upsert_media_item(&pool, &double)
        .await
        .unwrap();
    let batch = [
        b33_episode(lib.id, season.id, 5, Some(6)),
        b33_episode(lib.id, season.id, 7, None),
    ];
    rustyfin_db::repo_media::batch_upsert_media_items(&pool, &batch)
        .await
        .unwrap();

    let expected = [(3, Some(4)), (5, Some(6)), (7, None)];
    for (item, want) in [&double, &batch[0], &batch[1]].into_iter().zip(expected) {
        let stored = rustyfin_db::repo_media::get_item_by_id(&pool, item.id)
            .await
            .unwrap()
            .expect("the episode is stored");
        assert_eq!(
            (stored.index_number.unwrap(), stored.index_number_end),
            want
        );
    }
    let children = rustyfin_db::repo_media::get_children(&pool, season.id)
        .await
        .unwrap();
    let episodes = rustyfin_db::repo_media::get_episodes_for_series(&pool, series.id, Some(1))
        .await
        .unwrap();
    for read in [children, episodes] {
        let mut ends: Vec<_> = read
            .iter()
            .map(|episode| (episode.index_number.unwrap(), episode.index_number_end))
            .collect();
        ends.sort();
        assert_eq!(ends, expected);
    }
}

/// Season `index` of `series`, with `location_type`: Virtual for one made for loose files.
fn b32_season(series: &MediaItem, index: i32, location_type: LocationType) -> MediaItem {
    MediaItem {
        id: Uuid::new_v4(),
        name: format!("Season {index}"),
        media_type: MediaType::Season,
        path: format!("{}/Season {index}", series.path),
        parent_id: Some(series.id),
        library_id: series.library_id,
        index_number: Some(index),
        tmdb_id: Some(4000 + i64::from(index)),
        location_type,
        ..MediaItem::default_dto_fields()
    }
}

#[tokio::test]
async fn b32_both_upserts_and_the_item_reads_keep_a_season_virtual() {
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("TV", LibraryContentType::TvShows);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();
    let series = MediaItem {
        id: Uuid::new_v4(),
        name: "One Night".to_string(),
        media_type: MediaType::Series,
        path: "/media/tv/One Night (2023)".to_string(),
        library_id: lib.id,
        ..MediaItem::default_dto_fields()
    };
    rustyfin_db::repo_media::upsert_media_item(&pool, &series)
        .await
        .unwrap();
    let one = b32_season(&series, 1, LocationType::Virtual);
    rustyfin_db::repo_media::upsert_media_item(&pool, &one)
        .await
        .unwrap();
    let batch = [
        b32_season(&series, 2, LocationType::Virtual),
        b32_season(&series, 3, LocationType::FileSystem),
    ];
    rustyfin_db::repo_media::batch_upsert_media_items(&pool, &batch)
        .await
        .unwrap();

    // The TMDB id is bound beside the location type, so a misplaced bind would show in it.
    let expected = [
        (1, Some(4001), LocationType::Virtual),
        (2, Some(4002), LocationType::Virtual),
        (3, Some(4003), LocationType::FileSystem),
    ];
    for (season, want) in [&one, &batch[0], &batch[1]].into_iter().zip(expected) {
        let stored = rustyfin_db::repo_media::get_item_by_id(&pool, season.id)
            .await
            .unwrap()
            .expect("the season is stored");
        assert_eq!(
            (
                stored.index_number.unwrap(),
                stored.tmdb_id,
                stored.location_type
            ),
            want
        );
    }
    let mut seasons: Vec<_> = rustyfin_db::repo_media::get_children(&pool, series.id)
        .await
        .unwrap()
        .iter()
        .map(|season| {
            (
                season.index_number.unwrap(),
                season.tmdb_id,
                season.location_type,
            )
        })
        .collect();
    seasons.sort_by_key(|season| season.0);
    assert_eq!(seasons, expected);
}

#[tokio::test]
async fn b32_f9_both_upserts_update_the_last_episode_and_the_location_type() {
    // B31 made both upserts ON CONFLICT(id) DO UPDATE, so each SET list must name both columns.
    let temp_db = test_pool().await;
    let pool = temp_db.pool.clone();
    let lib = make_library("TV", LibraryContentType::TvShows);
    rustyfin_db::repo_library::create_library(&pool, &lib)
        .await
        .unwrap();
    let series = MediaItem {
        id: Uuid::new_v4(),
        name: "Hotel Series".to_string(),
        media_type: MediaType::Series,
        path: "/media/tv/Hotel Series".to_string(),
        library_id: lib.id,
        ..MediaItem::default_dto_fields()
    };
    rustyfin_db::repo_media::upsert_media_item(&pool, &series)
        .await
        .unwrap();
    let single = b32_season(&series, 1, LocationType::Virtual);
    let batched = b32_season(&series, 2, LocationType::Virtual);
    let single_episode = b33_episode(lib.id, single.id, 3, Some(4));
    let batched_episode = b33_episode(lib.id, batched.id, 5, Some(6));
    for item in [&single, &single_episode] {
        rustyfin_db::repo_media::upsert_media_item(&pool, item)
            .await
            .unwrap();
    }
    rustyfin_db::repo_media::batch_upsert_media_items(
        &pool,
        &[batched.clone(), batched_episode.clone()],
    )
    .await
    .unwrap();

    // Each one again, with new values.
    let single = MediaItem {
        location_type: LocationType::FileSystem,
        ..single
    };
    let batched = MediaItem {
        location_type: LocationType::FileSystem,
        ..batched
    };
    let single_episode = MediaItem {
        index_number_end: None,
        ..single_episode
    };
    let batched_episode = MediaItem {
        index_number_end: Some(7),
        ..batched_episode
    };
    for item in [&single, &single_episode] {
        rustyfin_db::repo_media::upsert_media_item(&pool, item)
            .await
            .unwrap();
    }
    rustyfin_db::repo_media::batch_upsert_media_items(
        &pool,
        &[batched.clone(), batched_episode.clone()],
    )
    .await
    .unwrap();

    for (item, want) in [
        (&single, (None, LocationType::FileSystem)),
        (&batched, (None, LocationType::FileSystem)),
        (&single_episode, (None, LocationType::FileSystem)),
        (&batched_episode, (Some(7), LocationType::FileSystem)),
    ] {
        let stored = rustyfin_db::repo_media::get_item_by_id(&pool, item.id)
            .await
            .unwrap()
            .expect("the item is stored");
        assert_eq!(
            (stored.index_number_end, stored.location_type),
            want,
            "{}",
            item.name
        );
    }
}
