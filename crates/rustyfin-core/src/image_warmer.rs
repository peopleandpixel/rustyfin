//! Prepares the grid's pictures in the background, so that scrolling a library never waits on a
//! resize: after each scan or metadata refresh (and once at startup), every item's Primary image
//! gets its grid master (`image_proxy::GRID_MASTER`, the copy smaller grid copies are made from)
//! and its BlurHash, where it has none yet. Remote originals are kept in the data dir as the
//! image route does on first use.
//!
//! It renders in the background lane (`image_proxy::RenderFor::Background`): one picture at a
//! time, only while no client render runs, and never queued for memory ahead of a client.
//!
//! Masters are written only while the whole disk image cache stays under `WARM_SHARE` of its cap,
//! counting the copies already there, so a pass never pushes the cache to its sweep and never
//! evicts what clients (or an earlier pass) stored. A library too large for that warms its top
//! part; its other pictures still get a BlurHash when their original is on disk already.
//! A picture that fails is not tried again until the server restarts.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex, OnceLock};

use rustyfin_model::image::ItemImage;
use sqlx::SqlitePool;

use crate::image_proxy;

/// The share of the disk image cache's cap that masters may fill it to: half.
pub const WARM_SHARE: (u64, u64) = (1, 2);

/// How long a pass waits after a kick, so that a burst of kicks makes one pass.
const SETTLE: std::time::Duration = std::time::Duration::from_secs(2);

static KICKS: OnceLock<tokio::sync::Notify> = OnceLock::new();

/// Pictures (path and stamp) that failed in this process, so a broken or missing remote picture
/// is not fetched again on every scan.
type PictureKey = (String, Option<i64>);
static FAILED: LazyLock<Mutex<HashSet<PictureKey>>> = LazyLock::new(Default::default);

fn failed_before(img: &ItemImage) -> bool {
    FAILED
        .lock()
        .is_ok_and(|s| s.contains(&(img.path.clone(), img.stamp)))
}

fn note_failed(img: &ItemImage) {
    if let Ok(mut s) = FAILED.lock() {
        s.insert((img.path.clone(), img.stamp));
    }
}

/// What a pass did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct WarmReport {
    /// Grid masters written.
    pub prepared: u32,
    /// BlurHashes stored.
    pub hashed: u32,
    /// Pictures that had what they could get already.
    pub ready: u32,
    /// Pictures that could not be read or decoded.
    pub failed: u32,
    /// Pictures skipped because they failed in an earlier pass.
    pub skipped: u32,
    /// Bytes of masters written.
    pub bytes: u64,
    /// Whether masters stopped at the cache share.
    pub capped: bool,
}

/// Starts the warmer, which runs a pass now and after every `kick`. Later calls do nothing.
pub fn start(pool: SqlitePool, data_dir: PathBuf, http_client: reqwest::Client) {
    if KICKS.set(tokio::sync::Notify::new()).is_err() {
        return;
    }
    kick();
    tokio::spawn(async move {
        let kicks = KICKS.get().expect("set above");
        loop {
            kicks.notified().await;
            tokio::time::sleep(SETTLE).await;
            let started = std::time::Instant::now();
            match warm_pass(&pool, &data_dir, &http_client).await {
                Ok(r) if r.prepared > 0 || r.hashed > 0 || r.failed > 0 => tracing::info!(
                    "prepared grid images: {} copies ({} KiB), {} BlurHashes, {} already ready, \
                     {} failed, {} failed before{} in {:.1} s",
                    r.prepared,
                    r.bytes / 1024,
                    r.hashed,
                    r.ready,
                    r.failed,
                    r.skipped,
                    if r.capped {
                        ", copies stopped at the cache's share,"
                    } else {
                        ""
                    },
                    started.elapsed().as_secs_f64()
                ),
                Ok(_) => {}
                Err(e) => tracing::warn!("failed to prepare grid images: {}", e),
            }
        }
    });
}

