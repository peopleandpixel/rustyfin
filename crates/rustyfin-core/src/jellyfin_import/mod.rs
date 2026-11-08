//! Import from Jellyfin (B20): users, watch data and track choices from a Jellyfin data folder,
//! read read-only. Spec: the B20 brief and spec, Stage 2.

pub mod images;
pub mod job;
pub mod merge;
pub mod metadata;
pub mod paths;
pub mod plan;
pub mod source;
pub mod tracks;

#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, hash_map::Entry};
use std::path::{Path, PathBuf};

use chrono::Utc;
pub use job::{ImportJob, ImportStage, ImportStatus, JobHandle, JobState};
pub use metadata::{MetadataMode, MetadataReport};
use paths::PathMapping;
use rustyfin_auth::{generate_setup_code, hash_setup_code, unusable_password_hash};
use rustyfin_db::repo_jellyfin_import::{
    ImportTarget, SourceStreams, UserDataRow, item_sources, user_data_rows, write_user_data,
};
use rustyfin_model::user::User;
use serde::{Deserialize, Serialize};
use source::{JfItem, JfStream, JfUserData, StreamKind};
use sqlx::SqlitePool;
use uuid::Uuid;

/// Why an import can't read its source, or failed writing.
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    /// The source folder or one of its databases is unusable (a 400; names the file, no row data).
    #[error("{0}")]
    BadSource(String),
    /// Reading or writing Rustyfin's own database failed (a 500). Deliberately no `From`, so a
    /// failed read of the source can't become a 500 by `?`; source errors are `BadSource`.
    #[error(transparent)]
    Db(sqlx::Error),
    /// Hashing a password or a setup code failed (a 500; the text names no secret).
    #[error("hashing a password or code failed")]
    Hash,
}

/// The layout a request asks for (spec 2.2).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum LayoutChoice {
    #[default]
    Auto,
    New,
    Old,
}

/// The layout a source was read as (spec 2.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Layout {
    New,
    Old,
}

/// An import request (spec 2.17). Unknown fields are ignored.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImportRequest {
    pub source_path: String,
    #[serde(default)]
    pub layout: LayoutChoice,
    /// Absent: use the suggested pairs. Present (even empty): use exactly these.
    pub path_mappings: Option<Vec<PathMapping>>,
    /// Jellyfin user ids to import; absent means all users.
    pub user_ids: Option<Vec<Uuid>>,
    /// The suggested matches the admin confirmed (R2). A Jellyfin user's data goes into an
    /// existing account only when its pairing with that account's id is here.
    #[serde(default)]
    pub confirmed_matches: Vec<ConfirmedMatch>,
    /// How existing metadata is treated (spec 5.1); absent means `FillEmpty`.
    #[serde(default)]
    pub metadata_mode: MetadataMode,
}

/// A pairing the admin confirmed: the Jellyfin user and the existing account it goes into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ConfirmedMatch {
    pub jellyfin_id: Uuid,
    pub user_id: Uuid,
}

/// The preview's answer (spec 2.20), and a run's report. `Metadata` is the spec 5 part.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImportReport {
    pub layout: Layout,
    pub data_path: String,
    pub path_mappings: Vec<PathMapping>,
    pub suggested_path_mappings: Vec<PathMapping>,
    pub unpaired_library_paths: Vec<String>,
    pub users: Vec<UserRow>,
    pub items: ItemReport,
    pub watch_data: WatchDataReport,
    pub tracks: TrackReport,
    pub metadata: MetadataReport,
}

/// The watch data of the report (spec 2.20, 2.32): `imported + unchanged + unmatched` is `rows`.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct WatchDataReport {
    /// The rows of the importing users, one per user and item.
    pub rows: usize,
    /// The rows that change a Rustyfin row (a run writes them).
    pub imported: usize,
    /// The rows that leave a Rustyfin row as it is.
    pub unchanged: usize,
    /// The rows whose Jellyfin item matches no Rustyfin item.
    pub unmatched: usize,
}

/// The track choices of the report (spec 2.20, 2.32).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct TrackReport {
    /// Audio choices mapped to a Rustyfin stream.
    pub audio: usize,
    /// Subtitle choices mapped to a Rustyfin stream.
    pub subtitle: usize,
    /// Subtitle choices that switch subtitles off.
    pub subtitle_off: usize,
    /// Choices with no Rustyfin stream.
    pub unmapped: usize,
}

