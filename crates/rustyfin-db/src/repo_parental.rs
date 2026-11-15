//! Parental limits: storing a user's policy, and finding the items it hides.
//!
//! An item is hidden when it, or any item above it (season, series, album), fails the policy:
//! - its rating, or when it has none its parent's or grandparent's (an episode takes its season's
//!   or series' rating), is above the maximum or is a rating no known system has while the item's
//!   kind is blocked when unrated (an unknown rating counts as unrated, as in Jellyfin);
//! - it has a blocked tag;
//! - it is a top-level item without an allowed tag, when the policy lists allowed tags.
//!
//! Ratings are matched in SQL against the stored values the policy allows, worked out per request
//! from the distinct values in the table; a value written after that (a scan running meanwhile)
//! is in neither list and hides its item until the next request. Tags are matched here in Rust,
//! because SQLite's `lower` folds ASCII only.

use std::collections::HashSet;

use rustyfin_model::parental::{ParentalPolicy, UNRATED_KIND_OF_TYPE, item_tag_keys, rating_score};
use sqlx::SqlitePool;
use uuid::Uuid;

/// Reads a user's parental policy. An unknown user, or the API key's nil id, has none.
pub async fn get_policy(pool: &SqlitePool, user_id: Uuid) -> Result<ParentalPolicy, sqlx::Error> {
    let row: Option<(Option<i32>, Option<i32>, String, String, String)> = sqlx::query_as(
        "SELECT max_parental_rating, max_parental_sub_rating, block_unrated_items, blocked_tags, \
         allowed_tags FROM users WHERE id = ?",
    )
    .bind(user_id.to_string())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(policy_from_row).unwrap_or_default())
}

/// Reads every user's parental policy, keyed by id.
pub async fn all_policies(
    pool: &SqlitePool,
) -> Result<std::collections::HashMap<Uuid, ParentalPolicy>, sqlx::Error> {
    let rows: Vec<(String, Option<i32>, Option<i32>, String, String, String)> = sqlx::query_as(
        "SELECT id, max_parental_rating, max_parental_sub_rating, block_unrated_items, \
         blocked_tags, allowed_tags FROM users",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, max, sub, unrated, blocked, allowed)| {
            Some((
                Uuid::parse_str(&id).ok()?,
                policy_from_row((max, sub, unrated, blocked, allowed)),
            ))
        })
        .collect())
}

fn policy_from_row(
    (max, sub, unrated, blocked, allowed): (Option<i32>, Option<i32>, String, String, String),
) -> ParentalPolicy {
    // Only set_policy writes these columns, always as JSON arrays of strings.
    let list = |s: &str| serde_json::from_str::<Vec<String>>(s).unwrap_or_default();
    ParentalPolicy {
        max_parental_rating: max,
        max_parental_sub_rating: sub,
        block_unrated_items: list(&unrated),
        blocked_tags: list(&blocked),
        allowed_tags: list(&allowed),
    }
}

/// Stores a user's parental policy. Returns false when there is no such user.
pub async fn set_policy(
    pool: &SqlitePool,
    user_id: Uuid,
    policy: &ParentalPolicy,
) -> Result<bool, sqlx::Error> {
    let json = |v: &[String]| serde_json::Value::from(v.to_vec()).to_string();
    let done = sqlx::query(
        "UPDATE users SET max_parental_rating = ?, max_parental_sub_rating = ?, \
         block_unrated_items = ?, blocked_tags = ?, allowed_tags = ? WHERE id = ?",
    )
    .bind(policy.max_parental_rating)
    .bind(policy.max_parental_sub_rating)
    .bind(json(&policy.block_unrated_items))
    .bind(json(&policy.blocked_tags))
    .bind(json(&policy.allowed_tags))
    .bind(user_id.to_string())
    .execute(pool)
    .await?;
    Ok(done.rows_affected() > 0)
}

/// The SQL `CASE` giving the unrated kind of `media_type`, NULL for folders an unrated limit
/// never hides.
fn unrated_kind_sql(col: &str) -> String {
    let mut sql = format!("CASE {col}");
    for (media_type, kind) in UNRATED_KIND_OF_TYPE {
        match kind {
            Some(kind) => sql.push_str(&format!(" WHEN '{media_type}' THEN '{kind}'")),
            None => sql.push_str(&format!(" WHEN '{media_type}' THEN NULL")),
        }
    }
    sql.push_str(" ELSE 'Other' END");
    sql
}

