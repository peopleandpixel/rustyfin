//! The pools under a mixed load like a server's: tile set and segment saves, transactions that
//! read and then write, reads on both pools, and callers that give up part way (a client that
//! hangs up, a cancelled job).
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rustyfin_model::trickplay::TrickplayInfo;
use sqlx::SqlitePool;
use uuid::Uuid;

struct Load {
    stop: Arc<AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Load {
    async fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        for task in self.tasks {
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
    }
}

async fn seed(pool: &SqlitePool, items: usize) -> Arc<Vec<Uuid>> {
    let library = Uuid::new_v4();
    crate::insert_test_library(pool, library).await;
    let mut ids = Vec::with_capacity(items);
    for i in 0..items {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO media_items (id, name, media_type, path, library_id, date_created) \
             VALUES (?1, ?2, 'Episode', ?3, ?4, '2020-01-01')",
        )
        .bind(id.to_string())
        .bind(format!("Episode {i}"))
        .bind(format!("/media/{i}.mkv"))
        .bind(library.to_string())
        .execute(pool)
        .await
        .unwrap();
        ids.push(id);
    }
    Arc::new(ids)
}

/// A transaction that reads and then writes, as `BEGIN` (deferred) runs it.
async fn read_then_write(pool: &SqlitePool, id: Uuid) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT count(*) FROM media_segments")
        .fetch_one(&mut *tx)
        .await?;
    sqlx::query("UPDATE media_items SET sort_name = ?1 WHERE id = ?2")
        .bind("x")
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    tx.commit().await
}

async fn save_tiles(pool: &SqlitePool, id: Uuid) -> Result<(), sqlx::Error> {
    let info = TrickplayInfo {
        width: 320,
        height: 180,
        tile_width: 10,
        tile_height: 10,
        thumbnail_count: 100,
        interval: 10_000,
        bandwidth: 1,
    };
    crate::repo_trickplay::save_trickplay(pool, id, &info).await
}

async fn save_segments(pool: &SqlitePool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM media_segments WHERE item_id = ?1 AND segment_type = 'Intro'")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    crate::repo_media_segment::save_media_segment(pool, id, "Intro", 0, 10).await
}

async fn list(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT m.id, t.width, s.id FROM media_items m \
         LEFT JOIN trickplay_infos t ON t.item_id = m.id \
         LEFT JOIN media_segments s ON s.item_id = m.id ORDER BY m.name",
    )
    .fetch_all(pool)
    .await
    .map(|_| ())
}

/// Runs the load until stopped. Each call is given 5 to 400 ms, and dropped unfinished after.
fn start_load(rw: &SqlitePool, ro: &SqlitePool, ids: &Arc<Vec<Uuid>>) -> Load {
    let stop = Arc::new(AtomicBool::new(false));
    let mut tasks = Vec::new();
    for kind in 0..10 {
        let (rw, ro, ids, stop) = (rw.clone(), ro.clone(), ids.clone(), stop.clone());
        tasks.push(tokio::spawn(async move {
            while !stop.load(Ordering::Relaxed) {
                let id = ids[fastrand::usize(..ids.len())];
                let call = async {
                    match kind {
                        0..=2 => read_then_write(&rw, id).await,
                        3..=4 => save_tiles(&rw, id).await,
                        5 => save_segments(&rw, id).await,
                        6..=7 => list(&rw).await,
                        _ => list(&ro).await,
                    }
                };
                let limit = Duration::from_millis(fastrand::u64(5..400));
                let _ = tokio::time::timeout(limit, call).await;
            }
        }));
    }
    Load { stop, tasks }
}

async fn answers(pool: &SqlitePool) -> bool {
    let probe = sqlx::query_scalar::<_, i64>("SELECT count(*) FROM media_items").fetch_one(pool);
    matches!(
        tokio::time::timeout(Duration::from_secs(10), probe).await,
        Ok(Ok(_))
    )
}

// The nightly database task's integrity check, under that load, never returned on shared-cache
// connections, and every connection of both pools waited on it for good. Both pools then timed
// out every request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r3dbpool_integrity_check_under_load_leaves_the_pools_working() {
    let dir = crate::tests::temp_dir();
    let path = dir.join("pool.db");
    let path = path.to_str().unwrap();
    let rw = crate::init_db(path).await.unwrap();
    let ro = crate::init_read_only_db(path).await.unwrap();
    let ids = seed(&rw, 300).await;

    let load = start_load(&rw, &ro, &ids);
    tokio::time::sleep(Duration::from_secs(1)).await;
    // A few checks in a row: one alone sometimes slips between the transactions it waits on.
    let mut checked = Vec::new();
    let checks = async {
        for _ in 0..5 {
            let status = sqlx::query_scalar::<_, String>("PRAGMA integrity_check")
                .fetch_one(&rw)
                .await;
            checked.push(status.map_err(|e| e.to_string()));
        }
    };
    let finished = tokio::time::timeout(Duration::from_secs(20), checks)
        .await
        .is_ok();
    tokio::time::sleep(Duration::from_secs(2)).await;
    load.stop().await;

    let rw_answers = answers(&rw).await;
    let ro_answers = answers(&ro).await;
    assert!(finished, "the integrity checks hung after {checked:?}");
    assert!(
        checked.iter().all(|c| c.as_deref() == Ok("ok")),
        "the integrity checks failed: {checked:?}"
    );
    assert!(rw_answers, "the pool no longer answers");
    assert!(ro_answers, "the read-only pool no longer answers");
}