/// One Jellyfin user and what the import does with them (spec 2.5, 2.6, 2.20).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserRow {
    pub jellyfin_id: Uuid,
    pub name: String,
    pub was_administrator: bool,
    pub action: UserAction,
    pub reason: Option<SkipReason>,
    /// The Rustyfin user: the confirmed match for `Match`; null for `Create` in a preview.
    pub user_id: Option<Uuid>,
    /// The existing account with this name, for a picked user: the data goes there only when
    /// the request confirms the pairing (R2).
    pub suggested_match: Option<SuggestedMatch>,
    /// The one-time code of a created user; only a run has codes.
    pub code: Option<String>,
}

/// The existing account a Jellyfin user's name matches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SuggestedMatch {
    pub user_id: Uuid,
    pub name: String,
}

/// What an import does with a Jellyfin user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum UserAction {
    Match,
    Create,
    Skip,
}

/// Why a Jellyfin user is skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SkipReason {
    NotSelected,
    Ambiguous,
    /// The name matches an existing account, and the request doesn't confirm that pairing.
    NotConfirmed,
    /// The request confirms a pairing with an account that isn't the suggested match.
    ConfirmedOtherAccount,
}

/// The item counts of the report (spec 2.11, 2.20). The three counts add up to `total`.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemReport {
    pub total: usize,
    pub matched_by_path: usize,
    pub matched_by_id: usize,
    pub unmatched: usize,
    pub unmatched_samples: Vec<UnmatchedSample>,
}

/// An unmatched item that has watch data for an imported user.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UnmatchedSample {
    #[serde(rename = "Type")]
    pub item_type: String,
    pub name: String,
    pub path: String,
}

/// Reads the Jellyfin data folder named by `req` and reports what an import would do. Writes
/// nothing, here or in the source. `data_dir` is Rustyfin's data folder (spec 5.9).
pub async fn preview(
    db: &SqlitePool,
    req: &ImportRequest,
    data_dir: &Path,
) -> Result<ImportReport, ImportError> {
    let prepared = prepare(req, data_dir).await?;
    let report = plan_report(db, req, &prepared, data_dir, None).await?;

    let layout = report.layout;
    let source = &req.source_path;
    let user_count = report.users.len();
    let total = report.items.total;
    let (by_path, by_id, unmatched) = (
        report.items.matched_by_path,
        report.items.matched_by_id,
        report.items.unmatched,
    );
    let (rows, imported, unchanged, watch_unmatched) = (
        report.watch_data.rows,
        report.watch_data.imported,
        report.watch_data.unchanged,
        report.watch_data.unmatched,
    );
    let (mode, items, fields, people, images, missing, locked) = (
        report.metadata.mode,
        report.metadata.items,
        report.metadata.fields,
        report.metadata.people,
        report.metadata.images,
        report.metadata.images_missing,
        report.metadata.locked,
    );
    tracing::info!(
        "jellyfin import preview: source {source}, layout {layout:?}, {user_count} users, {total} items ({by_path} by path, {by_id} by id, {unmatched} unmatched), {rows} watch rows ({imported} imported, {unchanged} unchanged, {watch_unmatched} unmatched), metadata {mode:?} ({items} items, {fields} fields, {people} people, {images} images, {missing} missing, {locked} locked)"
    );
    Ok(report)
}

/// A source read for a run (spec 2.26).
pub struct Prepared {
    pub location: source::SourceLocation,
    pub snapshot: source::Snapshot,
    pub folders: Vec<source::LibraryFolder>,
}

/// Locates and reads the source of `req`, with the preview's errors (spec 2.17, 2.18, 2.26). A
/// database too big for memory is copied into `data_dir` while it is read (`source::read_snapshot`).
pub async fn prepare(req: &ImportRequest, data_dir: &Path) -> Result<Prepared, ImportError> {
    let location = source::locate(Path::new(&req.source_path))?;
    let snapshot = source::read_snapshot(&location, req.layout, data_dir).await?;
    let folders = source::read_library_folders(&location.program_data_path)?;
    Ok(Prepared {
        location,
        snapshot,
        folders,
    })
}

