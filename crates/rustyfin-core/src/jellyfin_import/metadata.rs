//! Metadata of the matched items (spec 5.1 to 5.10): plans what an import writes and, for a run,
//! copies the image files and then writes the rows in one transaction.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rustyfin_db::repo_jellyfin_import::{
    ExtrasWrite, ImageRow, ImageWrite, ImportTarget, ItemMetadata, ItemWrite, LinkWrite,
    MetadataState, MetadataWrites, NewPerson, PersonLink, StoredImage, StoredLink, StudioWrite,
    metadata_state, write_metadata,
};
use rustyfin_model::image::ImageType;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use super::ImportError;
use super::images::{self, UsableImage};
use super::job::{ImportStage, JobHandle};
use super::paths::PathMapping;
use super::plan;
use super::source::{JfCredit, JfImage, JfItem, JfItemMeta, Snapshot, parse_jellyfin_date};

/// How a run treats the metadata Rustyfin already has (spec 5.1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MetadataMode {
    /// Writes a value only where Rustyfin's is empty.
    #[default]
    FillEmpty,
    /// Writes every value Jellyfin has that differs from Rustyfin's.
    Replace,
    /// Imports no metadata; every count is 0.
    None,
}

/// The metadata part of an import report (spec 5.2). A preview reports what a run would write.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MetadataReport {
    pub mode: MetadataMode,
    /// Matched items with any change: a field, a person or an image.
    pub items: usize,
    /// Changed field values: each row of spec 5.4 counts 1 per item, each provider id 1.
    pub fields: usize,
    /// `item_people` rows written.
    pub people: usize,
    /// Image rows written: files copied, for items and people.
    pub images: usize,
    /// Jellyfin images of a known type not copied: missing, unusable, or failed to copy.
    pub images_missing: usize,
    /// Matched items skipped because `lock_data` is set.
    pub locked: usize,
    /// Whether the run wrote anything. Not reported.
    #[serde(skip)]
    pub written: bool,
}

/// What `import` reads: the mode, the source snapshot, the matched items and the paths to images.
pub struct MetadataInput<'a> {
    pub mode: MetadataMode,
    pub snapshot: &'a Snapshot,
    /// Jellyfin item id to Rustyfin item id (`plan::matched_items`).
    pub matched: &'a HashMap<Uuid, Uuid>,
    /// The Jellyfin items matched by path (`plan::matched_by_path`), which a matched item's
    /// metadata comes from first (spec 5.15).
    pub by_path: &'a HashSet<Uuid>,
    pub pairs: &'a [PathMapping],
    pub program_data: &'a Path,
    pub data_dir: &'a Path,
}

/// Plans the metadata of the matched items (spec 5.1 to 5.9). With a `handle` (a run) it copies the
/// planned image files and writes the rows in one transaction, then rebuilds the filter indexes once
/// (spec 5.10). A preview (no handle) only plans: it copies nothing, writes nothing and creates no
/// folder.
pub async fn import(
    db: &SqlitePool,
    input: &MetadataInput<'_>,
    handle: Option<&JobHandle>,
) -> Result<MetadataReport, ImportError> {
    let mut report = MetadataReport {
        mode: input.mode,
        ..MetadataReport::default()
    };
    if input.mode == MetadataMode::None {
        return Ok(report);
    }
    let state = metadata_state(db).await.map_err(ImportError::Db)?;
    let mut plan = plan_writes(input, &state);
    report.fields = plan.fields;
    report.people = plan.people;
    report.locked = plan.locked;
    report.images_missing = plan.missing;
    if let Some(handle) = handle {
        handle.stage(ImportStage::Metadata, plan.copies.len());
        let mut failed = HashSet::new();
        for (done, file) in plan.copies.iter().enumerate() {
            if images::copy(&file.source, &file.dest).is_err() {
                failed.insert((file.group, file.row));
            }
            handle.progress(done + 1);
        }
        report.images_missing += failed.len();
        drop_failed(&mut plan.writes, &failed);
    }
    report.items = touched_items(&plan.writes);
    report.images = plan
        .writes
        .images
        .iter()
        .map(|group| group.rows.len())
        .sum();
    if handle.is_some() && !plan.writes.is_empty() {
        write_metadata(db, &plan.writes)
            .await
            .map_err(ImportError::Db)?;
        rustyfin_db::repo_media::rebuild_filter_indexes(db)
            .await
            .map_err(ImportError::Db)?;
        report.written = true;
    }
    Ok(report)
}