/// A stored rating, NULL when blank.
fn rating_sql(alias: &str) -> String {
    format!(
        "CASE WHEN TRIM(COALESCE({alias}.official_rating, '')) = '' THEN NULL \
         ELSE {alias}.official_rating END"
    )
}

/// The policy as the SQL parameters `?1` to `?5` of the queries below: whether ratings are
/// limited, the stored ratings within the limit, the stored ratings that count as unrated, the
/// unrated kinds to hide, and the ids that fail on their tags (JSON arrays).
struct Params {
    limits_ratings: bool,
    allowed: String,
    unrated: String,
    block_unrated: String,
    tag_failures: String,
}

/// Whether the `r` row (id, kind, eff = rating with inheritance) fails the policy in `?1`-`?5`.
/// The lists are bound parameters, not columns, so each `IN (SELECT ... json_each(?n))` is an
/// uncorrelated subquery that SQLite builds once per statement rather than once per row.
const FAILS_SQL: &str = "(r.id IN (SELECT value FROM json_each(?5)) \
     OR (?1 = 1 AND CASE \
         WHEN r.eff IS NULL OR r.eff IN (SELECT value FROM json_each(?3)) \
             THEN COALESCE(r.kind IN (SELECT value FROM json_each(?4)), 0) \
         WHEN r.eff IN (SELECT value FROM json_each(?2)) THEN 0 \
         ELSE 1 END))";

/// Every item the policy in `?1`-`?5` hides: the failing ones and everything under them.
fn hidden_sql() -> String {
    format!(
        "WITH RECURSIVE rated AS ({rated}), \
         failing(id) AS (SELECT r.id FROM rated r WHERE {FAILS_SQL}), \
         hidden(id) AS (SELECT id FROM failing \
             UNION SELECT c.id FROM media_items c JOIN hidden h ON c.parent_id = h.id) \
         SELECT id FROM hidden",
        rated = rated_select(""),
    )
}

/// Whether any of the items in the JSON array `?6` fails the policy in `?1`-`?5`.
fn chain_fails_sql() -> String {
    format!(
        "WITH rated AS ({rated}) SELECT EXISTS (SELECT 1 FROM rated r WHERE {FAILS_SQL})",
        rated = rated_select("WHERE m.id IN (SELECT value FROM json_each(?6))"),
    )
}

/// The rows `FAILS_SQL` reads: every media item `m`, or those `filter` (a WHERE clause) keeps.
fn rated_select(filter: &str) -> String {
    format!(
        "SELECT m.id AS id, {kind} AS kind, COALESCE({own}, {parent}, {grandparent}) AS eff \
         FROM media_items m \
         LEFT JOIN media_items mp ON mp.id = m.parent_id \
         LEFT JOIN media_items mg ON mg.id = mp.parent_id {filter}",
        kind = unrated_kind_sql("m.media_type"),
        own = rating_sql("m"),
        parent = rating_sql("mp"),
        grandparent = rating_sql("mg"),
    )
}

async fn params(
    pool: &SqlitePool,
    policy: &ParentalPolicy,
    tag_failures: &[String],
) -> Result<Params, sqlx::Error> {
    let mut allowed = Vec::new();
    let mut unrated = Vec::new();
    if policy.limits_ratings() {
        let ratings: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT official_rating FROM media_items WHERE official_rating IS NOT NULL",
        )
        .fetch_all(pool)
        .await?;
        for rating in ratings {
            match rating_score(&rating) {
                None => unrated.push(rating),
                Some(score) if policy.allows_score(score) => allowed.push(rating),
                Some(_) => {}
            }
        }
    }
    let json = |v: Vec<String>| serde_json::Value::from(v).to_string();
    Ok(Params {
        limits_ratings: policy.limits_ratings(),
        allowed: json(allowed),
        unrated: json(unrated),
        block_unrated: json(policy.block_unrated_items.clone()),
        tag_failures: json(tag_failures.to_vec()),
    })
}

/// Whether one item, by its tags, fails the policy: a blocked tag, or no allowed tag on a
/// top-level item when the policy has allowed tags.
fn fails_on_tags(
    tags: &str,
    top_level: bool,
    blocked: &HashSet<String>,
    allowed: &HashSet<String>,
) -> bool {
    if !blocked.is_empty() && item_tag_keys(tags).any(|t| blocked.contains(&t)) {
        return true;
    }
    top_level && !allowed.is_empty() && !item_tag_keys(tags).any(|t| allowed.contains(&t))
}