/// The planning `preview` and `run` share (spec 2.20, 2.26). A run passes its handle: it then
/// reports the Reading, Users and Matching stages and creates the users it plans to create, so
/// the items are matched with the final actions.
async fn plan_report(
    db: &SqlitePool,
    req: &ImportRequest,
    prepared: &Prepared,
    data_dir: &Path,
    handle: Option<&JobHandle>,
) -> Result<ImportReport, ImportError> {
    if let Some(handle) = handle {
        handle.stage(ImportStage::Reading, 0);
    }
    let libraries = rustyfin_db::repo_library::get_all_libraries(db)
        .await
        .map_err(ImportError::Db)?;
    let rustyfin_users = rustyfin_db::repo_jellyfin_import::user_names(db)
        .await
        .map_err(ImportError::Db)?;
    let targets = rustyfin_db::repo_jellyfin_import::import_targets(db)
        .await
        .map_err(ImportError::Db)?;

    let mut suggested = paths::suggest(
        &prepared.folders,
        &libraries,
        &prepared.location.program_data_path,
        &prepared.location.data_path,
    );
    let mut path_mappings = req
        .path_mappings
        .clone()
        .unwrap_or_else(|| suggested.pairs.clone());
    // The ids this run's metadata step writes count for matching (spec 5.14).
    let targets = metadata::targets_after(
        db,
        req.metadata_mode,
        &prepared.snapshot,
        &targets,
        &path_mappings,
    )
    .await?;
    let mut users = plan::plan_users(
        &prepared.snapshot.users,
        req.user_ids.as_deref(),
        &req.confirmed_matches,
        &rustyfin_users,
    );
    if let Some(handle) = handle {
        create_users(db, &mut users, rustyfin_users, handle).await?;
    }
    let importing: HashSet<Uuid> = users
        .iter()
        .filter(|user| matches!(user.action, UserAction::Match | UserAction::Create))
        .map(|user| user.jellyfin_id)
        .collect();
    let watched: HashSet<Uuid> = prepared
        .snapshot
        .user_data
        .iter()
        .filter(|row| importing.contains(&row.user))
        .map(|row| row.item)
        .collect();
    if let Some(handle) = handle {
        handle.stage(ImportStage::Matching, 0);
    }
    let items = plan::match_items(&prepared.snapshot.items, &targets, &path_mappings, &watched);
    let (watch_data, track_counts) =
        plan_watch_data(db, prepared, &targets, &users, &path_mappings, handle).await?;

    // Metadata (spec 5.9): the metadata folder's pair is suggested from the matched items' images.
    let matched = plan::matched_items(&prepared.snapshot.items, &targets, &path_mappings);
    let by_path = plan::matched_by_path(&prepared.snapshot.items, &targets, &path_mappings);
    let image_paths = metadata_image_paths(&prepared.snapshot, &matched);
    if let Some(pair) = paths::suggest_metadata(
        &image_paths,
        &prepared.location.program_data_path,
        &suggested.pairs,
    ) {
        suggested.pairs.push(pair.clone());
        if req.path_mappings.is_none() {
            path_mappings.push(pair);
        }
    }
    let metadata_report = metadata::import(
        db,
        &metadata::MetadataInput {
            mode: req.metadata_mode,
            snapshot: &prepared.snapshot,
            matched: &matched,
            by_path: &by_path,
            pairs: &path_mappings,
            program_data: &prepared.location.program_data_path,
            data_dir,
        },
        handle,
    )
    .await?;

    Ok(ImportReport {
        layout: prepared.snapshot.layout,
        data_path: prepared.location.data_path.display().to_string(),
        path_mappings,
        suggested_path_mappings: suggested.pairs,
        unpaired_library_paths: suggested.unpaired,
        users,
        items,
        watch_data,
        tracks: track_counts,
        metadata: metadata_report,
    })
}

/// The image paths of the matched items and of the people credited on them (spec 5.9): the
/// inputs of the metadata folder's suggested pair.
fn metadata_image_paths<'a>(
    snapshot: &'a source::Snapshot,
    matched: &HashMap<Uuid, Uuid>,
) -> Vec<&'a str> {
    let mut paths = Vec::new();
    let mut seen_people: HashSet<&str> = HashSet::new();
    for jf_id in matched.keys() {
        if let Some(meta) = snapshot.metadata.items.get(jf_id) {
            paths.extend(meta.images.iter().map(|image| image.path.as_str()));
        }
        for credit in snapshot.metadata.credits.get(jf_id).into_iter().flatten() {
            let name = credit.name.trim();
            if !seen_people.insert(name) {
                continue;
            }
            if let Some(images) = snapshot.metadata.person_images.get(name) {
                paths.extend(images.iter().map(|image| image.path.as_str()));
            }
        }
    }
    paths
}

