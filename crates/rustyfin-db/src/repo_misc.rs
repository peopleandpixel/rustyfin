use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DlnaStreamingProfileRow {
    pub id: String,
    pub name: String,
    pub profile_xml: String,
    pub container: String,
    pub audio_codec: String,
    pub video_codec: String,
    pub direct_play_profiles: String,
    pub transcoding_profiles: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LocalizationProviderRow {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub enabled: bool,
    pub configuration: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BackupRow {
    pub id: String,
    pub name: String,
    pub backup_path: String,
    pub size_bytes: i64,
    pub created_at: String,
}

// DLNA Streaming Profiles

pub async fn get_streaming_profiles(
    pool: &SqlitePool,
) -> Result<Vec<DlnaStreamingProfileRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, DlnaStreamingProfileRow>(
        "SELECT id, name, profile_xml, container, audio_codec, video_codec, direct_play_profiles, transcoding_profiles
         FROM dlna_streaming_profiles",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_streaming_profile(
    pool: &SqlitePool,
    profile_id: &str,
) -> Result<Option<DlnaStreamingProfileRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, DlnaStreamingProfileRow>(
        "SELECT id, name, profile_xml, container, audio_codec, video_codec, direct_play_profiles, transcoding_profiles
         FROM dlna_streaming_profiles WHERE id = ?1",
    )
    .bind(profile_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn create_streaming_profile(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    profile_xml: &str,
    container: &str,
    audio_codec: &str,
    video_codec: &str,
    direct_play_profiles: &str,
    transcoding_profiles: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO dlna_streaming_profiles (id, name, profile_xml, container, audio_codec, video_codec, direct_play_profiles, transcoding_profiles)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
    )
    .bind(id)
    .bind(name)
    .bind(profile_xml)
    .bind(container)
    .bind(audio_codec)
    .bind(video_codec)
    .bind(direct_play_profiles)
    .bind(transcoding_profiles)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_streaming_profile(
    pool: &SqlitePool,
    profile_id: &str,
    name: &str,
    profile_xml: &str,
    container: &str,
    audio_codec: &str,
    video_codec: &str,
    direct_play_profiles: &str,
    transcoding_profiles: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dlna_streaming_profiles SET name = ?1, profile_xml = ?2, container = ?3,
         audio_codec = ?4, video_codec = ?5, direct_play_profiles = ?6, transcoding_profiles = ?7
         WHERE id = ?8",
    )
    .bind(name)
    .bind(profile_xml)
    .bind(container)
    .bind(audio_codec)
    .bind(video_codec)
    .bind(direct_play_profiles)
    .bind(transcoding_profiles)
    .bind(profile_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_streaming_profile(
    pool: &SqlitePool,
    profile_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM dlna_streaming_profiles WHERE id = ?1")
        .bind(profile_id)
        .execute(pool)
        .await?;
    Ok(())
}

// Localization Providers

pub async fn get_localization_providers(
    pool: &SqlitePool,
) -> Result<Vec<LocalizationProviderRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, LocalizationProviderRow>(
        "SELECT id, name, provider_id, enabled, configuration FROM localization_providers",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn add_localization_provider(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    provider_id: &str,
    configuration: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO localization_providers (id, name, provider_id, enabled, configuration)
         VALUES (?1, ?2, ?3, 1, ?4)",
    )
    .bind(id)
    .bind(name)
    .bind(provider_id)
    .bind(configuration)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_localization_provider(
    pool: &SqlitePool,
    provider_id: &str,
    enabled: bool,
    configuration: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE localization_providers SET enabled = ?1, configuration = ?2 WHERE id = ?3")
        .bind(enabled)
        .bind(configuration)
        .bind(provider_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_localization_provider(
    pool: &SqlitePool,
    provider_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM localization_providers WHERE id = ?1")
        .bind(provider_id)
        .execute(pool)
        .await?;
    Ok(())
}

// Backups

pub async fn get_backups(pool: &SqlitePool) -> Result<Vec<BackupRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, BackupRow>(
        "SELECT id, name, backup_path, size_bytes, created_at FROM backups ORDER BY created_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn create_backup(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    backup_path: &str,
    size_bytes: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO backups (id, name, backup_path, size_bytes) VALUES (?1, ?2, ?3, ?4)")
        .bind(id)
        .bind(name)
        .bind(backup_path)
        .bind(size_bytes)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_backup(pool: &SqlitePool, backup_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM backups WHERE id = ?1")
        .bind(backup_id)
        .execute(pool)
        .await?;
    Ok(())
}
