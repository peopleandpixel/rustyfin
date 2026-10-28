use chrono::Utc;
use sqlx::SqlitePool;
use std::path::PathBuf;
use uuid::Uuid;

use rustyfin_model::library::{Library, LibraryContentType};
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
    )
    .await
    .unwrap();

    assert_eq!(total_hash, 1);
    assert_eq!(items_hash[0].name, "2001: A Space Odyssey");
}