/// The watch-data step of the planning (spec 2.32): merges each importing user's rows into the
/// Rustyfin user's rows and counts the rows and the track choices. A run writes the changed rows;
/// a preview writes nothing.
async fn plan_watch_data(
    db: &SqlitePool,
    prepared: &Prepared,
    targets: &[ImportTarget],
    users: &[UserRow],
    pairs: &[PathMapping],
    handle: Option<&JobHandle>,
) -> Result<(WatchDataReport, TrackReport), ImportError> {
    let snapshot = &prepared.snapshot;
    let matched = plan::matched_items(&snapshot.items, targets, pairs);
    // Each importing Jellyfin user's Rustyfin user; None for a preview's Create.
    let owners: HashMap<Uuid, Option<Uuid>> = users
        .iter()
        .filter(|user| matches!(user.action, UserAction::Match | UserAction::Create))
        .map(|user| (user.jellyfin_id, user.user_id))
        .collect();
    let imported: Vec<JfUserData> = snapshot
        .user_data
        .iter()
        .filter(|row| owners.contains_key(&row.user))
        .cloned()
        .collect();
    let rows = merge::harmonize(imported);
    let mut report = WatchDataReport {
        rows: rows.len(),
        ..WatchDataReport::default()
    };
    let mut track_counts = TrackReport::default();
    if let Some(handle) = handle {
        handle.stage(ImportStage::WatchData, report.rows);
    }

    // Groups in order of first appearance. The rows of one Rustyfin user share a group, even when
    // two Jellyfin users match it; a preview's Create groups by its Jellyfin id.
    let mut groups: Vec<(Uuid, Option<Uuid>, Vec<JfUserData>)> = Vec::new();
    for row in rows {
        let owner = owners[&row.user];
        let key = owner.unwrap_or(row.user);
        let found = groups.iter().position(|group| group.0 == key);
        match found {
            Some(n) => groups[n].2.push(row),
            None => groups.push((key, owner, vec![row])),
        }
    }

    let mut mapper = TrackMapper {
        pairs,
        items: snapshot.items.iter().map(|item| (item.id, item)).collect(),
        streams: HashMap::new(),
        sources: HashMap::new(),
    };
    for stream in &snapshot.streams {
        mapper
            .streams
            .entry(stream.item)
            .or_default()
            .push(stream.clone());
    }

    let mut done = 0;
    for (_, owner, group_rows) in groups {
        // The tracks are mapped first, outside any transaction: the mapper reads through `db`.
        let mut mapped = Vec::with_capacity(group_rows.len());
        for row in &group_rows {
            done += 1;
            if let Some(handle) = handle {
                handle.progress(done);
            }
            let Some(&rf_item) = matched.get(&row.item) else {
                report.unmatched += 1;
                continue;
            };
            let tracks = mapper
                .map_tracks(db, rf_item, row, &mut track_counts)
                .await?;
            mapped.push((rf_item, row, tracks));
        }
        // A run with an owner reads, merges and writes in one `BEGIN IMMEDIATE` transaction, so a
        // progress report saved meanwhile is not overwritten (B20 fix 4). Between the begin and the
        // commit only `tx` is used: a second pool connection would wait on this transaction.
        if let (Some(_), Some(user)) = (handle, owner) {
            let mut tx = db
                .begin_with("BEGIN IMMEDIATE")
                .await
                .map_err(ImportError::Db)?;
            let (current, changed) = merge_group(&mut *tx, owner, mapped, &mut report).await?;
            let writes: Vec<(Uuid, UserDataRow)> = changed
                .iter()
                .filter_map(|item| current.get(item).map(|row| (*item, row.clone())))
                .collect();
            if !writes.is_empty() {
                write_user_data(&mut tx, user, &writes)
                    .await
                    .map_err(ImportError::Db)?;
            }
            tx.commit().await.map_err(ImportError::Db)?;
        } else {
            // A preview, or a group without an owner: merged for the counts, written nowhere.
            merge_group(db, owner, mapped, &mut report).await?;
        }
    }
    Ok((report, track_counts))
}

