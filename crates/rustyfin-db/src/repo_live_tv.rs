use sqlx::Row;
use sqlx::SqlitePool;
use uuid::Uuid;

// ── Tuner Hosts ──

#[derive(Debug, Clone)]
pub struct TunerHost {
    pub id: String,
    pub name: String,
    pub r#type: String,
    pub hostname: String,
    pub port: i32,
    pub username: String,
    pub password: String,
    pub tuner_count: i32,
    pub is_ready: bool,
}

pub async fn get_tuner_hosts(pool: &SqlitePool) -> Result<Vec<TunerHost>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            i32,
            String,
            String,
            i32,
            i32,
        ),
    >(
        "SELECT id, name, type, hostname, port, username, password, tuner_count, is_ready
         FROM live_tv_tuner_hosts ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| TunerHost {
            id: r.0,
            name: r.1,
            r#type: r.2,
            hostname: r.3,
            port: r.4,
            username: r.5,
            password: r.6,
            tuner_count: r.7,
            is_ready: r.8 != 0,
        })
        .collect())
}

pub async fn get_tuner_host(pool: &SqlitePool, id: &str) -> Result<Option<TunerHost>, sqlx::Error> {
    let row: Option<(
        String,
        String,
        String,
        String,
        i32,
        String,
        String,
        i32,
        i32,
    )> = sqlx::query_as(
        "SELECT id, name, type, hostname, port, username, password, tuner_count, is_ready
             FROM live_tv_tuner_hosts WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| TunerHost {
        id: r.0,
        name: r.1,
        r#type: r.2,
        hostname: r.3,
        port: r.4,
        username: r.5,
        password: r.6,
        tuner_count: r.7,
        is_ready: r.8 != 0,
    }))
}

pub async fn create_tuner_host(
    pool: &SqlitePool,
    name: &str,
    r#type: &str,
    hostname: &str,
    port: i32,
    username: &str,
    password: &str,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO live_tv_tuner_hosts (id, name, type, hostname, port, username, password)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
    )
    .bind(&id)
    .bind(name)
    .bind(r#type)
    .bind(hostname)
    .bind(port)
    .bind(username)
    .bind(password)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn delete_tuner_host(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM live_tv_tuner_hosts WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_all_tuner_hosts(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM live_tv_tuner_hosts")
        .execute(pool)
        .await?;
    Ok(())
}

// ── Tuners ──

#[derive(Debug, Clone)]
pub struct Tuner {
    pub id: String,
    pub host_id: String,
    pub name: String,
    pub channel_url: String,
    pub channel_id: String,
    pub is_ready: bool,
    pub is_in_use: bool,
}

pub async fn get_tuners(pool: &SqlitePool) -> Result<Vec<Tuner>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String, String, String, i32, i32)>(
        "SELECT id, host_id, name, channel_url, channel_id, is_ready, is_in_use
         FROM live_tv_tuners ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Tuner {
            id: r.0,
            host_id: r.1,
            name: r.2,
            channel_url: r.3,
            channel_id: r.4,
            is_ready: r.5 != 0,
            is_in_use: r.6 != 0,
        })
        .collect())
}

pub async fn get_tuner(pool: &SqlitePool, id: &str) -> Result<Option<Tuner>, sqlx::Error> {
    let row: Option<(String, String, String, String, String, i32, i32)> = sqlx::query_as(
        "SELECT id, host_id, name, channel_url, channel_id, is_ready, is_in_use
         FROM live_tv_tuners WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Tuner {
        id: r.0,
        host_id: r.1,
        name: r.2,
        channel_url: r.3,
        channel_id: r.4,
        is_ready: r.5 != 0,
        is_in_use: r.6 != 0,
    }))
}

// ── Listing Providers ──

#[derive(Debug, Clone)]
pub struct ListingProvider {
    pub id: String,
    pub name: String,
    pub r#type: String,
    pub path: String,
    pub is_ready: bool,
}

