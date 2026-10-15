use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};

use rustyfin_model::configuration::{
    DisplayConfiguration, MediaEncodingConfiguration, NetworkingConfiguration,
    ParentalControlsConfiguration, ServerRecordingConfiguration,
};

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/Configuration/MediaEncoding",
            get(get_media_encoding).put(update_media_encoding),
        )
        .route(
            "/Configuration/Networking",
            get(get_networking).put(update_networking),
        )
        .route("/Configuration/ServerRecording", get(get_server_recording))
        .route("/Configuration/Display", get(get_display))
        .route(
            "/Configuration/ParentalControls",
            get(get_parental_controls),
        )
}

async fn get_media_encoding() -> Json<MediaEncodingConfiguration> {
    Json(MediaEncodingConfiguration::default())
}

async fn update_media_encoding(
    Json(config): Json<MediaEncodingConfiguration>,
) -> Json<MediaEncodingConfiguration> {
    // Store in memory for now; could persist to DB later
    Json(config)
}

async fn get_networking(State(state): State<AppState>) -> Json<NetworkingConfiguration> {
    let config = state.config.read().await;
    Json(NetworkingConfiguration {
        published_server_uri: config.server_name.clone(), // Placeholder; real value from env
        ..NetworkingConfiguration::default()
    })
}

async fn update_networking(
    Json(config): Json<NetworkingConfiguration>,
) -> Json<NetworkingConfiguration> {
    Json(config)
}

async fn get_server_recording() -> Json<ServerRecordingConfiguration> {
    Json(ServerRecordingConfiguration::default())
}

async fn get_display() -> Json<DisplayConfiguration> {
    Json(DisplayConfiguration::default())
}

async fn get_parental_controls() -> Json<ParentalControlsConfiguration> {
    Json(ParentalControlsConfiguration::default())
}
