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

/// A user's language preference as Jellyfin's `MediaSourceManager.NormalizeLanguage` reads it:
/// the three-letter codes of the culture it names (by name, two- or three-letter code, any case),
/// the preference itself when it names none, and nothing when it is empty.
pub(crate) fn normalize_language(language: &str) -> Vec<String> {
    if language.is_empty() {
        return Vec::new();
    }
    let found = cultures().into_iter().find(|culture| {
        [
            culture.name,
            culture.two_letter_iso_language_name,
            culture.three_letter_iso_language_name,
        ]
        .iter()
        .chain(culture.three_letter_iso_language_names.iter())
        .any(|code| !code.is_empty() && code.eq_ignore_ascii_case(language))
    });
    match found {
        Some(culture) => culture
            .three_letter_iso_language_names
            .iter()
            .map(|code| code.to_string())
            .collect(),
        None => vec![language.to_string()],
    }
}

/// The display name of the culture a language names (by display name, name, or two- or
/// three-letter code, any case), as Jellyfin 12.4's `LocalizationManager.FindLanguageInfo`
/// finds it; `None` when it names none.
pub(crate) fn language_display_name(language: &str) -> Option<&'static str> {
    if language.is_empty() {
        return None;
    }
    cultures()
        .into_iter()
        .find(|culture| {
            [
                culture.display_name,
                culture.name,
                culture.two_letter_iso_language_name,
            ]
            .iter()
            .chain(culture.three_letter_iso_language_names.iter())
            .any(|code| !code.is_empty() && code.eq_ignore_ascii_case(language))
        })
        .map(|culture| culture.display_name)
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

/// Jellyfin's `LocalizationOption`: a display language the web client is offered.
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct LocalizationOption {
    name: &'static str,
    value: &'static str,
}

/// The display languages, as Jellyfin 12.4's `LocalizationManager.GetLocalizationOptions` lists
/// them: one per bundled server translation, named in its own language (the culture's
/// `NativeName`, or the code where none is known), and `English` for `en-US`, sorted by name
/// without case.
const LOCALIZATION_OPTIONS: &[(&str, &str)] = &[
    ("Abkhazian", "ab"),
    ("Afrikaans", "af"),
    ("azərbaycan", "az"),
    ("bosanski", "bs"),
    ("brezhoneg", "br"),
    ("català", "ca"),
    ("chiShona", "sn"),
    ("Cymraeg", "cy"),
    ("dansk", "da"),
    ("Deutsch", "de"),
    ("eesti", "et"),
    ("English", "en-US"),
    ("English (United Kingdom)", "en-GB"),
    ("español", "es"),
    ("español (Argentina)", "es-AR"),
    ("español (Latinoamérica)", "es_419"),
    ("español (México)", "es-MX"),
    ("español (República Dominicana)", "es_DO"),
    ("Esperanto", "eo"),
    ("euskara", "eu"),
    ("Filipino", "fil"),
    ("français", "fr"),
    ("français (Canada)", "fr-CA"),
    ("føroyskt", "fo"),
    ("Gaeilge", "ga"),
    ("galego", "gl"),
    ("Haitian Creole", "ht"),
    ("hrvatski", "hr"),
    ("Indonesia", "id"),
    ("isiZulu", "zu"),
    ("italiano", "it"),
    ("kernewek", "kw"),
    ("Kiswahili", "sw"),
    ("latviešu", "lv"),
    ("lietuvių (Lietuva)", "lt-LT"),
    ("Literary Chinese", "lzh"),
    ("Lojban", "jbo"),
    ("Lëtzebuergesch", "lb"),
    ("magyar", "hu"),
    ("Malti", "mt"),
    ("Melayu", "ms"),
    ("Māori", "mi"),
    ("Nederlands", "nl"),
    ("norsk bokmål", "nb"),
    ("norsk nynorsk", "nn"),
    ("occitan", "oc"),
    ("o‘zbek", "uz"),
    ("polski", "pl"),
    ("português", "pt"),
    ("português (Brasil)", "pt-BR"),
    ("português (Portugal)", "pt-PT"),
    ("pr", "pr"),
    ("română", "ro"),
    ("Schwiizertüütsch", "gsw"),
    ("shqip", "sq"),
    ("slovenčina", "sk"),
    ("slovenščina (Slovenija)", "sl-SI"),
    ("suomi", "fi"),
    ("svenska", "sv"),
    ("Taqbaylit", "kab"),
    ("Tiếng Việt", "vi"),
    ("Türkçe", "tr"),
    ("íslenska", "is"),
    ("čeština", "cs"),
    ("Ελληνικά", "el"),
    ("беларуская", "be"),
    ("български (България)", "bg-BG"),
    ("кыргызча", "ky"),
    ("македонски", "mk"),
    ("монгол", "mn"),
    ("русский", "ru"),
    ("српски", "sr"),
    ("українська", "uk"),
    ("қазақ тілі", "kk"),
    ("հայերեն", "hy"),
    ("עברית", "he"),
    ("עברית (ישראל)", "he_IL"),
    ("ئۇيغۇرچە", "ug"),
    ("اردو", "ur"),
    ("اردو (پاکستان)", "ur_PK"),
    ("العربية", "ar"),
    ("العربية (المملكة العربية السعودية)", "ar_SA"),
    ("فارسی", "fa"),
    ("नेपाली", "ne"),
    ("मराठी", "mr"),
    ("हिन्दी", "hi"),
    ("অসমীয়া", "as"),
    ("বাংলা", "bn"),
    ("ਪੰਜਾਬੀ", "pa"),
    ("ଓଡ଼ିଆ", "or"),
    ("தமிழ்", "ta"),
    ("తెలుగు", "te"),
    ("ಕನ್ನಡ", "kn"),
    ("മലയാളം", "ml"),
    ("සිංහල", "si"),
    ("ไทย", "th"),
    ("မြန်မာ", "my"),
    ("ᏣᎳᎩ", "chr"),
    ("ខ្មែរ", "km"),
    ("ქართული", "ka"),
    ("中文（中国）", "zh-CN"),
    ("中文（中國香港特別行政區）", "zh-HK"),
    ("中文（台灣）", "zh-TW"),
    ("日本語", "ja"),
    ("한국어", "ko"),
];

/// GET /Localization/Options - the display languages, Jellyfin's list of `{ Name, Value }`
async fn get_options() -> Json<Vec<LocalizationOption>> {
    Json(
        LOCALIZATION_OPTIONS
            .iter()
            .map(|&(name, value)| LocalizationOption { name, value })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::normalize_language;

    /// A preference names a culture by any of its codes and becomes its three-letter codes; one
    /// that names none is kept, and an empty one is no preference.
    #[test]
    fn r3gaps3_language_preferences_normalize_as_jellyfin() {
        assert_eq!(normalize_language("en"), vec!["eng"]);
        assert_eq!(normalize_language("ENG"), vec!["eng"]);
        assert_eq!(normalize_language("ger"), vec!["deu", "ger"]);
        assert_eq!(
            normalize_language("OriginalLanguage"),
            vec!["OriginalLanguage"]
        );
        assert!(normalize_language("").is_empty());
    }
}