pub async fn get_listing_providers(pool: &SqlitePool) -> Result<Vec<ListingProvider>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, String, String, i32)>(
        "SELECT id, name, type, path, is_ready FROM live_tv_listing_providers ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| ListingProvider {
            id: r.0,
            name: r.1,
            r#type: r.2,
            path: r.3,
            is_ready: r.4 != 0,
        })
        .collect())
}

pub async fn get_listing_provider(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<ListingProvider>, sqlx::Error> {
    let row: Option<(String, String, String, String, i32)> = sqlx::query_as(
        "SELECT id, name, type, path, is_ready FROM live_tv_listing_providers WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| ListingProvider {
        id: r.0,
        name: r.1,
        r#type: r.2,
        path: r.3,
        is_ready: r.4 != 0,
    }))
}

pub async fn create_listing_provider(
    pool: &SqlitePool,
    name: &str,
    r#type: &str,
    path: &str,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO live_tv_listing_providers (id, name, type, path) VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&id)
    .bind(name)
    .bind(r#type)
    .bind(path)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn delete_listing_provider(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM live_tv_listing_providers WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_all_listing_providers(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM live_tv_listing_providers")
        .execute(pool)
        .await?;
    Ok(())
}

// ── Channels ──

#[derive(Debug, Clone)]
pub struct LiveTvChannel {
    pub id: String,
    pub name: String,
    pub channel_number: String,
    pub tuner_host_id: Option<String>,
    pub tuner_id: Option<String>,
    pub epg_id: String,
    pub display_name: String,
    pub logo_url: String,
    pub is_radio: bool,
}

pub async fn get_channels(pool: &SqlitePool) -> Result<Vec<LiveTvChannel>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (String, String, String, Option<String>, Option<String>, String, String, String, i32),
    >(
        "SELECT id, name, channel_number, tuner_host_id, tuner_id, epg_id, display_name, logo_url, is_radio
         FROM live_tv_channels ORDER BY channel_number",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| LiveTvChannel {
            id: r.0,
            name: r.1,
            channel_number: r.2,
            tuner_host_id: r.3,
            tuner_id: r.4,
            epg_id: r.5,
            display_name: r.6,
            logo_url: r.7,
            is_radio: r.8 != 0,
        })
        .collect())
}

pub async fn get_channel(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<LiveTvChannel>, sqlx::Error> {
    let row: Option<(String, String, String, Option<String>, Option<String>, String, String, String, i32)> =
        sqlx::query_as(
            "SELECT id, name, channel_number, tuner_host_id, tuner_id, epg_id, display_name, logo_url, is_radio
             FROM live_tv_channels WHERE id = ?1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| LiveTvChannel {
        id: r.0,
        name: r.1,
        channel_number: r.2,
        tuner_host_id: r.3,
        tuner_id: r.4,
        epg_id: r.5,
        display_name: r.6,
        logo_url: r.7,
        is_radio: r.8 != 0,
    }))
}

// ── Guide Entries ──

#[derive(Debug, Clone)]
pub struct GuideEntry {
    pub id: String,
    pub channel_id: String,
    pub name: String,
    pub description: String,
    pub start_time: String,
    pub end_time: String,
    pub series_id: String,
    pub season_display: String,
    pub episode_display: String,
    pub official_rating: String,
    pub community_rating: Option<f64>,
    pub image_url: String,
    pub genre: String,
    pub is_series: bool,
    pub is_movie: bool,
    pub is_news: bool,
    pub is_live: bool,
}

fn row_to_guide_entry(row: &sqlx::sqlite::SqliteRow) -> GuideEntry {
    GuideEntry {
        id: row.get("id"),
        channel_id: row.get("channel_id"),
        name: row.get("name"),
        description: row.get("description"),
        start_time: row.get("start_time"),
        end_time: row.get("end_time"),
        series_id: row.get("series_id"),
        season_display: row.get("season_display"),
        episode_display: row.get("episode_display"),
        official_rating: row.get("official_rating"),
        community_rating: row.get("community_rating"),
        image_url: row.get("image_url"),
        genre: row.get("genre"),
        is_series: row.get::<i32, _>("is_series") != 0,
        is_movie: row.get::<i32, _>("is_movie") != 0,
        is_news: row.get::<i32, _>("is_news") != 0,
        is_live: row.get::<i32, _>("is_live") != 0,
    }
}

