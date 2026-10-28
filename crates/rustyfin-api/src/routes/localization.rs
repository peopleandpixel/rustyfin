use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::ApiError;
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LocalizationProviderDto {
    id: String,
    name: String,
    #[serde(rename = "ProviderId")]
    provider_id: String,
    enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    configuration: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AddProviderRequest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    provider_id: String,
    #[serde(default)]
    configuration: serde_json::Value,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateProviderRequest {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    configuration: serde_json::Value,
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/Localization/Providers",
            get(get_providers).post(add_provider),
        )
        .route(
            "/Localization/Providers/{providerId}",
            put(update_provider).delete(delete_provider),
        )
        .route("/Localization/Options", get(get_options))
}

async fn get_providers(
    State(state): State<AppState>,
) -> Result<Json<Vec<LocalizationProviderDto>>, ApiError> {
    let providers = rustyfin_db::repo_misc::get_localization_providers(&state.db_ro)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    let items: Vec<LocalizationProviderDto> = providers
        .into_iter()
        .map(|p| LocalizationProviderDto {
            id: p.id,
            name: p.name,
            provider_id: p.provider_id,
            enabled: p.enabled,
            configuration: serde_json::from_str(&p.configuration).ok(),
        })
        .collect();

    Ok(Json(items))
}

async fn add_provider(
    State(state): State<AppState>,
    Json(req): Json<AddProviderRequest>,
) -> Result<StatusCode, ApiError> {
    let id = Uuid::new_v4().to_string();
    let config = serde_json::to_string(&req.configuration).unwrap_or_else(|_| "{}".to_string());

    rustyfin_db::repo_misc::add_localization_provider(
        &state.db,
        &id,
        &req.name,
        &req.provider_id,
        &config,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn update_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
    Json(req): Json<UpdateProviderRequest>,
) -> Result<StatusCode, ApiError> {
    let config = serde_json::to_string(&req.configuration).unwrap_or_else(|_| "{}".to_string());

    rustyfin_db::repo_misc::update_localization_provider(
        &state.db,
        &provider_id,
        req.enabled,
        &config,
    )
    .await
    .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

async fn delete_provider(
    State(state): State<AppState>,
    Path(provider_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    rustyfin_db::repo_misc::delete_localization_provider(&state.db, &provider_id)
        .await
        .map_err(|e| ApiError::Internal(e.to_string()))?;

    Ok(StatusCode::OK)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LocalizationOptions {
    #[serde(rename = "SubtitleLanguages")]
    subtitle_languages: Vec<LanguageDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LanguageDto {
    two_letter_code: String,
    english_name: String,
    local_name: String,
}

async fn get_options(_state: State<AppState>) -> Result<Json<LocalizationOptions>, ApiError> {
    // Return a default set of common subtitle languages
    let languages = vec![
        LanguageDto {
            two_letter_code: "en".to_string(),
            english_name: "English".to_string(),
            local_name: "English".to_string(),
        },
        LanguageDto {
            two_letter_code: "de".to_string(),
            english_name: "German".to_string(),
            local_name: "Deutsch".to_string(),
        },
        LanguageDto {
            two_letter_code: "fr".to_string(),
            english_name: "French".to_string(),
            local_name: "Français".to_string(),
        },
        LanguageDto {
            two_letter_code: "es".to_string(),
            english_name: "Spanish".to_string(),
            local_name: "Español".to_string(),
        },
        LanguageDto {
            two_letter_code: "it".to_string(),
            english_name: "Italian".to_string(),
            local_name: "Italiano".to_string(),
        },
        LanguageDto {
            two_letter_code: "ja".to_string(),
            english_name: "Japanese".to_string(),
            local_name: "日本語".to_string(),
        },
        LanguageDto {
            two_letter_code: "ko".to_string(),
            english_name: "Korean".to_string(),
            local_name: "한국어".to_string(),
        },
        LanguageDto {
            two_letter_code: "pt".to_string(),
            english_name: "Portuguese".to_string(),
            local_name: "Português".to_string(),
        },
        LanguageDto {
            two_letter_code: "ru".to_string(),
            english_name: "Russian".to_string(),
            local_name: "Русский".to_string(),
        },
        LanguageDto {
            two_letter_code: "zh".to_string(),
            english_name: "Chinese".to_string(),
            local_name: "中文".to_string(),
        },
    ];

    Ok(Json(LocalizationOptions {
        subtitle_languages: languages,
    }))
}
