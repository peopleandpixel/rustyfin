use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct QueryResult<T> {
    items: Vec<T>,
    #[serde(rename = "TotalRecordCount")]
    total_record_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "StartIndex")]
    start_index: Option<i64>,
}

// ── Channel DTOs ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ChannelDto {
    id: String,
    name: String,
    #[serde(rename = "ChannelNumber")]
    channel_number: String,
    #[serde(rename = "DisplayName")]
    display_name: String,
    #[serde(rename = "LogoUrl", skip_serializing_if = "Option::is_none")]
    logo_url: Option<String>,
    #[serde(rename = "IsRadio", default)]
    is_radio: bool,
}

// ── Tuner Host DTOs ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct TunerHostDto {
    id: String,
    name: String,
    #[serde(rename = "Type")]
    r#type: String,
    #[serde(rename = "IsReady", default)]
    is_ready: bool,
    #[serde(rename = "TunerCount", default)]
    tuner_count: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateTunerHostRequest {
    #[serde(default)]
    name: String,
    #[serde(rename = "Type", default)]
    r#type: String,
    #[serde(default)]
    hostname: String,
    #[serde(default)]
    port: i32,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
}

// ── Tuner DTOs ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct TunerDto {
    id: String,
    #[serde(rename = "HostId")]
    host_id: String,
    name: String,
    #[serde(rename = "IsReady", default)]
    is_ready: bool,
    #[serde(rename = "IsInUse", default)]
    is_in_use: bool,
}

// ── Listing Provider DTOs ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ListingProviderDto {
    id: String,
    name: String,
    #[serde(rename = "Type")]
    r#type: String,
    #[serde(rename = "IsReady", default)]
    is_ready: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateListingProviderRequest {
    #[serde(default)]
    name: String,
    #[serde(rename = "Type", default)]
    r#type: String,
    #[serde(default)]
    path: String,
}

// ── Guide Entry DTOs ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct GuideEntryDto {
    id: String,
    #[serde(rename = "ChannelId")]
    channel_id: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "StartTime")]
    start_time: String,
    #[serde(rename = "EndTime")]
    end_time: String,
    #[serde(rename = "SeriesId", skip_serializing_if = "Option::is_none")]
    series_id: Option<String>,
    #[serde(rename = "SeasonDisplay", skip_serializing_if = "Option::is_none")]
    season_display: Option<String>,
    #[serde(rename = "EpisodeDisplay", skip_serializing_if = "Option::is_none")]
    episode_display: Option<String>,
    #[serde(rename = "OfficialRating", skip_serializing_if = "Option::is_none")]
    official_rating: Option<String>,
    #[serde(rename = "CommunityRating", skip_serializing_if = "Option::is_none")]
    community_rating: Option<f64>,
    #[serde(rename = "ImageUrl", skip_serializing_if = "Option::is_none")]
    image_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    genre: Option<String>,
    #[serde(rename = "IsSeries", default)]
    is_series: bool,
    #[serde(rename = "IsMovie", default)]
    is_movie: bool,
    #[serde(rename = "IsNews", default)]
    is_news: bool,
    #[serde(rename = "IsLive", default)]
    is_live: bool,
}

// ── Timer DTOs ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct TimerDto {
    id: String,
    #[serde(rename = "ChannelId")]
    channel_id: String,
    #[serde(rename = "ProgramId", skip_serializing_if = "Option::is_none")]
    program_id: Option<String>,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "StartTime")]
    start_time: String,
    #[serde(rename = "EndTime")]
    end_time: String,
    #[serde(rename = "RecordingPath", skip_serializing_if = "Option::is_none")]
    recording_path: Option<String>,
    status: String,
    #[serde(rename = "RecordingStatus")]
    recording_status: String,
    priority: String,
    #[serde(rename = "KeepOption")]
    keep_option: String,
    #[serde(rename = "MaxRecordDays", default)]
    max_record_days: i32,
    #[serde(rename = "EnableEmail", default)]
    enable_email: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateTimerRequest {
    #[serde(rename = "ChannelId", default)]
    channel_id: String,
    #[serde(rename = "ProgramId", default)]
    program_id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    description: String,
    #[serde(rename = "StartTime", default)]
    start_time: String,
    #[serde(rename = "EndTime", default)]
    end_time: String,
    #[serde(rename = "RecordingPath", default)]
    recording_path: String,
}