/// Every item the policy hides, the items under them included, by id as stored. Empty when the
/// policy hides nothing.
pub async fn hidden_item_ids(
    pool: &SqlitePool,
    policy: &ParentalPolicy,
) -> Result<HashSet<String>, sqlx::Error> {
    if !policy.is_restricted() {
        return Ok(HashSet::new());
    }
    let blocked: HashSet<String> = policy.blocked_tag_keys().into_iter().collect();
    let allowed: HashSet<String> = policy.allowed_tag_keys().into_iter().collect();
    let mut tag_failures = Vec::new();
    if !blocked.is_empty() || !allowed.is_empty() {
        let rows: Vec<(String, String, bool)> = sqlx::query_as(
            "SELECT id, tags, parent_id IS NULL FROM media_items \
             WHERE tags != '' OR (? AND parent_id IS NULL)",
        )
        .bind(!allowed.is_empty())
        .fetch_all(pool)
        .await?;
        tag_failures = rows
            .into_iter()
            .filter(|(_, tags, top)| fails_on_tags(tags, *top, &blocked, &allowed))
            .map(|(id, _, _)| id)
            .collect();
    }
    let p = params(pool, policy, &tag_failures).await?;
    let ids: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(hidden_sql()))
        .bind(p.limits_ratings)
        .bind(&p.allowed)
        .bind(&p.unrated)
        .bind(&p.block_unrated)
        .bind(&p.tag_failures)
        .fetch_all(pool)
        .await?;
    Ok(ids.into_iter().collect())
}

