//! Jellyfin's instant mix (`Jellyfin.Api/Controllers/InstantMixController.cs`, built by
//! `Emby.Server.Implementations/Library/MusicManager.cs`): up to 200 random tracks that share a
//! genre with the item. A song's mix starts with the song. An album's, artist's or playlist's genres
//! are its own, else its tracks'; a folder's or library's are its tracks'; a music genre is its own
//! genre. With no genre the mix is any 200 tracks, as Jellyfin's empty genre filter is no filter.
//! Another item, such as a movie, mixes nothing.

use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Extension, Json, Router};
use rustyfin_db::repo_by_name::{ByNameKind, by_name_id};
use rustyfin_db::repo_instant_mix as mix;
use rustyfin_model::auth::Claims;
use rustyfin_model::media::{MediaItem, MediaType, QueryResult};
use std::collections::HashSet;
use uuid::Uuid;

use super::items::{ImageOptions, enrich_items_with};
use crate::error::ApiError;
use crate::parental::HiddenItems;
use crate::query;
use crate::state::AppState;

/// The most tracks a mix holds, besides the song it starts from.
const MIX_SIZE: usize = 200;

type Params = Query<Vec<(String, String)>>;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/Items/{id}/InstantMix", get(item_mix))
        .route("/Songs/{id}/InstantMix", get(item_mix))
        .route("/Albums/{id}/InstantMix", get(item_mix))
        .route("/Artists/{id}/InstantMix", get(item_mix))
        .route("/Playlists/{id}/InstantMix", get(item_mix))
        .route("/Artists/InstantMix", get(item_mix_by_query))
        .route("/MusicGenres/InstantMix", get(item_mix_by_query))
        .route("/MusicGenres/{name}/InstantMix", get(genre_mix))
        .route(
            "/Users/{userId}/Items/{itemId}/InstantMix",
            get(user_item_mix),
        )
}

/// What a mix is built from: the song it starts with, and the genres its tracks share.
struct Seed {
    first: Option<MediaItem>,
    genres: Option<Vec<String>>,
}

impl Seed {
    fn genres(genres: Vec<String>) -> Self {
        Seed {
            first: None,
            genres: Some(genres),
        }
    }
}

async fn item_mix(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(params): Params,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let seed = seed_of(&state, &claims, id).await?;
    answer(&state, &claims, &params, seed).await.map(Json)
}

async fn user_item_mix(
    State(state): State<AppState>,
    Path((_user, id)): Path<(Uuid, Uuid)>,
    Query(params): Params,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let seed = seed_of(&state, &claims, id).await?;
    answer(&state, &claims, &params, seed).await.map(Json)
}

/// `/Artists/InstantMix?id=` and `/MusicGenres/InstantMix?id=`, Jellyfin's older forms.
async fn item_mix_by_query(
    State(state): State<AppState>,
    Query(params): Params,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    let id = query::uuid_param(&params, "id")?
        .ok_or_else(|| ApiError::BadRequest("id is required".into()))?;
    let seed = seed_of(&state, &claims, id).await?;
    answer(&state, &claims, &params, seed).await.map(Json)
}

async fn genre_mix(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Query(params): Params,
    Extension(claims): Extension<Claims>,
) -> Result<Json<QueryResult>, ApiError> {
    answer(&state, &claims, &params, Seed::genres(vec![name]))
        .await
        .map(Json)
}

/// Or the tracks' genres when the item has none of its own.
fn own_or(own: &[String], tracks: Vec<String>) -> Vec<String> {
    if own.is_empty() { tracks } else { own.to_vec() }
}