// ── Series Timer DTOs ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct SeriesTimerDto {
    id: String,
    name: String,
    #[serde(rename = "SeriesId", skip_serializing_if = "Option::is_none")]
    series_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    query: Option<String>,
    #[serde(rename = "SearchUntilTime", skip_serializing_if = "Option::is_none")]
    search_until_time: Option<String>,
    #[serde(rename = "RecordByDate", skip_serializing_if = "Option::is_none")]
    record_by_date: Option<String>,
    #[serde(rename = "IgnoreWords", skip_serializing_if = "Option::is_none")]
    ignore_words: Option<String>,
    #[serde(rename = "RequireWords", skip_serializing_if = "Option::is_none")]
    require_words: Option<String>,
    #[serde(rename = "RecordingPath", skip_serializing_if = "Option::is_none")]
    recording_path: Option<String>,
    priority: String,
    #[serde(rename = "KeepOption")]
    keep_option: String,
    #[serde(rename = "MaxRecordDays", default)]
    max_record_days: i32,
    #[serde(rename = "EnableEmail", default)]
    enable_email: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateSeriesTimerRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    query: String,
    #[serde(rename = "RecordingPath", default)]
    recording_path: String,
}

// ── Recording DTOs ──

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct RecordingDto {
    id: String,
    #[serde(rename = "MediaItemId")]
    media_item_id: String,
    #[serde(rename = "ChannelId")]
    channel_id: String,
    name: String,
    #[serde(rename = "RecordingPath", skip_serializing_if = "Option::is_none")]
    recording_path: Option<String>,
}

// ── Query params ──

#[derive(Debug, Deserialize)]
struct GuideQuery {
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default, rename = "StartDate")]
    start_date: Option<String>,
    #[serde(default, rename = "EndDate")]
    end_date: Option<String>,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/LiveTv/Channels", get(get_channels))
        .route("/LiveTv/Channels/{channelId}", get(get_channel))
        .route("/LiveTv/Recordings", get(get_recordings))
        .route("/LiveTv/Recordings/{recordingId}", delete(delete_recording))
        .route("/LiveTv/Guides", get(get_guides))
        .route("/LiveTv/GuideInfo", get(get_guide_info))
        .route(
            "/LiveTv/SeriesTimers",
            get(get_series_timers).post(create_series_timer),
        )
        .route(
            "/LiveTv/SeriesTimers/{timerId}",
            get(get_series_timer)
                .put(update_series_timer)
                .delete(delete_series_timer),
        )
        .route("/LiveTv/Timers", get(get_timers).post(create_timer))
        .route(
            "/LiveTv/Timers/{timerId}",
            get(get_timer).put(update_timer).delete(delete_timer),
        )
        .route("/LiveTv/RecommendedPrograms", get(get_recommended_programs))
        .route("/LiveTv/Programs", get(get_programs))
        .route("/LiveTv/Programs/{programId}", get(get_program))
        .route("/LiveTv/LiveStream", post(start_live_stream))
        .route("/LiveTv/VirtualChannels", get(get_virtual_channels))
        .route(
            "/LiveTv/TunerHosts",
            get(get_tuner_hosts)
                .post(create_tuner_host)
                .delete(delete_tuner_hosts),
        )
        .route(
            "/LiveTv/TunerHosts/{tunerHostId}",
            get(get_tuner_host).delete(delete_tuner_host),
        )
        .route("/LiveTv/Tuners", get(get_tuners))
        .route("/LiveTv/Tuners/{tunerId}", get(get_tuner))
        .route(
            "/LiveTv/ListingProviders",
            get(get_listing_providers)
                .post(create_listing_provider)
                .delete(delete_listing_providers),
        )
        .route(
            "/LiveTv/ListingProviders/{providerId}",
            get(get_listing_provider).delete(delete_listing_provider),
        )
        .route(
            "/LiveTv/ChannelMappingOptions",
            get(get_channel_mapping_options),
        )
        .route(
            "/LiveTv/RecommendedProgramsPtr",
            get(get_recommended_programs),
        )
        .route("/LiveTv/RecordingsFolders", get(get_recordings_folders))
        .route(
            "/LiveTv/NextLiveMediaSource",
            get(get_next_live_media_source),
        )
        .route(
            "/LiveTv/Programs/{programId}/MediaSources",
            get(get_program_media_sources),
        )
        .route(
            "/LiveTv/Programs/{programId}/MediaSources/{mediaSourceId}/stream",
            get(get_program_stream),
        )
}