/// Asks the warmer for a pass, after the one running if any. Does nothing before `start`.
pub fn kick() {
    if let Some(kicks) = KICKS.get() {
        kicks.notify_one();
    }
}

/// One pass over every item's Primary image, the top of each library first, within the disk
/// image cache's cap.
pub async fn warm_pass(
    pool: &SqlitePool,
    data_dir: &Path,
    http_client: &reqwest::Client,
) -> Result<WarmReport, sqlx::Error> {
    warm_pass_within(
        pool,
        data_dir,
        http_client,
        image_proxy::IMAGE_CACHE_MAX_BYTES,
    )
    .await
}

/// `warm_pass` for a cache whose cap is `cache_cap` bytes.
pub async fn warm_pass_within(
    pool: &SqlitePool,
    data_dir: &Path,
    http_client: &reqwest::Client,
    cache_cap: u64,
) -> Result<WarmReport, sqlx::Error> {
    let images = rustyfin_db::repo_image::list_primary_images(pool).await?;
    let cache_dir = data_dir.join("cache").join("images");
    let store = data_dir.join("metadata").join("remote-images");
    let dir = cache_dir.clone();
    let in_cache = tokio::task::spawn_blocking(move || folder_bytes(&dir))
        .await
        .unwrap_or(0);
    let mut budget = warm_budget(in_cache, cache_cap);
    let mut report = WarmReport::default();
    for img in images {
        if failed_before(&img) {
            report.skipped += 1;
            continue;
        }
        match warm_one(pool, &img, &cache_dir, &store, http_client, budget).await {
            Ok(Warmed {
                written,
                hashed,
                capped,
            }) => {
                if written > 0 {
                    report.prepared += 1;
                    report.bytes += written;
                    budget = budget.saturating_sub(written);
                }
                report.capped |= capped;
                if hashed {
                    report.hashed += 1;
                }
                if written == 0 && !hashed {
                    report.ready += 1;
                }
            }
            Err(e) => {
                report.failed += 1;
                note_failed(&img);
                tracing::debug!(path = %img.path, "could not prepare grid image: {}", e);
            }
        }
    }
    Ok(report)
}

/// The bytes of masters a pass may write when the cache holds `in_cache` bytes: up to
/// `WARM_SHARE` of the cap, all copies counted.
pub fn warm_budget(in_cache: u64, cache_cap: u64) -> u64 {
    (cache_cap / WARM_SHARE.1 * WARM_SHARE.0).saturating_sub(in_cache)
}

/// The total size of the files in `dir` (0 when it can't be read).
fn folder_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

/// What `warm_one` did.
struct Warmed {
    /// Bytes of master written (0 when it was there, or there was no room).
    written: u64,
    /// Whether a BlurHash was stored.
    hashed: bool,
    /// Whether the master was missing and the budget had no room for it.
    capped: bool,
}

/// The grid copies made from the master after it, so a landing is served from disk without a
/// render, at the size asked (see `image_proxy::GRID_COPY_WIDTHS`).
pub use image_proxy::GRID_COPY_WIDTHS;

/// Makes the `GRID_COPY_WIDTHS` copies of a picture that are not on disk, from its grid master,
/// while `budget` has room. Returns the bytes written. A copy that fails is left to be made on
/// request.
async fn prepare_grid_copies(img: &ItemImage, source: &str, cache_dir: &Path, budget: u64) -> u64 {
    let known = match (img.width, img.height) {
        (Some(w), Some(h)) if w > 0 && h > 0 => Some((w as u32, h as u32)),
        _ => None,
    };
    let mut written = 0;
    for width in GRID_COPY_WIDTHS {
        if budget.saturating_sub(written) < 64 << 10 {
            break;
        }
        let Ok(rendition) = image_proxy::plan_rendition(source, known, Some(width), None).await
        else {
            continue;
        };
        let image_proxy::Rendition::Copy {
            width: w,
            height: h,
        } = rendition
        else {
            continue;
        };
        let name = image_proxy::cached_copy_name(source, img.stamp, w, h);
        if std::fs::metadata(cache_dir.join(&name)).is_ok_and(|m| m.len() > 0) {
            continue;
        }
        match image_proxy::render_image_for(
            source,
            img.stamp,
            rendition,
            cache_dir,
            None,
            image_proxy::RenderFor::Background,
        )
        .await
        {
            Ok(bytes) => written += bytes.len() as u64,
            Err(e) => tracing::debug!(path = %source, "could not prepare grid copy: {}", e),
        }
    }
    written
}

