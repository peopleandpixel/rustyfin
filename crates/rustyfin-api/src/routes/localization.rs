use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
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
        .route("/Localization/Providers", get(get_providers))
        .route("/Localization/Options", get(get_options))
        .route("/Localization/ParentalRatings", get(get_parental_ratings))
}

/// The ratings a user's limit can be set to, for the parental control editor.
async fn get_parental_ratings() -> Json<Vec<serde_json::Value>> {
    Json(rustyfin_model::parental::parental_ratings())
}

/// Language and country lists, Jellyfin's FirstTimeSetupOrDefault routes: `routes::build_router`
/// wraps them in `require_setup_or_auth`, so the setup wizard can read them before any user exists.
pub fn setup_or_user_routes() -> Router<AppState> {
    Router::new()
        .route("/Localization/Cultures", get(get_cultures))
        .route("/Localization/Countries", get(get_countries))
}

/// Jellyfin's `CultureDto`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct CultureDto {
    name: &'static str,
    display_name: &'static str,
    #[serde(rename = "TwoLetterISOLanguageName")]
    two_letter_iso_language_name: &'static str,
    #[serde(rename = "ThreeLetterISOLanguageName")]
    three_letter_iso_language_name: &'static str,
    #[serde(rename = "ThreeLetterISOLanguageNames")]
    three_letter_iso_language_names: Vec<&'static str>,
}

/// Jellyfin's `CountryInfo`; its `Name` is the two-letter code.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct CountryInfo {
    name: &'static str,
    display_name: &'static str,
    #[serde(rename = "TwoLetterISORegionName")]
    two_letter_iso_region_name: &'static str,
    #[serde(rename = "ThreeLetterISORegionName")]
    three_letter_iso_region_name: &'static str,
}

/// ISO 639-2 languages (see `data/README.md`): `bibliographic|terminologic|alpha-2|name`.
const LANGUAGES: &str = include_str!("../../data/iso639-2.txt");
/// ISO 3166-1 countries (see `data/README.md`): `alpha-2|alpha-3|name`.
const COUNTRIES: &str = include_str!("../../data/iso3166-1.txt");

/// The cultures as Jellyfin lists them: the ISO 639-2/T code first where it differs from the
/// bibliographic one (`deu`, then `ger`).
fn cultures() -> Vec<CultureDto> {
    LANGUAGES
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('|');
            let (b, t, two, name) = (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
            let codes = if t.is_empty() { vec![b] } else { vec![t, b] };
            Some(CultureDto {
                name,
                display_name: name,
                two_letter_iso_language_name: two,
                three_letter_iso_language_name: codes[0],
                three_letter_iso_language_names: codes,
            })
        })
        .collect()
}

fn countries() -> Vec<CountryInfo> {
    COUNTRIES
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('|');
            let (two, three, name) = (parts.next()?, parts.next()?, parts.next()?);
            Some(CountryInfo {
                name: two,
                display_name: name,
                two_letter_iso_region_name: two,
                three_letter_iso_region_name: three,
            })
        })
        .collect()
}

/// GET /Localization/Cultures - every ISO 639-2 language
async fn get_cultures() -> Json<Vec<CultureDto>> {
    Json(cultures())
}

/// GET /Localization/Countries - every ISO 3166-1 country
async fn get_countries() -> Json<Vec<CountryInfo>> {
    Json(countries())
}

/// Administrator-only localization routes. `routes::build_router` wraps them in `require_admin`.
pub fn admin_routes() -> Router<AppState> {
    Router::new()
        .route("/Localization/Providers", post(add_provider))
        .route(
            "/Localization/Providers/{providerId}",
            put(update_provider).delete(delete_provider),
        )
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