/// The seed of the item `id`: a library item, an artist known only by its credits, a playlist, a
/// genre, or a library. 404 when it is none of them, or hidden from the caller.
async fn seed_of(state: &AppState, claims: &Claims, id: Uuid) -> Result<Seed, ApiError> {
    let pool = &state.db_ro;
    let hidden = HiddenItems::for_caller(state, claims).await?;
    if let Some(item) = rustyfin_db::repo_media::get_item_by_id(pool, id).await? {
        if hidden.hides(item.id, item.library_id) {
            return Err(ApiError::NotFound("Item not found".into()));
        }
        let seed = match item.media_type {
            MediaType::Audio => Seed {
                genres: Some(item.genres.clone()),
                first: Some(item),
            },
            MediaType::MusicAlbum | MediaType::Playlist => {
                let tracks = mix::track_genres_under(pool, item.id).await?;
                Seed::genres(own_or(&item.genres, tracks))
            }
            MediaType::MusicArtist => {
                let person = rustyfin_db::repo_person::music_artist_person_id(&item.name);
                let tracks = mix::artist_genres(pool, person).await?;
                Seed::genres(own_or(&item.genres, tracks))
            }
            media if media.is_folder() => {
                let mut genres = item.genres.clone();
                genres.extend(mix::track_genres_under(pool, item.id).await?);
                Seed::genres(genres)
            }
            _ => Seed {
                first: None,
                genres: None,
            },
        };
        return Ok(seed);
    }
    if rustyfin_db::repo_person::music_artist_by_id(pool, id)
        .await?
        .is_some()
    {
        return Ok(Seed::genres(mix::artist_genres(pool, id).await?));
    }
    let caller = if claims.is_admin {
        rustyfin_db::repo_playlist::PlaylistCaller::Admin
    } else {
        rustyfin_db::repo_playlist::PlaylistCaller::Owner(claims.sub)
    };
    if rustyfin_db::repo_playlist::get_playlist(pool, id, caller)
        .await?
        .is_some()
    {
        let mut genres = Vec::new();
        for item in rustyfin_db::repo_playlist::get_playlist_items(pool, id, caller).await? {
            if item.media_type == MediaType::Audio {
                genres.extend(item.genres);
            }
        }
        return Ok(Seed::genres(genres));
    }
    if let Some(entry) = rustyfin_db::repo_by_name::get_by_id(pool, id).await?
        && matches!(entry.kind, ByNameKind::MusicGenre | ByNameKind::Genre)
    {
        return Ok(Seed::genres(vec![entry.name]));
    }
    if let Some(library) = rustyfin_db::repo_library::get_library_by_id(pool, id).await? {
        if hidden.hides(Uuid::nil(), library.id) {
            return Err(ApiError::NotFound("Item not found".into()));
        }
        return Ok(Seed::genres(
            mix::track_genres_in_library(pool, library.id).await?,
        ));
    }
    Err(ApiError::NotFound("Item not found".into()))
}

/// The ids of the tracks of the mix, in random order, without the song it starts from.
async fn mix_track_ids(
    state: &AppState,
    hidden: &HiddenItems,
    genres: &[String],
    first: Option<Uuid>,
) -> Result<Vec<Uuid>, ApiError> {
    let pool = &state.db_ro;
    let kept: Option<HashSet<String>> = if genres.is_empty() {
        None
    } else {
        let ids: Vec<Uuid> = genres
            .iter()
            .map(|name| by_name_id(ByNameKind::MusicGenre, name))
            .collect();
        rustyfin_db::repo_by_name::item_ids_with(pool, &ids, &[])
            .await?
            .map(|ids| ids.into_iter().collect())
    };
    Ok(mix::all_tracks(pool)
        .await?
        .into_iter()
        .filter(|(id, _)| kept.as_ref().is_none_or(|k| k.contains(&id.to_string())))
        .filter(|(id, library)| !hidden.hides(*id, *library))
        .map(|(id, _)| id)
        .take(MIX_SIZE)
        .filter(|id| Some(*id) != first)
        .collect())
}

async fn answer(
    state: &AppState,
    claims: &Claims,
    params: &[(String, String)],
    seed: Seed,
) -> Result<QueryResult, ApiError> {
    let limit = query::count_param(params, "limit")?;
    let Some(genres) = seed.genres else {
        return Ok(QueryResult {
            items: Vec::new(),
            total_record_count: 0,
            start_index: 0,
        });
    };
    let hidden = HiddenItems::for_caller(state, claims).await?;
    let first_id = seed.first.as_ref().map(|item| item.id);
    let mut ids: Vec<Uuid> = first_id.into_iter().collect();
    ids.extend(mix_track_ids(state, &hidden, &genres, first_id).await?);
    let total = ids.len() as i64;
    if let Some(limit) = limit {
        ids.truncate(limit);
    }
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(item) = rustyfin_db::repo_media::get_item_by_id(&state.db_ro, id).await? {
            items.push(item);
        }
    }

    let enable_images = query::bool_param(params, "enableImages")?.unwrap_or(true);
    let types = query::first_value(params, "enableImageTypes").map(|types| {
        types
            .split(',')
            .filter_map(|t| rustyfin_model::image::ImageType::from_str(t.trim()))
            .collect::<Vec<_>>()
    });
    let images = ImageOptions {
        types: types.filter(|t| !t.is_empty()),
        limit: if enable_images {
            query::count_param(params, "imageTypeLimit")?
        } else {
            Some(0)
        },
    };
    let user = query::uuid_param(params, "userId")?.unwrap_or(claims.sub);
    let user_data = query::bool_param(params, "enableUserData")?.unwrap_or(true);
    let tmdb = state.tmdb.api(None);
    enrich_items_with(
        &state.db_ro,
        &state.server_id,
        user_data.then_some(user),
        &mut items,
        &state.http_client,
        tmdb.as_ref(),
        &state.data_dir,
        true,
        &images,
    )
    .await;
    Ok(QueryResult {
        items,
        total_record_count: total,
        start_index: 0,
    })
}