/// Prepares one picture: its grid master if missing and `budget` has room, and its BlurHash if it
/// has none (from the master, else from an original already on disk).
async fn warm_one(
    pool: &SqlitePool,
    img: &ItemImage,
    cache_dir: &Path,
    store: &Path,
    http_client: &reqwest::Client,
    budget: u64,
) -> Result<Warmed, String> {
    let remote = img.path.starts_with("http://") || img.path.starts_with("https://");
    // A downloaded remote original, or the local file; for a remote one not downloaded yet, None
    let on_disk = if remote {
        image_proxy::stored_remote_image(&img.path, store).map(|p| p.to_string_lossy().into_owned())
    } else {
        Some(img.path.clone())
    };
    let master_path =
        |source: &str| cache_dir.join(image_proxy::grid_master_name(source, img.stamp));
    let existed = on_disk
        .as_deref()
        .is_some_and(|s| std::fs::metadata(master_path(s)).is_ok_and(|m| m.len() > 0));
    if existed && img.blurhash.is_some() {
        // A server warmed before the copies were added gets them now
        let source = on_disk.expect("a master exists only for a picture on disk");
        return Ok(Warmed {
            written: prepare_grid_copies(img, &source, cache_dir, budget).await,
            hashed: false,
            capped: false,
        });
    }
    // A master takes about 60-100 KB; without room for one, only the BlurHash is made
    let room = budget >= 128 << 10;
    let mut warmed = Warmed {
        written: 0,
        hashed: false,
        capped: !existed && !room,
    };
    let picture = if existed || room {
        let source = match on_disk {
            Some(source) => source,
            None => image_proxy::localize_remote_image_checked(&img.path, store, Some(http_client))
                .await
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .into_owned(),
        };
        let master = image_proxy::render_image_for(
            &source,
            img.stamp,
            image_proxy::Rendition::Copy {
                width: image_proxy::GRID_MASTER.0,
                height: image_proxy::GRID_MASTER.1,
            },
            cache_dir,
            Some(http_client),
            image_proxy::RenderFor::Background,
        )
        .await?;
        if !existed {
            warmed.written = master.len() as u64;
        }
        warmed.written += prepare_grid_copies(
            img,
            &source,
            cache_dir,
            budget.saturating_sub(warmed.written),
        )
        .await;
        Some(master)
    } else if img.blurhash.is_none()
        && let Some(source) = on_disk
    {
        // Only a JPEG decoded at 1/8 scale (a few MiB), and only while no client render runs
        let bytes = tokio::fs::read(&source).await.map_err(|e| e.to_string())?;
        if image_proxy::render_cost_mib(&bytes, 32, 32) >= image_proxy::RENDER_BUDGET_MIB / 2 {
            return Ok(warmed);
        }
        while image_proxy::clients_rendering() {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        Some(bytes)
    } else {
        None
    };
    if img.blurhash.is_none()
        && let Some(bytes) = picture
    {
        let hash = tokio::task::spawn_blocking(move || image_proxy::blurhash_of(&bytes))
            .await
            .map_err(|e| e.to_string())?
            .ok_or("could not decode the picture")?;
        warmed.hashed = rustyfin_db::repo_image::set_image_blurhash(pool, img, &hash)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(warmed)
}