/// Whether the policy hides this item: it or an item above it fails. An id that isn't a media
/// item (a library, person, genre or collection) is never hidden by this check.
pub async fn is_item_hidden(
    pool: &SqlitePool,
    policy: &ParentalPolicy,
    item_id: Uuid,
) -> Result<bool, sqlx::Error> {
    if !policy.is_restricted() {
        return Ok(false);
    }
    // The item and every item above it, nearest first.
    let chain: Vec<(String, String, bool)> = sqlx::query_as(
        "WITH RECURSIVE chain(id, depth) AS (SELECT ?, 0 \
             UNION ALL SELECT m.parent_id, c.depth + 1 FROM chain c \
             JOIN media_items m ON m.id = c.id WHERE m.parent_id IS NOT NULL AND c.depth < 64) \
         SELECT m.id, m.tags, m.parent_id IS NULL FROM chain c JOIN media_items m ON m.id = c.id",
    )
    .bind(item_id.to_string())
    .fetch_all(pool)
    .await?;
    if chain.is_empty() {
        return Ok(false);
    }
    let blocked: HashSet<String> = policy.blocked_tag_keys().into_iter().collect();
    let allowed: HashSet<String> = policy.allowed_tag_keys().into_iter().collect();
    if chain
        .iter()
        .any(|(_, tags, top)| fails_on_tags(tags, *top, &blocked, &allowed))
    {
        return Ok(true);
    }
    if !policy.limits_ratings() {
        return Ok(false);
    }
    let ids: Vec<String> = chain.into_iter().map(|(id, _, _)| id).collect();
    let p = params(pool, policy, &[]).await?;
    let hidden: bool = sqlx::query_scalar(sqlx::AssertSqlSafe(chain_fails_sql()))
        .bind(p.limits_ratings)
        .bind(&p.allowed)
        .bind(&p.unrated)
        .bind(&p.block_unrated)
        .bind(&p.tag_failures)
        .bind(serde_json::Value::from(ids).to_string())
        .fetch_one(pool)
        .await?;
    Ok(hidden)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database of `movies` top-level movies (every 3rd tagged "Kids", every 7th "Horror") and
    /// `series` series with a season of 5 episodes each, under one library.
    async fn library(movies: usize, series: usize) -> (SqlitePool, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("rustyfin_parental_{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let pool = crate::init_db(dir.join("test.db").to_str().unwrap())
            .await
            .unwrap();
        let lib = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO libraries (id, name, content_type) VALUES (?, 'L', 'movies')")
            .bind(&lib)
            .execute(&pool)
            .await
            .unwrap();
        let mut tx = pool.begin().await.unwrap();
        let ratings = ["G", "PG", "PG-13", "R", ""];
        let add =
            |name: String, kind: &'static str, parent: Option<String>, rating: &str, tags: &str| {
                let id = Uuid::new_v4().to_string();
                let q = sqlx::query(
                    "INSERT INTO media_items (id, library_id, name, media_type, path, parent_id, \
                 official_rating, tags) VALUES (?, ?, ?, ?, ?, ?, NULLIF(?, ''), ?)",
                )
                .bind(id.clone())
                .bind(lib.clone())
                .bind(name.clone())
                .bind(kind)
                .bind(format!("/m/{name}"))
                .bind(parent)
                .bind(rating.to_string())
                .bind(tags.to_string());
                (id, q)
            };
        for n in 0..movies {
            let tags = match (n % 3, n % 7) {
                (_, 0) => "Kids|Horror",
                (0, _) => "Kids",
                _ => "",
            };
            let (_, q) = add(format!("movie {n}"), "Movie", None, ratings[n % 5], tags);
            q.execute(&mut *tx).await.unwrap();
        }
        for n in 0..series {
            let tags = if n % 2 == 0 { "Kids" } else { "" };
            let (sid, q) = add(format!("series {n}"), "Series", None, ratings[n % 5], tags);
            q.execute(&mut *tx).await.unwrap();
            let (season, q) = add(format!("season {n}"), "Season", Some(sid), "", "");
            q.execute(&mut *tx).await.unwrap();
            for e in 0..5 {
                let (_, q) = add(
                    format!("ep {n} {e}"),
                    "Episode",
                    Some(season.clone()),
                    "",
                    "",
                );
                q.execute(&mut *tx).await.unwrap();
            }
        }
        tx.commit().await.unwrap();
        (pool, dir)
    }

    fn strict_policy() -> ParentalPolicy {
        ParentalPolicy {
            max_parental_rating: Some(10),
            max_parental_sub_rating: None,
            block_unrated_items: vec!["Movie".into()],
            blocked_tags: vec!["horror".into()],
            allowed_tags: vec!["kids".into()],
        }
    }

    #[tokio::test]
    async fn r2parental_hidden_set_stays_fast_with_tag_lists() {
        let (pool, dir) = library(4000, 100).await;
        let policy = strict_policy();
        let started = std::time::Instant::now();
        let hidden = hidden_item_ids(&pool, &policy).await.unwrap();
        let took = started.elapsed();
        // 4,700 items, about 2,700 of them failing on tags alone, so the tag list is long. The
        // bound is loose so that a busy machine doesn't fail it.
        eprintln!("hidden set of {} items in {took:?}", hidden.len());
        assert!(took < std::time::Duration::from_secs(1), "took {took:?}");

        let ids: Vec<String> = hidden.into_iter().collect();
        let names: HashSet<String> = sqlx::query_scalar(
            "SELECT name FROM media_items WHERE id IN (SELECT value FROM json_each(?))",
        )
        .bind(serde_json::Value::from(ids).to_string())
        .fetch_all(&pool)
        .await
        .unwrap()
        .into_iter()
        .collect();
        for (name, hidden, why) in [
            ("movie 15", false, "Kids, G"),
            ("movie 6", false, "Kids, PG"),
            ("movie 1", true, "no Kids tag"),
            ("movie 0", true, "Kids and Horror"),
            ("movie 3", true, "Kids, R"),
            ("movie 9", true, "Kids, unrated movie"),
            ("ep 0 3", false, "under a Kids series rated G"),
            ("ep 1 0", true, "under a series without Kids"),
        ] {
            assert_eq!(names.contains(name), hidden, "{name}: {why}");
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn r2parental_hidden_set_plan_has_no_correlated_subquery() {
        let (pool, dir) = library(50, 5).await;
        let p = params(&pool, &strict_policy(), &["x".to_string()])
            .await
            .unwrap();
        for (sql, chain) in [(hidden_sql(), false), (chain_fails_sql(), true)] {
            let mut q = sqlx::query_as::<_, (i64, i64, i64, String)>(sqlx::AssertSqlSafe(format!(
                "EXPLAIN QUERY PLAN {sql}"
            )))
            .bind(p.limits_ratings)
            .bind(&p.allowed)
            .bind(&p.unrated)
            .bind(&p.block_unrated)
            .bind(&p.tag_failures);
            if chain {
                q = q.bind("[]");
            }
            let plan: Vec<String> = q
                .fetch_all(&pool)
                .await
                .unwrap()
                .into_iter()
                .map(|row| row.3)
                .collect();
            assert!(!plan.is_empty());
            assert!(
                !plan.iter().any(|step| step.contains("CORRELATED")),
                "{plan:#?}"
            );
            // The parents are found by primary key, not by scanning.
            assert!(
                plan.iter().any(|step| step.contains("mp USING INDEX")
                    || step.contains("mp USING PRIMARY KEY")
                    || step.contains("SEARCH mp")),
                "{plan:#?}"
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