/// The targets the matching uses (spec 5.14): the stored ones, plus the TMDB and TVDB ids this
/// run's metadata step would write, since those ids decide what the next run matches.
pub async fn targets_after(
    db: &SqlitePool,
    mode: MetadataMode,
    snapshot: &Snapshot,
    targets: &[ImportTarget],
    pairs: &[PathMapping],
) -> Result<Vec<ImportTarget>, ImportError> {
    if mode == MetadataMode::None {
        return Ok(targets.to_vec());
    }
    let state = metadata_state(db).await.map_err(ImportError::Db)?;
    let rows: HashMap<Uuid, &ItemMetadata> = state
        .items
        .iter()
        .filter_map(|row| Some((Uuid::parse_str(&row.id).ok()?, row)))
        .collect();
    let position: HashMap<Uuid, usize> = targets
        .iter()
        .enumerate()
        .map(|(n, target)| (target.id, n))
        .collect();
    // Path matching does not depend on the ids, so it is the same in every round.
    let by_path = plan::matched_by_path(&snapshot.items, targets, pairs);
    let mut current = targets.to_vec();
    let mut matched = plan::matched_items(&snapshot.items, &current, pairs);
    for _ in 0..4 {
        let mut next = targets.to_vec();
        for (jf, id) in sources(&snapshot.items, &matched, &by_path) {
            let (Some(&row), Some(&n)) = (rows.get(&id), position.get(&id)) else {
                continue;
            };
            if row.lock_data {
                continue;
            }
            let (tmdb, tvdb) = provider_writes(jf, row);
            if tmdb.is_some() {
                next[n].tmdb_id = tmdb;
            }
            if tvdb.is_some() {
                next[n].tvdb_id = tvdb;
            }
        }
        let next_matched = plan::matched_items(&snapshot.items, &next, pairs);
        if next_matched == matched {
            return Ok(next);
        }
        current = next;
        matched = next_matched;
    }
    Ok(current)
}

/// What a run plans (spec 5.10): the rows to write, the report counts, and the image files to copy.
#[derive(Default)]
struct Plan {
    writes: MetadataWrites,
    fields: usize,
    people: usize,
    locked: usize,
    missing: usize,
    copies: Vec<FileCopy>,
}

/// An image file to copy, and the `writes.images` group and row it belongs to.
struct FileCopy {
    group: usize,
    row: usize,
    source: UsableImage,
    dest: PathBuf,
}

/// The Jellyfin item each matched Rustyfin id takes its metadata from (spec 5.15): the first
/// of its items matched by path, else the first of its items. Returned with the id, in item order.
fn sources<'a>(
    items: &'a [JfItem],
    matched: &HashMap<Uuid, Uuid>,
    by_path: &HashSet<Uuid>,
) -> Vec<(&'a JfItem, Uuid)> {
    // Each Rustyfin id's source position: its first item, moved to its first item by path.
    let mut chosen: HashMap<Uuid, usize> = HashMap::new();
    for (n, jf) in items.iter().enumerate() {
        let Some(&id) = matched.get(&jf.id) else {
            continue;
        };
        let slot = chosen.entry(id).or_insert(n);
        if by_path.contains(&jf.id) && !by_path.contains(&items[*slot].id) {
            *slot = n;
        }
    }
    let mut picked: Vec<(usize, Uuid)> = chosen.into_iter().map(|(id, n)| (n, id)).collect();
    picked.sort_unstable();
    picked.into_iter().map(|(n, id)| (&items[n], id)).collect()
}

