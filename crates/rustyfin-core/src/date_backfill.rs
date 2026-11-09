//! The one-time backfill of `date_created` for the items older versions stored, which dated each
//! item by the time of its scan. A scan dates a new item by its path but never rewrites an existing
//! item's date, so each item stored before gets the earlier of its stored date and its path's date,
//! from `scanner::path_date`. The marker row `date_added_backfill` (migration 061) holds the id of
//! the last item done, and the run that finishes deletes it.

use crate::scanner;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;
use std::path::Path;
use std::time::Instant;

/// One batch: the items after the last one done, of the kinds a scan dates by their paths.
const BATCH_SQL: &str = "SELECT id, path, date_created FROM media_items \
    WHERE id > ?1 AND media_type IN ('Movie','Series','Season','Episode', \
    'Audio','MusicAlbum','Photo') ORDER BY id LIMIT ?2";

/// What one run of the date added backfill did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BackfillRun {
    pub checked: u64,    // items read
    pub moved: u64,      // items moved to their path's date
    pub unreadable: u64, // items whose path couldn't be read; they keep their dates
    pub finished: bool,  // the backfill is done; no later start runs it
}

/// Items per batch. Each batch is read, then written in one short transaction.
pub const BATCH_SIZE: u32 = 500;

/// The whole pending backfill: run_some(pool, BATCH_SIZE, u32::MAX, after_batch).
/// after_batch runs after each batch commits.
pub async fn run_pending(
    pool: &SqlitePool,
    after_batch: impl FnMut(&BackfillRun),
) -> Result<Option<BackfillRun>, sqlx::Error> {
    run_some(pool, BATCH_SIZE, u32::MAX, after_batch).await
}

/// At most `max_batches` batches of `batch_size` items, from where the last run stopped.
/// after_batch runs after each batch commits.
pub async fn run_some(
    pool: &SqlitePool,
    batch_size: u32,
    max_batches: u32,
    after_batch: impl FnMut(&BackfillRun),
) -> Result<Option<BackfillRun>, sqlx::Error> {
    let marker: Option<String> =
        sqlx::query_scalar("SELECT value FROM configuration WHERE key = 'date_added_backfill'")
            .fetch_optional(pool)
            .await?;
    let Some(last) = marker else {
        return Ok(None);
    };
    let started = Instant::now();
    if last.is_empty() {
        tracing::info!("date added backfill: started");
    } else {
        tracing::info!("date added backfill: resumed after item {last}");
    }
    let mut run = BackfillRun::default();
    let result = run_batches(pool, &mut run, last, batch_size, max_batches, after_batch).await;
    let elapsed = started.elapsed();
    let BackfillRun {
        checked,
        moved,
        unreadable,
        finished,
    } = run;
    match result {
        Ok(()) => {
            let outcome = if finished {
                "finished"
            } else {
                "stopped part way"
            };
            tracing::info!(
                "date added backfill: {outcome} in {elapsed:.2?}; {checked} items checked, \
                 {moved} moved to their path's date, {unreadable} unreadable"
            );
            Ok(Some(run))
        }
        Err(error) => {
            tracing::warn!(
                "date added backfill: stopped by an error in {elapsed:.2?}; \
                 {checked} items checked, {moved} moved to their path's date, \
                 {unreadable} unreadable"
            );
            Err(error)
        }
    }
}

/// The batches of run_some, from `last`. A batch's moves count in `run` once it has committed.
/// after_batch runs after each batch commits.
async fn run_batches(
    pool: &SqlitePool,
    run: &mut BackfillRun,
    mut last: String,
    batch_size: u32,
    max_batches: u32,
    mut after_batch: impl FnMut(&BackfillRun),
) -> Result<(), sqlx::Error> {
    // A batch holds at least one item, so each batch moves the run on.
    let batch_size = batch_size.max(1);
    for _ in 0..max_batches {
        let rows: Vec<(String, String, String)> = sqlx::query_as(BATCH_SQL)
            .bind(last.as_str())
            .bind(i64::from(batch_size))
            .fetch_all(pool)
            .await?;
        // Each path is read on its own, with no transaction open.
        let mut moves = Vec::new();
        for (id, path, stored) in &rows {
            run.checked += 1;
            let Some(date) = scanner::path_date(Path::new(path)).await else {
                run.unreadable += 1;
                continue;
            };
            // A stored date that doesn't parse is kept.
            if let Ok(stored_date) = stored.parse::<DateTime<Utc>>()
                && date < stored_date
            {
                moves.push((id, path, stored, date));
            }
        }
        // The batch and the marker go in one short transaction. A short batch is the last one.
        let finished = (rows.len() as u64) < u64::from(batch_size);
        let next = rows.last().map(|row| row.0.clone()).unwrap_or_default();
        let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
        let mut batch_moved: u64 = 0;
        for (id, path, stored, date) in &moves {
            batch_moved += sqlx::query(
                "UPDATE media_items SET date_created = ?1 \
                 WHERE id = ?2 AND path = ?3 AND date_created = ?4",
            )
            .bind(date.to_rfc3339())
            .bind(id.as_str())
            .bind(path.as_str())
            .bind(stored.as_str())
            .execute(&mut *tx)
            .await?
            .rows_affected();
        }
        if finished {
            sqlx::query("DELETE FROM configuration WHERE key = 'date_added_backfill'")
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("UPDATE configuration SET value = ?1 WHERE key = 'date_added_backfill'")
                .bind(next.as_str())
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        run.moved += batch_moved;
        run.finished = finished;
        after_batch(run);
        if finished {
            break;
        }
        last = next;
    }
    Ok(())
}