pub async fn get_guide_entries(
    pool: &SqlitePool,
    _channel_id: Option<&str>,
    _start_time: Option<&str>,
    _end_time: Option<&str>,
) -> Result<Vec<GuideEntry>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT id, channel_id, name, description, start_time, end_time, series_id,
                season_display, episode_display, official_rating, community_rating,
                image_url, genre, is_series, is_movie, is_news, is_live
         FROM live_tv_guide_entries
         ORDER BY start_time
         LIMIT 500",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(|r| row_to_guide_entry(&r)).collect())
}

pub async fn get_guide_entry(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<GuideEntry>, sqlx::Error> {
    let row = sqlx::query(
        "SELECT id, channel_id, name, description, start_time, end_time, series_id,
                season_display, episode_display, official_rating, community_rating,
                image_url, genre, is_series, is_movie, is_news, is_live
         FROM live_tv_guide_entries WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| row_to_guide_entry(&r)))
}

// ── Timers ──

#[derive(Debug, Clone)]
pub struct Timer {
    pub id: String,
    pub channel_id: String,
    pub program_id: String,
    pub name: String,
    pub description: String,
    pub start_time: String,
    pub end_time: String,
    pub recording_path: String,
    pub status: String,
    pub recording_status: String,
    pub priority: String,
    pub keep_option: String,
    pub max_record_days: i32,
    pub enable_email: bool,
}

pub async fn get_timers(pool: &SqlitePool) -> Result<Vec<Timer>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            i32,
            i32,
        ),
    >(
        "SELECT id, channel_id, program_id, name, description, start_time, end_time,
                recording_path, status, recording_status, priority, keep_option, max_record_days, enable_email
         FROM live_tv_timers ORDER BY start_time",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Timer {
            id: r.0,
            channel_id: r.1,
            program_id: r.2,
            name: r.3,
            description: r.4,
            start_time: r.5,
            end_time: r.6,
            recording_path: r.7,
            status: r.8,
            recording_status: r.9,
            priority: r.10,
            keep_option: r.11,
            max_record_days: r.12,
            enable_email: r.13 != 0,
        })
        .collect())
}

pub async fn get_timer(pool: &SqlitePool, id: &str) -> Result<Option<Timer>, sqlx::Error> {
    let row: Option<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i32,
        i32,
    )> = sqlx::query_as(
        "SELECT id, channel_id, program_id, name, description, start_time, end_time,
                recording_path, status, recording_status, priority, keep_option, max_record_days, enable_email
         FROM live_tv_timers WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| Timer {
        id: r.0,
        channel_id: r.1,
        program_id: r.2,
        name: r.3,
        description: r.4,
        start_time: r.5,
        end_time: r.6,
        recording_path: r.7,
        status: r.8,
        recording_status: r.9,
        priority: r.10,
        keep_option: r.11,
        max_record_days: r.12,
        enable_email: r.13 != 0,
    }))
}