/// Plans every write of the import from the Rustyfin rows and the snapshot (spec 5.1 to 5.9). It
/// reads files only through `images::check`.
fn plan_writes(input: &MetadataInput<'_>, state: &MetadataState) -> Plan {
    let mut plan = Plan::default();
    let rows: HashMap<Uuid, &ItemMetadata> = state
        .items
        .iter()
        .filter_map(|row| Some((Uuid::parse_str(&row.id).ok()?, row)))
        .collect();
    let mut studios_of: HashMap<&str, Vec<String>> = HashMap::new();
    for (item, studio) in &state.studios {
        studios_of
            .entry(item.as_str())
            .or_default()
            .push(studio.clone());
    }
    let mut links_of: HashMap<&str, Vec<&StoredLink>> = HashMap::new();
    for link in &state.links {
        links_of
            .entry(link.item_id.as_str())
            .or_default()
            .push(link);
    }
    let mut item_images_of: HashMap<&str, Vec<&StoredImage>> = HashMap::new();
    for image in &state.item_images {
        item_images_of
            .entry(image.owner_id.as_str())
            .or_default()
            .push(image);
    }
    let mut person_images_of: HashMap<&str, Vec<&StoredImage>> = HashMap::new();
    for image in &state.person_images {
        person_images_of
            .entry(image.owner_id.as_str())
            .or_default()
            .push(image);
    }
    // The people are in id order, so the first row of a name is its lowest id.
    let mut person_ids: HashMap<String, String> = HashMap::new();
    for (id, name) in &state.people {
        person_ids.entry(name.clone()).or_insert_with(|| id.clone());
    }
    let empty = JfItemMeta::default();
    let mut person_names: Vec<Arc<str>> = Vec::new();
    for (jf, id) in sources(&input.snapshot.items, input.matched, input.by_path) {
        let Some(&row) = rows.get(&id) else {
            continue;
        };
        if row.lock_data {
            plan.locked += 1;
            continue;
        }
        let locked: HashSet<&str> = row
            .locked_fields
            .split('|')
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .collect();
        let unlocked = |field: &str| !locked.contains(field);
        let meta = input.snapshot.metadata.items.get(&jf.id).unwrap_or(&empty);
        let studios: &[String] = studios_of
            .get(row.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let fields = field_writes(input.mode, id, jf, meta, row, studios, &locked);
        plan.fields += fields.changed;
        plan.writes.items.extend(fields.item);
        plan.writes.extras.extend(fields.extras);
        plan.writes.studios.extend(fields.studios);

        let planned = planned_credits(
            input
                .snapshot
                .metadata
                .credits
                .get(&jf.id)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
        );
        let cast_open = unlocked("Cast");
        if cast_open {
            person_names.extend(planned.iter().map(|link| Arc::clone(&link.name)));
        }
        let stored: &[&StoredLink] = links_of
            .get(row.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let links_changed = cast_open
            && !planned.is_empty()
            && match input.mode {
                MetadataMode::FillEmpty => stored.is_empty(),
                MetadataMode::Replace => !same_links(stored, &planned),
                MetadataMode::None => false,
            };
        if links_changed {
            let mut links = Vec::with_capacity(planned.len());
            for (position, link) in planned.iter().enumerate() {
                links.push(PersonLink {
                    person_id: person_for(&mut person_ids, &mut plan.writes.new_people, link),
                    kind: link.kind.clone(),
                    role: link.role.clone(),
                    sort_order: position as i64,
                });
            }
            plan.people += links.len();
            plan.writes.links.push(LinkWrite { item_id: id, links });
        }

        let item_images: &[&StoredImage] = item_images_of
            .get(row.id.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        plan_images(&mut plan, input, &meta.images, id, false, item_images);
    }

    // Person images for the names that exist after this run's link writes, each name once (5.9).
    let mut done: HashSet<&str> = HashSet::new();
    for name in &person_names {
        let name: &str = name;
        if !done.insert(name) {
            continue;
        }
        let Some(raw) = person_ids.get(name) else {
            continue;
        };
        let Ok(owner) = Uuid::parse_str(raw) else {
            continue;
        };
        let Some(images) = input.snapshot.metadata.person_images.get(name) else {
            continue;
        };
        let stored: &[&StoredImage] = person_images_of
            .get(raw.as_str())
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        plan_images(&mut plan, input, images, owner, true, stored);
    }
    plan
}

/// The field writes of one matched item (spec 5.4): the `media_items` UPDATE, the extras row and the
/// studio rows, each only when a value changes, and how many values change.
struct FieldWrites {
    item: Option<ItemWrite>,
    extras: Option<ExtrasWrite>,
    studios: Option<StudioWrite>,
    changed: usize,
}

/// Plans the fields of one item (spec 5.4, 5.5). `studios` are the item's studio ids now.
fn field_writes(
    mode: MetadataMode,
    id: Uuid,
    jf: &JfItem,
    meta: &JfItemMeta,
    row: &ItemMetadata,
    studios: &[String],
    locked: &HashSet<&str>,
) -> FieldWrites {
    let unlocked = |field: &str| !locked.contains(field);

    let name = wanted(
        mode,
        unlocked("Name"),
        text(jf.name.as_deref()),
        Some(&row.name),
        blank(Some(row.name.as_str())),
    );
    // The sort name is lowercased; Replace compares the lowercased values.
    let forced = text(meta.forced_sort_name.as_deref()).map(|sort| sort.to_lowercase());
    let sort_now = row
        .sort_name
        .as_deref()
        .map(|sort| sort.trim().to_lowercase());
    let sort_jf = wanted(
        mode,
        true,
        forced.clone(),
        sort_now.as_ref(),
        blank(row.sort_name.as_deref()),
    );
    // Not counted: a written name also moves a sort name that tracked the old name, when Jellyfin
    // has no forced sort name (spec 5.4).
    let old_lower = row.name.to_lowercase();
    let sort_auto = match &name {
        Some(new)
            if forced.is_none()
                && (row.sort_name.is_none()
                    || row.sort_name.as_deref() == Some(old_lower.as_str())) =>
        {
            Some(new.to_lowercase())
        }
        _ => None,
    };
    let overview = wanted(
        mode,
        unlocked("Overview"),
        text(meta.overview.as_deref()),
        row.overview.as_ref(),
        blank(row.overview.as_deref()),
    );
    let tagline = wanted(
        mode,
        true,
        text(meta.tagline.as_deref()),
        row.tagline.as_ref(),
        blank(row.tagline.as_deref()),
    );
    let official_rating = wanted(
        mode,
        unlocked("OfficialRating"),
        text(meta.official_rating.as_deref()),
        row.official_rating.as_ref(),
        blank(row.official_rating.as_deref()),
    );
    let community_rating = wanted(
        mode,
        true,
        meta.community_rating.filter(|rating| rating.is_finite()),
        row.community_rating.as_ref(),
        row.community_rating.is_none(),
    );
    let premiere_date = wanted(
        mode,
        true,
        date(meta.premiere_date.as_deref()),
        row.premiere_date.as_ref(),
        blank(row.premiere_date.as_deref()),
    );
    let end_date = wanted(
        mode,
        true,
        date(meta.end_date.as_deref()),
        row.end_date.as_ref(),
        blank(row.end_date.as_deref()),
    );
    let production_year = wanted(
        mode,
        true,
        meta.production_year
            .filter(|year| (1_i64..=9999).contains(year)),
        row.production_year.as_ref(),
        row.production_year.is_none(),
    );
    let genres_now = stored_list(&row.genres);
    let genres = wanted(
        mode,
        unlocked("Genres"),
        jellyfin_list(meta.genres.as_deref()),
        Some(&genres_now),
        genres_now.is_empty(),
    );
    let tags_now = stored_list(&row.tags);
    let tags = wanted(
        mode,
        unlocked("Tags"),
        jellyfin_list(meta.tags.as_deref()),
        Some(&tags_now),
        tags_now.is_empty(),
    );
    let (tmdb, tvdb) = provider_writes(jf, row);
    let original_title = wanted(
        mode,
        true,
        text(meta.original_title.as_deref()),
        row.original_title.as_ref(),
        blank(row.original_title.as_deref()),
    );
    let critic_rating = wanted(
        mode,
        true,
        meta.critic_rating.filter(|rating| rating.is_finite()),
        row.critic_rating.as_ref(),
        row.critic_rating.is_none(),
    );
    let imdb = wanted(
        mode,
        true,
        text(jf.providers.imdb.as_deref()),
        row.imdb_id.as_ref(),
        blank(row.imdb_id.as_deref()),
    );

    let incoming = studio_rows(meta.studios.as_deref());
    let studio_change = unlocked("Studios")
        && !incoming.is_empty()
        && match mode {
            MetadataMode::FillEmpty => studios.is_empty(),
            MetadataMode::Replace => {
                let incoming_ids: HashSet<&str> = incoming
                    .iter()
                    .map(|(studio_id, _)| studio_id.as_str())
                    .collect();
                let current_ids: HashSet<&str> = studios.iter().map(String::as_str).collect();
                incoming_ids != current_ids
            }
            MetadataMode::None => false,
        };

    let item_fields = counted(&name)
        + counted(&sort_jf)
        + counted(&overview)
        + counted(&tagline)
        + counted(&official_rating)
        + counted(&community_rating)
        + counted(&premiere_date)
        + counted(&end_date)
        + counted(&production_year)
        + counted(&genres)
        + counted(&tags)
        + counted(&tmdb)
        + counted(&tvdb);
    let extras_fields = counted(&original_title) + counted(&critic_rating) + counted(&imdb);
    let item = (item_fields > 0 || sort_auto.is_some()).then(|| ItemWrite {
        id,
        name: name.unwrap_or_else(|| row.name.clone()),
        sort_name: sort_jf.or(sort_auto).or_else(|| row.sort_name.clone()),
        overview: overview.or_else(|| row.overview.clone()),
        tagline: tagline.or_else(|| row.tagline.clone()),
        official_rating: official_rating.or_else(|| row.official_rating.clone()),
        community_rating: community_rating.or(row.community_rating),
        premiere_date: premiere_date.or_else(|| row.premiere_date.clone()),
        end_date: end_date.or_else(|| row.end_date.clone()),
        production_year: production_year.or(row.production_year),
        genres: genres.map_or_else(|| row.genres.clone(), |list| list.join("|")),
        tags: tags.map_or_else(|| row.tags.clone(), |list| list.join("|")),
        tmdb_id: tmdb.or(row.tmdb_id),
        tvdb_id: tvdb.or(row.tvdb_id),
    });
    let extras = (extras_fields > 0).then(|| ExtrasWrite {
        item_id: id,
        original_title: original_title.or_else(|| row.original_title.clone()),
        critic_rating: critic_rating.or(row.critic_rating),
        imdb_id: imdb.or_else(|| row.imdb_id.clone()),
    });
    let studio_write = studio_change.then_some(StudioWrite {
        item_id: id,
        studios: incoming,
    });
    FieldWrites {
        item,
        extras,
        studios: studio_write,
        changed: item_fields + extras_fields + usize::from(studio_change),
    }
}

/// The TMDB and TVDB ids a matched item gets (spec 5.15). They only fill an empty field, in both
/// modes, because they decide what matches (B20 fix 3). `targets_after` makes the same calls, so
/// the matching of the next run sees the ids this run writes.
fn provider_writes(jf: &JfItem, row: &ItemMetadata) -> (Option<i64>, Option<i64>) {
    let tmdb = wanted(
        MetadataMode::FillEmpty,
        true,
        jf.providers.tmdb,
        row.tmdb_id.as_ref(),
        row.tmdb_id.is_none(),
    );
    let tvdb = wanted(
        MetadataMode::FillEmpty,
        true,
        jf.providers.tvdb,
        row.tvdb_id.as_ref(),
        row.tvdb_id.is_none(),
    );
    (tmdb, tvdb)
}

/// Whether a Jellyfin value is written over Rustyfin's `current` (spec 5.1): `FillEmpty` only when
/// `empty`, `Replace` when it differs. A value Jellyfin lacks, and a locked field, are never written.
fn wanted<T: PartialEq>(
    mode: MetadataMode,
    unlocked: bool,
    incoming: Option<T>,
    current: Option<&T>,
    empty: bool,
) -> Option<T> {
    let incoming = incoming.filter(|_| unlocked)?;
    match mode {
        MetadataMode::FillEmpty if empty => Some(incoming),
        MetadataMode::Replace if current != Some(&incoming) => Some(incoming),
        _ => None,
    }
}

/// 1 when a value is written, else 0.
fn counted<T>(value: &Option<T>) -> usize {
    usize::from(value.is_some())
}

/// Trimmed text, or None when it is empty after trimming (spec 5.1).
fn text(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .map(str::to_string)
}

/// Whether a stored or Jellyfin text is missing or empty after trimming.
fn blank(value: Option<&str>) -> bool {
    text(value).is_none()
}

/// A Jellyfin date as Rustyfin stores it (spec 5.4): RFC 3339, or None when it doesn't parse.
fn date(value: Option<&str>) -> Option<String> {
    text(value)
        .and_then(|value| parse_jellyfin_date(&value))
        .map(|parsed| parsed.to_rfc3339())
}

/// A pipe-joined list: entries trimmed, empty and repeated entries dropped, order kept (spec 5.4).
fn list(value: &str) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for entry in value
        .split('|')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
    {
        if !entries.iter().any(|seen| seen == entry) {
            entries.push(entry.to_string());
        }
    }
    entries
}

/// A Rustyfin list column, where `[]` (the old default) and "" are empty (spec 5.1).
fn stored_list(value: &str) -> Vec<String> {
    if value.trim() == "[]" {
        Vec::new()
    } else {
        list(value)
    }
}

/// A Jellyfin pipe-joined list, or None when it has no entries (spec 5.4).
fn jellyfin_list(value: Option<&str>) -> Option<Vec<String>> {
    let entries = list(value.unwrap_or_default());
    (!entries.is_empty()).then_some(entries)
}

/// The studios of a pipe-joined Jellyfin list as (studio id, name), one row per id (spec 5.4).
fn studio_rows(value: Option<&str>) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = Vec::new();
    for name in list(value.unwrap_or_default()) {
        let studio_id = format!("studio-{}", name.to_lowercase().replace(' ', "-"));
        if !rows.iter().any(|(seen, _)| *seen == studio_id) {
            rows.push((studio_id, name));
        }
    }
    rows
}

/// A person of an item, planned (spec 5.7).
pub(super) struct PlannedLink {
    name: Arc<str>,
    kind: String,
    role: Option<String>,
}

/// The people of an item in DTO order (spec 5.7): ListOrder, then SortOrder with NULLs last, then
/// source order. Names are trimmed; empty and repeated names are dropped; kind and role are trimmed.
pub(super) fn planned_credits(credits: &[JfCredit]) -> Vec<PlannedLink> {
    let mut sorted: Vec<&JfCredit> = credits.iter().collect();
    sorted.sort_by(|a, b| {
        nulls_last(a.list_order, b.list_order).then_with(|| nulls_last(a.sort_order, b.sort_order))
    });
    let mut planned: Vec<PlannedLink> = Vec::new();
    // Names already planned: a repeat is skipped in O(1) instead of scanning `planned`.
    let mut seen: HashSet<&str> = HashSet::new();
    for credit in sorted {
        let trimmed = credit.name.trim();
        if trimmed.is_empty() || !seen.insert(trimmed) {
            continue;
        }
        // Credits of one person share a name; only a name that trimming changed is copied.
        let name = if trimmed.len() == credit.name.len() {
            Arc::clone(&credit.name)
        } else {
            Arc::from(trimmed)
        };
        planned.push(PlannedLink {
            name,
            kind: text(credit.kind.as_deref()).unwrap_or_else(|| "Unknown".to_string()),
            role: text(credit.role.as_deref()),
        });
    }
    planned
}

/// Orders missing values last (spec 5.7).
fn nulls_last(a: Option<i64>, b: Option<i64>) -> Ordering {
    a.is_none().cmp(&b.is_none()).then_with(|| a.cmp(&b))
}

/// Whether an item's stored links are the planned ones, in order (spec 5.7).
fn same_links(stored: &[&StoredLink], planned: &[PlannedLink]) -> bool {
    stored.len() == planned.len()
        && stored
            .iter()
            .zip(planned)
            .enumerate()
            .all(|(position, (link, planned_link))| {
                link.name.as_str() == planned_link.name.as_ref()
                    && link.kind == planned_link.kind
                    && link.role == planned_link.role
                    && link.sort_order == Some(position as i64)
            })
}

/// The id of a planned person: the lowest Rustyfin id of the exact name, else a person this run
/// creates (spec 5.7). A created person is written only together with its link.
fn person_for(
    ids: &mut HashMap<String, String>,
    new_people: &mut Vec<NewPerson>,
    link: &PlannedLink,
) -> String {
    if let Some(id) = ids.get(&*link.name) {
        return id.clone();
    }
    let id = Uuid::new_v4();
    ids.insert(link.name.to_string(), id.to_string());
    new_people.push(NewPerson {
        id,
        name: link.name.to_string(),
        kind: link.kind.clone(),
    });
    id.to_string()
}

/// Plans the image rows of one owner, an item or a person (spec 5.8, 5.9), grouped by image type in
/// first-seen order. `stored` are the owner's rows of every type.
fn plan_images(
    plan: &mut Plan,
    input: &MetadataInput<'_>,
    images: &[JfImage],
    owner: Uuid,
    is_person: bool,
    stored: &[&StoredImage],
) {
    let folder = if is_person {
        input
            .data_dir
            .join("images")
            .join("people")
            .join(owner.to_string())
    } else {
        input.data_dir.join("images").join(owner.to_string())
    };
    let mut groups: Vec<(ImageType, Vec<&JfImage>)> = Vec::new();
    for image in images {
        match groups
            .iter_mut()
            .find(|(kind, _)| *kind == image.image_type)
        {
            Some((_, group)) => group.push(image),
            None => groups.push((image.image_type, vec![image])),
        }
    }
    for (kind, group) in groups {
        let kind_name = kind.as_str();
        let current: Vec<&StoredImage> = stored
            .iter()
            .copied()
            .filter(|row| row.image_type == kind_name)
            .collect();
        if input.mode == MetadataMode::FillEmpty && !current.is_empty() {
            continue;
        }
        let mut rows: Vec<ImageRow> = Vec::new();
        let mut sources: Vec<(UsableImage, PathBuf)> = Vec::new();
        for image in group {
            let usable = images::resolve(&image.path, input.program_data, input.pairs)
                .and_then(|(file, root)| images::check(&file, &root));
            let Some(usable) = usable else {
                plan.missing += 1;
                continue;
            };
            let index = rows.len();
            let dest = folder.join(images::copy_name(kind, index, &usable));
            rows.push(ImageRow {
                index: index as i64,
                path: dest.display().to_string(),
                width: image.width.map(i64::from),
                height: image.height.map(i64::from),
                blurhash: text(image.blurhash.as_deref()),
            });
            sources.push((usable, dest));
        }
        if rows.is_empty() {
            continue;
        }
        if input.mode == MetadataMode::Replace {
            let mut stored_rows: Vec<ImageRow> = current
                .iter()
                .map(|row| ImageRow {
                    index: row.image_index,
                    path: row.path.clone(),
                    width: row.width,
                    height: row.height,
                    blurhash: row.blurhash.clone(),
                })
                .collect();
            stored_rows.sort_by_key(|row| row.index);
            if stored_rows == rows {
                continue;
            }
        }
        let group_index = plan.writes.images.len();
        for (row, (source, dest)) in sources.into_iter().enumerate() {
            plan.copies.push(FileCopy {
                group: group_index,
                row,
                source,
                dest,
            });
        }
        plan.writes.images.push(ImageWrite {
            owner_id: owner,
            is_person,
            image_type: kind_name.to_string(),
            rows,
        });
    }
}

/// Drops the rows of the copies that failed, and the groups left empty, so a failed copy gets no row
/// (spec 5.9). `failed` holds the (group, row) positions in `writes.images`.
fn drop_failed(writes: &mut MetadataWrites, failed: &HashSet<(usize, usize)>) {
    for (group, images) in writes.images.iter_mut().enumerate() {
        let mut position = 0;
        images.rows.retain(|_| {
            let keep = !failed.contains(&(group, position));
            position += 1;
            keep
        });
    }
    writes.images.retain(|images| !images.rows.is_empty());
}

/// The matched items with a field, people or item-image write (spec 5.2). Person images don't count.
fn touched_items(writes: &MetadataWrites) -> usize {
    let mut items: HashSet<Uuid> = HashSet::new();
    items.extend(writes.items.iter().map(|write| write.id));
    items.extend(writes.extras.iter().map(|write| write.item_id));
    items.extend(writes.studios.iter().map(|write| write.item_id));
    items.extend(writes.links.iter().map(|write| write.item_id));
    items.extend(
        writes
            .images
            .iter()
            .filter(|write| !write.is_person)
            .map(|write| write.owner_id),
    );
    items.len()
}