// ── Channels ──

async fn get_channels(
    State(state): State<AppState>,
) -> Result<Json<QueryResult<ChannelDto>>, ApiError> {
    let channels = rustyfin_db::repo_live_tv::get_channels(&state.db_ro).await?;
    let items: Vec<ChannelDto> = channels
        .into_iter()
        .map(|ch| ChannelDto {
            id: ch.id,
            name: ch.name,
            channel_number: ch.channel_number,
            display_name: ch.display_name,
            logo_url: if ch.logo_url.is_empty() {
                None
            } else {
                Some(ch.logo_url)
            },
            is_radio: ch.is_radio,
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn get_channel(
    State(state): State<AppState>,
    Path(channel_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let ch = rustyfin_db::repo_live_tv::get_channel(&state.db_ro, &channel_id.to_string())
        .await?
        .ok_or_else(|| ApiError::NotFound("channel not found".into()))?;

    Ok(Json(serde_json::json!({
        "Id": ch.id,
        "Name": ch.name,
        "ChannelNumber": ch.channel_number,
        "DisplayName": ch.display_name,
        "LogoUrl": ch.logo_url,
        "IsRadio": ch.is_radio,
    })))
}

// ── Recordings ──

async fn get_recordings(
    State(state): State<AppState>,
) -> Result<Json<QueryResult<RecordingDto>>, ApiError> {
    let recordings = rustyfin_db::repo_live_tv::get_recordings(&state.db_ro).await?;
    let items: Vec<RecordingDto> = recordings
        .into_iter()
        .map(|r| RecordingDto {
            id: r.id,
            media_item_id: r.media_item_id,
            channel_id: r.channel_id,
            name: r.name,
            recording_path: if r.recording_path.is_empty() {
                None
            } else {
                Some(r.recording_path)
            },
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn delete_recording(
    State(state): State<AppState>,
    Path(recording_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_live_tv::delete_recording(&state.db, &recording_id.to_string()).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Guides ──

async fn get_guides(
    State(state): State<AppState>,
    Query(query): Query<GuideQuery>,
) -> Result<Json<QueryResult<GuideEntryDto>>, ApiError> {
    let entries = rustyfin_db::repo_live_tv::get_guide_entries(
        &state.db_ro,
        query.channel_id.as_deref(),
        query.start_date.as_deref(),
        query.end_date.as_deref(),
    )
    .await?;
    let items: Vec<GuideEntryDto> = entries
        .into_iter()
        .map(|e| GuideEntryDto {
            id: e.id,
            channel_id: e.channel_id,
            name: e.name,
            description: if e.description.is_empty() {
                None
            } else {
                Some(e.description)
            },
            start_time: e.start_time,
            end_time: e.end_time,
            series_id: if e.series_id.is_empty() {
                None
            } else {
                Some(e.series_id)
            },
            season_display: if e.season_display.is_empty() {
                None
            } else {
                Some(e.season_display)
            },
            episode_display: if e.episode_display.is_empty() {
                None
            } else {
                Some(e.episode_display)
            },
            official_rating: if e.official_rating.is_empty() {
                None
            } else {
                Some(e.official_rating)
            },
            community_rating: e.community_rating,
            image_url: if e.image_url.is_empty() {
                None
            } else {
                Some(e.image_url)
            },
            genre: if e.genre.is_empty() {
                None
            } else {
                Some(e.genre)
            },
            is_series: e.is_series,
            is_movie: e.is_movie,
            is_news: e.is_news,
            is_live: e.is_live,
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn get_guide_info(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let has_tv = rustyfin_db::repo_live_tv::has_tv_provider(&state.db_ro)
        .await
        .unwrap_or(false);
    let has_radio = rustyfin_db::repo_live_tv::has_radio_provider(&state.db_ro)
        .await
        .unwrap_or(false);
    Ok(Json(serde_json::json!({
        "HasTvProvider": has_tv,
        "HasRadioProvider": has_radio,
    })))
}

// ── Series Timers ──

async fn get_series_timers(
    State(state): State<AppState>,
) -> Result<Json<QueryResult<SeriesTimerDto>>, ApiError> {
    let timers = rustyfin_db::repo_live_tv::get_series_timers(&state.db_ro).await?;
    let items: Vec<SeriesTimerDto> = timers
        .into_iter()
        .map(|t| SeriesTimerDto {
            id: t.id,
            name: t.name,
            series_id: if t.series_id.is_empty() {
                None
            } else {
                Some(t.series_id)
            },
            query: if t.query.is_empty() {
                None
            } else {
                Some(t.query)
            },
            search_until_time: if t.search_until_time.is_empty() {
                None
            } else {
                Some(t.search_until_time)
            },
            record_by_date: if t.record_by_date.is_empty() {
                None
            } else {
                Some(t.record_by_date)
            },
            ignore_words: if t.ignore_words.is_empty() {
                None
            } else {
                Some(t.ignore_words)
            },
            require_words: if t.require_words.is_empty() {
                None
            } else {
                Some(t.require_words)
            },
            recording_path: if t.recording_path.is_empty() {
                None
            } else {
                Some(t.recording_path)
            },
            priority: t.priority,
            keep_option: t.keep_option,
            max_record_days: t.max_record_days,
            enable_email: t.enable_email,
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn get_series_timer(
    State(state): State<AppState>,
    Path(timer_id): Path<Uuid>,
) -> Result<Json<SeriesTimerDto>, ApiError> {
    let t = rustyfin_db::repo_live_tv::get_series_timer(&state.db_ro, &timer_id.to_string())
        .await?
        .ok_or_else(|| ApiError::NotFound("series timer not found".into()))?;
    Ok(Json(SeriesTimerDto {
        id: t.id,
        name: t.name,
        series_id: if t.series_id.is_empty() {
            None
        } else {
            Some(t.series_id)
        },
        query: if t.query.is_empty() {
            None
        } else {
            Some(t.query)
        },
        search_until_time: if t.search_until_time.is_empty() {
            None
        } else {
            Some(t.search_until_time)
        },
        record_by_date: if t.record_by_date.is_empty() {
            None
        } else {
            Some(t.record_by_date)
        },
        ignore_words: if t.ignore_words.is_empty() {
            None
        } else {
            Some(t.ignore_words)
        },
        require_words: if t.require_words.is_empty() {
            None
        } else {
            Some(t.require_words)
        },
        recording_path: if t.recording_path.is_empty() {
            None
        } else {
            Some(t.recording_path)
        },
        priority: t.priority,
        keep_option: t.keep_option,
        max_record_days: t.max_record_days,
        enable_email: t.enable_email,
    }))
}

async fn create_series_timer(
    State(state): State<AppState>,
    Json(req): Json<CreateSeriesTimerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = rustyfin_db::repo_live_tv::create_series_timer(
        &state.db,
        &req.name,
        &req.query,
        &req.recording_path,
    )
    .await?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "Id": id }))))
}

async fn update_series_timer(
    State(state): State<AppState>,
    Path(timer_id): Path<Uuid>,
    Json(req): Json<CreateSeriesTimerRequest>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_live_tv::update_series_timer(
        &state.db,
        &timer_id.to_string(),
        &req.name,
        &req.query,
        &req.recording_path,
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_series_timer(
    State(state): State<AppState>,
    Path(timer_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_live_tv::delete_series_timer(&state.db, &timer_id.to_string()).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Timers ──

async fn get_timers(
    State(state): State<AppState>,
) -> Result<Json<QueryResult<TimerDto>>, ApiError> {
    let timers = rustyfin_db::repo_live_tv::get_timers(&state.db_ro).await?;
    let items: Vec<TimerDto> = timers
        .into_iter()
        .map(|t| TimerDto {
            id: t.id,
            channel_id: t.channel_id,
            program_id: if t.program_id.is_empty() {
                None
            } else {
                Some(t.program_id)
            },
            name: t.name,
            description: if t.description.is_empty() {
                None
            } else {
                Some(t.description)
            },
            start_time: t.start_time,
            end_time: t.end_time,
            recording_path: if t.recording_path.is_empty() {
                None
            } else {
                Some(t.recording_path)
            },
            status: t.status,
            recording_status: t.recording_status,
            priority: t.priority,
            keep_option: t.keep_option,
            max_record_days: t.max_record_days,
            enable_email: t.enable_email,
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn get_timer(
    State(state): State<AppState>,
    Path(timer_id): Path<Uuid>,
) -> Result<Json<TimerDto>, ApiError> {
    let t = rustyfin_db::repo_live_tv::get_timer(&state.db_ro, &timer_id.to_string())
        .await?
        .ok_or_else(|| ApiError::NotFound("timer not found".into()))?;
    Ok(Json(TimerDto {
        id: t.id,
        channel_id: t.channel_id,
        program_id: if t.program_id.is_empty() {
            None
        } else {
            Some(t.program_id)
        },
        name: t.name,
        description: if t.description.is_empty() {
            None
        } else {
            Some(t.description)
        },
        start_time: t.start_time,
        end_time: t.end_time,
        recording_path: if t.recording_path.is_empty() {
            None
        } else {
            Some(t.recording_path)
        },
        status: t.status,
        recording_status: t.recording_status,
        priority: t.priority,
        keep_option: t.keep_option,
        max_record_days: t.max_record_days,
        enable_email: t.enable_email,
    }))
}

async fn create_timer(
    State(state): State<AppState>,
    Json(req): Json<CreateTimerRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = rustyfin_db::repo_live_tv::create_timer(
        &state.db,
        &req.channel_id,
        &req.program_id,
        &req.name,
        &req.description,
        &req.start_time,
        &req.end_time,
        &req.recording_path,
    )
    .await?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "Id": id }))))
}

async fn update_timer(
    State(state): State<AppState>,
    Path(timer_id): Path<Uuid>,
    Json(req): Json<serde_json::Value>,
) -> Result<StatusCode, ApiError> {
    // Extract status from the request body if present
    if let Some(status) = req.get("Status").and_then(|s| s.as_str()) {
        rustyfin_db::repo_live_tv::update_timer_status(&state.db, &timer_id.to_string(), status)
            .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_timer(
    State(state): State<AppState>,
    Path(timer_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_live_tv::delete_timer(&state.db, &timer_id.to_string()).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Programs (alias for Guide Entries) ──

async fn get_recommended_programs(
    State(state): State<AppState>,
) -> Result<Json<QueryResult<GuideEntryDto>>, ApiError> {
    // Recommended programs are those with high community ratings
    let entries =
        rustyfin_db::repo_live_tv::get_guide_entries(&state.db_ro, None, None, None).await?;
    let items: Vec<GuideEntryDto> = entries
        .into_iter()
        .filter(|e| e.community_rating.is_some_and(|r| r >= 3.0))
        .map(|e| GuideEntryDto {
            id: e.id,
            channel_id: e.channel_id,
            name: e.name,
            description: if e.description.is_empty() {
                None
            } else {
                Some(e.description)
            },
            start_time: e.start_time,
            end_time: e.end_time,
            series_id: if e.series_id.is_empty() {
                None
            } else {
                Some(e.series_id)
            },
            season_display: if e.season_display.is_empty() {
                None
            } else {
                Some(e.season_display)
            },
            episode_display: if e.episode_display.is_empty() {
                None
            } else {
                Some(e.episode_display)
            },
            official_rating: if e.official_rating.is_empty() {
                None
            } else {
                Some(e.official_rating)
            },
            community_rating: e.community_rating,
            image_url: if e.image_url.is_empty() {
                None
            } else {
                Some(e.image_url)
            },
            genre: if e.genre.is_empty() {
                None
            } else {
                Some(e.genre)
            },
            is_series: e.is_series,
            is_movie: e.is_movie,
            is_news: e.is_news,
            is_live: e.is_live,
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn get_programs(
    State(state): State<AppState>,
    Query(query): Query<GuideQuery>,
) -> Result<Json<QueryResult<GuideEntryDto>>, ApiError> {
    get_guides(State(state), Query(query)).await
}

async fn get_program(
    State(state): State<AppState>,
    Path(program_id): Path<Uuid>,
) -> Result<Json<GuideEntryDto>, ApiError> {
    let e = rustyfin_db::repo_live_tv::get_guide_entry(&state.db_ro, &program_id.to_string())
        .await?
        .ok_or_else(|| ApiError::NotFound("program not found".into()))?;
    Ok(Json(GuideEntryDto {
        id: e.id,
        channel_id: e.channel_id,
        name: e.name,
        description: if e.description.is_empty() {
            None
        } else {
            Some(e.description)
        },
        start_time: e.start_time,
        end_time: e.end_time,
        series_id: if e.series_id.is_empty() {
            None
        } else {
            Some(e.series_id)
        },
        season_display: if e.season_display.is_empty() {
            None
        } else {
            Some(e.season_display)
        },
        episode_display: if e.episode_display.is_empty() {
            None
        } else {
            Some(e.episode_display)
        },
        official_rating: if e.official_rating.is_empty() {
            None
        } else {
            Some(e.official_rating)
        },
        community_rating: e.community_rating,
        image_url: if e.image_url.is_empty() {
            None
        } else {
            Some(e.image_url)
        },
        genre: if e.genre.is_empty() {
            None
        } else {
            Some(e.genre)
        },
        is_series: e.is_series,
        is_movie: e.is_movie,
        is_news: e.is_news,
        is_live: e.is_live,
    }))
}

// ── Live Stream ──

async fn start_live_stream(
    State(_state): State<AppState>,
    Json(_body): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Live streaming requires a real-time transcoding pipeline.
    // For now, return an empty response – the client will fall back to direct play.
    Ok(Json(serde_json::json!({})))
}

// ── Virtual Channels ──

async fn get_virtual_channels(
    State(_state): State<AppState>,
) -> Result<Json<QueryResult<serde_json::Value>>, ApiError> {
    // Virtual channels are not yet implemented
    Ok(Json(QueryResult {
        items: vec![],
        total_record_count: 0,
        start_index: Some(0),
    }))
}

// ── Tuner Hosts ──

async fn get_tuner_hosts(
    State(state): State<AppState>,
) -> Result<Json<QueryResult<TunerHostDto>>, ApiError> {
    let hosts = rustyfin_db::repo_live_tv::get_tuner_hosts(&state.db_ro).await?;
    let items: Vec<TunerHostDto> = hosts
        .into_iter()
        .map(|h| TunerHostDto {
            id: h.id,
            name: h.name,
            r#type: h.r#type,
            is_ready: h.is_ready,
            tuner_count: h.tuner_count,
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn create_tuner_host(
    State(state): State<AppState>,
    Json(req): Json<CreateTunerHostRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = rustyfin_db::repo_live_tv::create_tuner_host(
        &state.db,
        &req.name,
        &req.r#type,
        &req.hostname,
        req.port,
        &req.username,
        &req.password,
    )
    .await?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "Id": id }))))
}

async fn delete_tuner_hosts(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_live_tv::delete_all_tuner_hosts(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_tuner_host(
    State(state): State<AppState>,
    Path(tuner_host_id): Path<Uuid>,
) -> Result<Json<TunerHostDto>, ApiError> {
    let h = rustyfin_db::repo_live_tv::get_tuner_host(&state.db_ro, &tuner_host_id.to_string())
        .await?
        .ok_or_else(|| ApiError::NotFound("tuner host not found".into()))?;
    Ok(Json(TunerHostDto {
        id: h.id,
        name: h.name,
        r#type: h.r#type,
        is_ready: h.is_ready,
        tuner_count: h.tuner_count,
    }))
}

async fn delete_tuner_host(
    State(state): State<AppState>,
    Path(tuner_host_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_live_tv::delete_tuner_host(&state.db, &tuner_host_id.to_string()).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Tuners ──

async fn get_tuners(
    State(state): State<AppState>,
) -> Result<Json<QueryResult<TunerDto>>, ApiError> {
    let tuners = rustyfin_db::repo_live_tv::get_tuners(&state.db_ro).await?;
    let items: Vec<TunerDto> = tuners
        .into_iter()
        .map(|t| TunerDto {
            id: t.id,
            host_id: t.host_id,
            name: t.name,
            is_ready: t.is_ready,
            is_in_use: t.is_in_use,
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn get_tuner(
    State(state): State<AppState>,
    Path(tuner_id): Path<Uuid>,
) -> Result<Json<TunerDto>, ApiError> {
    let t = rustyfin_db::repo_live_tv::get_tuner(&state.db_ro, &tuner_id.to_string())
        .await?
        .ok_or_else(|| ApiError::NotFound("tuner not found".into()))?;
    Ok(Json(TunerDto {
        id: t.id,
        host_id: t.host_id,
        name: t.name,
        is_ready: t.is_ready,
        is_in_use: t.is_in_use,
    }))
}

// ── Listing Providers ──

async fn get_listing_providers(
    State(state): State<AppState>,
) -> Result<Json<QueryResult<ListingProviderDto>>, ApiError> {
    let providers = rustyfin_db::repo_live_tv::get_listing_providers(&state.db_ro).await?;
    let items: Vec<ListingProviderDto> = providers
        .into_iter()
        .map(|p| ListingProviderDto {
            id: p.id,
            name: p.name,
            r#type: p.r#type,
            is_ready: p.is_ready,
        })
        .collect();
    let total = items.len() as i64;
    Ok(Json(QueryResult {
        items,
        total_record_count: total,
        start_index: Some(0),
    }))
}

async fn create_listing_provider(
    State(state): State<AppState>,
    Json(req): Json<CreateListingProviderRequest>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let id = rustyfin_db::repo_live_tv::create_listing_provider(
        &state.db,
        &req.name,
        &req.r#type,
        &req.path,
    )
    .await?;
    Ok((StatusCode::OK, Json(serde_json::json!({ "Id": id }))))
}

async fn delete_listing_providers(State(state): State<AppState>) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_live_tv::delete_all_listing_providers(&state.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_listing_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
) -> Result<Json<ListingProviderDto>, ApiError> {
    let p = rustyfin_db::repo_live_tv::get_listing_provider(&state.db_ro, &provider_id.to_string())
        .await?
        .ok_or_else(|| ApiError::NotFound("listing provider not found".into()))?;
    Ok(Json(ListingProviderDto {
        id: p.id,
        name: p.name,
        r#type: p.r#type,
        is_ready: p.is_ready,
    }))
}

async fn delete_listing_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_live_tv::delete_listing_provider(&state.db, &provider_id.to_string()).await?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Channel Mapping Options ──

async fn get_channel_mapping_options(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let channels = rustyfin_db::repo_live_tv::get_channels(&state.db_ro).await?;
    let guides = rustyfin_db::repo_live_tv::get_listing_providers(&state.db_ro).await?;
    Ok(Json(serde_json::json!({
        "Channels": channels.iter().map(|ch| serde_json::json!({
            "Id": ch.id,
            "Name": ch.name,
            "ChannelNumber": ch.channel_number,
        })).collect::<Vec<_>>(),
        "Guides": guides.iter().map(|g| serde_json::json!({
            "Id": g.id,
            "Name": g.name,
        })).collect::<Vec<_>>(),
    })))
}

// ── Recordings Folders ──

async fn get_recordings_folders(
    State(_state): State<AppState>,
) -> Result<Json<QueryResult<serde_json::Value>>, ApiError> {
    // Recordings folders are derived from library paths
    Ok(Json(QueryResult {
        items: vec![],
        total_record_count: 0,
        start_index: Some(0),
    }))
}

// ── Next Live Media Source ──

async fn get_next_live_media_source(
    State(_state): State<AppState>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Returns the next live media source for a program
    Ok(Json(serde_json::json!({})))
}

// ── Program Media Sources ──

async fn get_program_media_sources(
    State(_state): State<AppState>,
    Path(_program_id): Path<Uuid>,
) -> Result<Json<Vec<serde_json::Value>>, ApiError> {
    // Media sources for a program (recordings or live streams)
    Ok(Json(vec![]))
}

// ── Program Stream ──

async fn get_program_stream(
    State(_state): State<AppState>,
    Path((_program_id, _media_source_id)): Path<(Uuid, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // Stream a program (recording or live)
    Ok(Json(serde_json::json!({})))
}