pub async fn create_timer(
    pool: &SqlitePool,
    channel_id: &str,
    program_id: &str,
    name: &str,
    description: &str,
    start_time: &str,
    end_time: &str,
    recording_path: &str,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO live_tv_timers (id, channel_id, program_id, name, description, start_time, end_time, recording_path)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(&id)
    .bind(channel_id)
    .bind(program_id)
    .bind(name)
    .bind(description)
    .bind(start_time)
    .bind(end_time)
    .bind(recording_path)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn update_timer_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE live_tv_timers SET status = ?1 WHERE id = ?2")
        .bind(status)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_timer(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM live_tv_timers WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Series Timers ──

#[derive(Debug, Clone)]
pub struct SeriesTimer {
    pub id: String,
    pub name: String,
    pub series_id: String,
    pub query: String,
    pub search_until_time: String,
    pub record_by_date: String,
    pub ignore_words: String,
    pub require_words: String,
    pub recording_path: String,
    pub priority: String,
    pub keep_option: String,
    pub max_record_days: i32,
    pub enable_email: bool,
    pub command: String,
}

pub async fn get_series_timers(pool: &SqlitePool) -> Result<Vec<SeriesTimer>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            String,
            i32,
            i32,
            String,
        ),
    >(
        "SELECT id, name, series_id, query, search_until_time, record_by_date,
                ignore_words, require_words, recording_path, priority, keep_option,
                max_record_days, enable_email, command
         FROM live_tv_series_timers ORDER BY name",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| SeriesTimer {
            id: r.0,
            name: r.1,
            series_id: r.2,
            query: r.3,
            search_until_time: r.4,
            record_by_date: r.5,
            ignore_words: r.6,
            require_words: r.7,
            recording_path: r.8,
            priority: r.9,
            keep_option: r.10,
            max_record_days: r.11,
            enable_email: r.12 != 0,
            command: r.13,
        })
        .collect())
}

pub async fn get_series_timer(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<SeriesTimer>, sqlx::Error> {
    let row: Option<(
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        i32,
        i32,
        String,
    )> = sqlx::query_as(
        "SELECT id, name, series_id, query, search_until_time, record_by_date,
                ignore_words, require_words, recording_path, priority, keep_option,
                max_record_days, enable_email, command
         FROM live_tv_series_timers WHERE id = ?1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| SeriesTimer {
        id: r.0,
        name: r.1,
        series_id: r.2,
        query: r.3,
        search_until_time: r.4,
        record_by_date: r.5,
        ignore_words: r.6,
        require_words: r.7,
        recording_path: r.8,
        priority: r.9,
        keep_option: r.10,
        max_record_days: r.11,
        enable_email: r.12 != 0,
        command: r.13,
    }))
}

pub async fn create_series_timer(
    pool: &SqlitePool,
    name: &str,
    query: &str,
    recording_path: &str,
) -> Result<String, sqlx::Error> {
    let id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO live_tv_series_timers (id, name, query, recording_path)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(&id)
    .bind(name)
    .bind(query)
    .bind(recording_path)
    .execute(pool)
    .await?;
    Ok(id)
}

pub async fn update_series_timer(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    query: &str,
    recording_path: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE live_tv_series_timers SET name = ?1, query = ?2, recording_path = ?3 WHERE id = ?4",
    )
    .bind(name)
    .bind(query)
    .bind(recording_path)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_series_timer(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM live_tv_series_timers WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Recordings ──

#[derive(Debug, Clone)]
pub struct Recording {
    pub id: String,
    pub media_item_id: String,
    pub timer_id: Option<String>,
    pub channel_id: String,
    pub name: String,
    pub recording_path: String,
}

pub async fn get_recordings(pool: &SqlitePool) -> Result<Vec<Recording>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, String, Option<String>, String, String, String)>(
        "SELECT id, media_item_id, timer_id, channel_id, name, recording_path
         FROM live_tv_recordings ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| Recording {
            id: r.0,
            media_item_id: r.1,
            timer_id: r.2,
            channel_id: r.3,
            name: r.4,
            recording_path: r.5,
        })
        .collect())
}

pub async fn delete_recording(pool: &SqlitePool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM live_tv_recordings WHERE id = ?1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── Guide Info ──

pub async fn has_tv_provider(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM live_tv_channels WHERE is_radio = 0")
        .fetch_one(pool)
        .await?;
    Ok(count > 0)
}

pub async fn has_radio_provider(pool: &SqlitePool) -> Result<bool, sqlx::Error> {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM live_tv_channels WHERE is_radio = 1")
        .fetch_one(pool)
        .await?;
    Ok(count > 0)
}
