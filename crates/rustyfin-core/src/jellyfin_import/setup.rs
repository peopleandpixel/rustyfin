//! The setup wizard's preview (B119): what a Jellyfin data folder holds, as names and counts, before
//! any owner exists. Writes nothing; the spec's "API → POST /Startup/JellyfinImport/Preview".

use std::collections::HashSet;
use std::path::Path;

use rustyfin_model::library::LibraryContentType;
use serde::Serialize;
use uuid::Uuid;

use super::{ImportError, Layout, LayoutChoice, source};

/// The setup preview's answer (B119). Names, admin flags, library names, types and paths, and counts
/// only: no password, token, item path or user data.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SetupPreview {
    pub layout: Layout,
    pub data_path: String,
    pub items: usize,
    pub users: Vec<SetupUser>,
    pub libraries: Vec<SetupLibrary>,
}

/// A Jellyfin user in the setup preview, in source order.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SetupUser {
    pub jellyfin_id: Uuid,
    pub name: String,
    pub was_administrator: bool,
    /// The user's played items: their rows marked played whose item is among the source's items.
    pub played_items: usize,
}

/// A Jellyfin library in the setup preview: its name, its collection type as Jellyfin names it,
/// the Rustyfin content type that type maps to, and its media paths.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SetupLibrary {
    pub name: String,
    pub collection_type: Option<String>,
    pub content_type: Option<LibraryContentType>,
    pub paths: Vec<String>,
}

/// Reads the source at `source_path` for the setup wizard's preview (B119). A database too big for
/// memory is copied into `data_dir` while it is read, as the admin preview does.
pub async fn setup_preview(
    source_path: &str,
    layout: LayoutChoice,
    data_dir: &Path,
) -> Result<SetupPreview, ImportError> {
    let location = source::locate(Path::new(source_path))?;
    let snapshot = source::read_snapshot(&location, layout, data_dir).await?;
    let folders = source::read_library_folders(&location.program_data_path)?;

    let items: HashSet<Uuid> = snapshot.items.iter().map(|item| item.id).collect();
    let users: Vec<SetupUser> = snapshot
        .users
        .iter()
        .map(|user| SetupUser {
            jellyfin_id: user.id,
            name: user.name.clone(),
            was_administrator: user.is_admin,
            played_items: snapshot
                .user_data
                .iter()
                .filter(|row| row.user == user.id && row.played && items.contains(&row.item))
                .count(),
        })
        .collect();
    let libraries: Vec<SetupLibrary> = folders
        .into_iter()
        .map(|folder| SetupLibrary {
            content_type: content_type(folder.collection_type.as_deref()),
            name: folder.name,
            collection_type: folder.collection_type,
            paths: folder.mblinks,
        })
        .collect();

    tracing::info!(
        "jellyfin import setup preview: {} users, {} libraries, {} items",
        users.len(),
        libraries.len(),
        snapshot.items.len(),
    );
    Ok(SetupPreview {
        layout: snapshot.layout,
        data_path: location.data_path.display().to_string(),
        items: snapshot.items.len(),
        users,
        libraries,
    })
}

/// The Rustyfin content type of a Jellyfin collection type; `None` for a type Rustyfin has no name for.
/// Home videos map to Mixed, not Photos: Rustyfin's Photos library scans only pictures, and a
/// Jellyfin home-video folder holds videos too (B119).
fn content_type(collection_type: Option<&str>) -> Option<LibraryContentType> {
    match collection_type {
        Some("movies") => Some(LibraryContentType::Movies),
        Some("tvshows") => Some(LibraryContentType::TvShows),
        Some("music") => Some(LibraryContentType::Music),
        Some("photos") => Some(LibraryContentType::Photos),
        Some("homevideos") => Some(LibraryContentType::Mixed),
        None | Some("mixed") => Some(LibraryContentType::Mixed),
        Some(_) => None,
    }
}