/// Merges one group's mapped rows into its owner's rows (spec 2.32) and counts them: `unchanged`
/// when the merge leaves a row as it was, `imported` otherwise. Returns the owner's rows after the
/// merge and the Rustyfin items whose row changed. Without an owner, the rows merge against nothing.
async fn merge_group<'e>(
    db: impl sqlx::SqliteExecutor<'e>,
    owner: Option<Uuid>,
    mapped: Vec<(Uuid, &JfUserData, merge::MappedTracks)>,
    report: &mut WatchDataReport,
) -> Result<(HashMap<Uuid, UserDataRow>, HashSet<Uuid>), ImportError> {
    let mut current = match owner {
        Some(user) => user_data_rows(db, user).await.map_err(ImportError::Db)?,
        None => HashMap::new(),
    };
    let mut changed: HashSet<Uuid> = HashSet::new();
    for (rf_item, row, tracks) in mapped {
        let merged = merge::merge(current.get(&rf_item), row, tracks);
        if current.get(&rf_item) == Some(&merged) {
            report.unchanged += 1;
        } else {
            report.imported += 1;
            current.insert(rf_item, merged);
            changed.insert(rf_item);
        }
    }
    Ok((current, changed))
}

/// The Jellyfin side of the track mapping (spec 2.30, 2.32), read once: the items and streams by
/// Jellyfin id, and each Rustyfin item's sources as a choice first needs them.
struct TrackMapper<'a> {
    pairs: &'a [PathMapping],
    items: HashMap<Uuid, &'a JfItem>,
    streams: HashMap<Uuid, Vec<JfStream>>,
    sources: HashMap<Uuid, Vec<SourceStreams>>,
}

impl TrackMapper<'_> {
    /// The mapped track choices of a matched row (spec 2.32), counted into `counts`. A subtitle of
    /// -1 switches subtitles off; an audio of -1 is no choice.
    async fn map_tracks(
        &mut self,
        db: &SqlitePool,
        rf_item: Uuid,
        row: &JfUserData,
        counts: &mut TrackReport,
    ) -> Result<merge::MappedTracks, ImportError> {
        let mut mapped = merge::MappedTracks::default();
        match row.subtitle {
            Some(-1) => {
                counts.subtitle_off += 1;
                mapped.subtitle = Some(-1);
            }
            Some(choice) => {
                let index = self
                    .choice_index(db, rf_item, row.item, choice, StreamKind::Subtitle)
                    .await?;
                match index {
                    Some((index, key)) => {
                        counts.subtitle += 1;
                        mapped.subtitle = Some(index);
                        mapped.subtitle_key = Some(key);
                    }
                    None => counts.unmapped += 1,
                }
            }
            None => {}
        }
        if let Some(choice) = row.audio.filter(|&choice| choice != -1) {
            let index = self
                .choice_index(db, rf_item, row.item, choice, StreamKind::Audio)
                .await?;
            match index {
                Some((index, key)) => {
                    counts.audio += 1;
                    mapped.audio = Some(index);
                    mapped.audio_key = Some(key);
                }
                None => counts.unmapped += 1,
            }
        }
        Ok(mapped)
    }

    /// The Rustyfin stream index a Jellyfin track choice maps to (spec 2.30, 2.32), with the key of
    /// that stream (see `StreamIdentity`), or None. The source is the one at the Jellyfin item's
    /// translated path (spec 2.16), and no other.
    async fn choice_index(
        &mut self,
        db: &SqlitePool,
        rf_item: Uuid,
        jf_item: Uuid,
        choice: i64,
        kind: StreamKind,
    ) -> Result<Option<(i64, String)>, ImportError> {
        let Some(jf) = self.items.get(&jf_item) else {
            return Ok(None);
        };
        let translated = jf
            .path
            .as_deref()
            .and_then(|path| paths::translate_path(path, self.pairs));
        let sources = match self.sources.entry(rf_item) {
            Entry::Occupied(found) => found.into_mut(),
            Entry::Vacant(vacant) => {
                let found = item_sources(db, rf_item).await.map_err(ImportError::Db)?;
                vacant.insert(found)
            }
        };
        let Some(source) = tracks::choose_source(sources, translated.as_deref()) else {
            return Ok(None);
        };
        let streams = self.streams.get(&jf_item).map(Vec::as_slice).unwrap_or(&[]);
        let Some(index) = tracks::map_track(choice, kind, streams, &source.streams) else {
            return Ok(None);
        };
        Ok(source
            .streams
            .iter()
            .find(|row| row.index == index)
            .map(|row| (index, row.identity().key())))
    }
}

/// The Users stage of a run (spec 2.7, 2.26): each row to create is matched again against
/// Rustyfin's users and the users created earlier in this run. An unmatched row gets a new user
/// with an unusable password and a one-time code, which the report shows once.
async fn create_users(
    db: &SqlitePool,
    users: &mut [UserRow],
    mut known: Vec<(Uuid, String)>,
    handle: &JobHandle,
) -> Result<(), ImportError> {
    let to_create: Vec<usize> = users
        .iter()
        .enumerate()
        .filter(|(_, user)| user.action == UserAction::Create)
        .map(|(index, _)| index)
        .collect();
    handle.stage(ImportStage::Users, to_create.len());
    for (done, index) in to_create.into_iter().enumerate() {
        let row = &mut users[index];
        match plan::match_user_name(&row.name, &known) {
            plan::UserMatch::Matched(id) => {
                row.action = UserAction::Match;
                row.user_id = Some(id);
            }
            plan::UserMatch::Ambiguous => {
                row.action = UserAction::Skip;
                row.reason = Some(SkipReason::Ambiguous);
            }
            plan::UserMatch::NoMatch => {
                let user = User {
                    id: Uuid::new_v4(),
                    name: row.name.clone(),
                    password_hash: unusable_password_hash()
                        .await
                        .map_err(|_| ImportError::Hash)?,
                    is_administrator: false,
                    created_at: Utc::now(),
                    last_login_date: None,
                    configuration: serde_json::Value::Object(Default::default()),
                };
                let code = generate_setup_code();
                let code_hash = hash_setup_code(&code)
                    .await
                    .map_err(|_| ImportError::Hash)?;
                rustyfin_db::repo_setup_code::create_user_with_code(db, &user, &code_hash)
                    .await
                    .map_err(ImportError::Db)?;
                known.push((user.id, user.name.clone()));
                row.user_id = Some(user.id);
                row.code = Some(code);
            }
        }
        handle.progress(done + 1);
    }
    Ok(())
}

/// Runs a prepared import in the background (spec 2.18-2.21, 2.26): creates the planned users and
/// reports through `handle`. `on_written` runs when metadata was written (spec 5.10), before the
/// report is completed. One INFO line on success; a WARN line and the error on failure.
pub async fn run(
    db: SqlitePool,
    req: ImportRequest,
    prepared: Prepared,
    handle: JobHandle,
    data_dir: PathBuf,
    on_written: impl FnOnce() + Send + 'static,
) {
    match plan_report(&db, &req, &prepared, &data_dir, Some(&handle)).await {
        Ok(report) => {
            let layout = report.layout;
            let source = &req.source_path;
            let user_count = report.users.len();
            let created = report
                .users
                .iter()
                .filter(|user| user.code.is_some())
                .count();
            let total = report.items.total;
            let (by_path, by_id, unmatched) = (
                report.items.matched_by_path,
                report.items.matched_by_id,
                report.items.unmatched,
            );
            let (rows, imported, unchanged, watch_unmatched) = (
                report.watch_data.rows,
                report.watch_data.imported,
                report.watch_data.unchanged,
                report.watch_data.unmatched,
            );
            let (mode, items, fields, people, images, missing, locked) = (
                report.metadata.mode,
                report.metadata.items,
                report.metadata.fields,
                report.metadata.people,
                report.metadata.images,
                report.metadata.images_missing,
                report.metadata.locked,
            );
            tracing::info!(
                "jellyfin import run: source {source}, layout {layout:?}, {user_count} users, {created} created, {total} items ({by_path} by path, {by_id} by id, {unmatched} unmatched), {rows} watch rows ({imported} imported, {unchanged} unchanged, {watch_unmatched} unmatched), metadata {mode:?} ({items} items, {fields} fields, {people} people, {images} images, {missing} missing, {locked} locked)"
            );
            if report.metadata.written {
                on_written();
            }
            handle.complete(report);
        }
        Err(error) => {
            tracing::warn!("jellyfin import run failed: {error}");
            handle.fail(error.to_string());
        }
    }
}
