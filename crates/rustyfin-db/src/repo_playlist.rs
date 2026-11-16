use chrono::{NaiveDateTime, TimeZone, Utc};
use rustyfin_model::media::MediaItem;
use rustyfin_model::playlist::{PlaylistDto, PlaylistUserPermissions};
use sqlx::{SqliteConnection, SqlitePool};
use std::collections::HashSet;
use uuid::Uuid;

/// Who calls: a user, who reaches the playlists Jellyfin lets them (their own, public ones, and
/// ones shared with them), or an administrator, who reaches any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaylistCaller {
    Owner(Uuid),
    Admin,
}

/// The user id to bind into the access conditions below: None for an administrator.
fn owner_filter(caller: PlaylistCaller) -> Option<String> {
    match caller {
        PlaylistCaller::Owner(uid) => Some(uid.to_string()),
        PlaylistCaller::Admin => None,
    }
}

/// SQL that is true when the caller bound at `?{n}` may read playlist `p`, as Jellyfin's
/// `Playlist.IsVisible`: an administrator, its owner, anyone when it is public, or a user it is
/// shared with.
fn can_read(n: u8) -> String {
    format!(
        "(?{n} IS NULL OR p.user_id = ?{n} OR p.open_access = 1 OR EXISTS (
            SELECT 1 FROM playlist_shares s WHERE s.playlist_id = p.id AND s.user_id = ?{n}))"
    )
}

/// SQL that is true when the caller bound at `?{n}` may change playlist `p`: an administrator,
/// its owner, or a user it is shared with for editing (Jellyfin's `CanEdit`).
fn can_edit(n: u8) -> String {
    format!(
        "(?{n} IS NULL OR p.user_id = ?{n} OR EXISTS (
            SELECT 1 FROM playlist_shares s
            WHERE s.playlist_id = p.id AND s.user_id = ?{n} AND s.can_edit = 1))"
    )
}

/// The entry id (Jellyfin's `PlaylistItemId`) of an item in a playlist. A playlist holds an item
/// once, so the pair names the entry; XOR keeps it reversible without a stored id.
pub fn entry_id(playlist_id: Uuid, item_id: Uuid) -> Uuid {
    let (p, i) = (playlist_id.as_u128(), item_id.as_u128());
    Uuid::from_u128(p ^ i)
}

/// The item an entry id names in a playlist: the inverse of `entry_id`.
pub fn entry_item(playlist_id: Uuid, entry: Uuid) -> Uuid {
    entry_id(playlist_id, entry)
}

fn parse_dt(s: &str) -> chrono::DateTime<Utc> {
    NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
        .map(|ndt| Utc.from_utc_datetime(&ndt))
        .unwrap_or_else(|_| Utc::now())
}

pub async fn create_playlist(
    pool: &SqlitePool,
    id: Uuid,
    name: &str,
    user_id: Uuid,
    media_type: Option<&str>,
    item_ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    let pid = id.to_string();
    let uid = user_id.to_string();
    sqlx::query("INSERT INTO playlists (id, name, user_id, media_type) VALUES (?1, ?2, ?3, ?4)")
        .bind(&pid)
        .bind(name)
        .bind(&uid)
        .bind(media_type)
        .execute(pool)
        .await?;

    for (i, item_id) in item_ids.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO playlist_items (playlist_id, item_id, sort_order) VALUES (?1, ?2, ?3)",
        )
        .bind(&pid)
        .bind(item_id.to_string())
        .bind(i as i32)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// A playlist row: id, name, owner, media type, entry count, created, public, caller may edit.
type PlaylistRow = (
    String,
    String,
    String,
    Option<String>,
    i64,
    String,
    bool,
    bool,
);

fn playlist_dto(r: PlaylistRow) -> Option<PlaylistDto> {
    Some(PlaylistDto {
        id: Uuid::parse_str(&r.0).ok()?,
        name: r.1,
        user_id: Uuid::parse_str(&r.2).ok()?,
        media_type: r.3,
        child_count: r.4,
        date_created: parse_dt(&r.5),
        open_access: r.6,
        shares: Vec::new(),
        item_ids: None,
        can_edit: r.7,
    })
}

/// The playlists a user reaches (their own, public ones and ones shared with them), by name.
pub async fn get_playlists(
    pool: &SqlitePool,
    user_id: Uuid,
) -> Result<Vec<PlaylistDto>, sqlx::Error> {
    let uid = user_id.to_string();
    let sql = format!(
        "SELECT p.id, p.name, p.user_id, p.media_type,
                (SELECT COUNT(*) FROM playlist_items pi WHERE pi.playlist_id = p.id),
                p.created_at, p.open_access, {edit}
         FROM playlists p
         WHERE {read}
         ORDER BY p.name",
        edit = can_edit(1),
        read = can_read(1),
    );
    let rows = sqlx::query_as::<_, PlaylistRow>(sqlx::AssertSqlSafe(sql))
        .bind(&uid)
        .fetch_all(pool)
        .await?;
    Ok(rows.into_iter().filter_map(playlist_dto).collect())
}

/// The ids of the playlists that hold an item of a library.
pub async fn playlists_in_library(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<std::collections::HashSet<Uuid>, sqlx::Error> {
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT pi.playlist_id FROM playlist_items pi
         JOIN media_items m ON m.id = pi.item_id
         WHERE m.library_id = ?1",
    )
    .bind(library_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect())
}

/// The item ids of a playlist the caller may read, in playlist order.
pub async fn get_playlist_item_ids(
    pool: &SqlitePool,
    playlist_id: Uuid,
    caller: PlaylistCaller,
) -> Result<Vec<Uuid>, sqlx::Error> {
    let pid = playlist_id.to_string();
    let owner = owner_filter(caller);
    let sql = format!(
        "SELECT pi.item_id FROM playlist_items pi
         JOIN playlists p ON p.id = pi.playlist_id
         WHERE pi.playlist_id = ?1 AND {read}
         ORDER BY pi.sort_order, pi.rowid",
        read = can_read(2),
    );
    let ids = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql))
        .bind(&pid)
        .bind(owner.as_deref())
        .fetch_all(pool)
        .await?;
    Ok(ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect())
}

/// The items of a playlist the caller may read, in playlist order, each with its
/// `PlaylistItemId`.
pub async fn get_playlist_items(
    pool: &SqlitePool,
    playlist_id: Uuid,
    caller: PlaylistCaller,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let ids = get_playlist_item_ids(pool, playlist_id, caller).await?;
    let mut items = Vec::with_capacity(ids.len());
    for id in ids {
        if let Ok(Some(mut item)) = crate::repo_media::get_item_by_id(pool, id).await {
            item.playlist_item_id = Some(entry_id(playlist_id, id));
            items.push(item);
        }
    }
    Ok(items)
}

/// A playlist the caller may read, with its shares and whether the caller may change it.
pub async fn get_playlist(
    pool: &SqlitePool,
    id: Uuid,
    caller: PlaylistCaller,
) -> Result<Option<PlaylistDto>, sqlx::Error> {
    let pid = id.to_string();
    let owner = owner_filter(caller);
    let sql = format!(
        "SELECT p.id, p.name, p.user_id, p.media_type,
                (SELECT COUNT(*) FROM playlist_items pi WHERE pi.playlist_id = p.id),
                p.created_at, p.open_access, {edit}
         FROM playlists p
         WHERE p.id = ?1 AND {read}",
        edit = can_edit(2),
        read = can_read(2),
    );
    let row = sqlx::query_as::<_, PlaylistRow>(sqlx::AssertSqlSafe(sql))
        .bind(&pid)
        .bind(owner.as_deref())
        .fetch_optional(pool)
        .await?;
    let Some(mut playlist) = row.and_then(playlist_dto) else {
        return Ok(None);
    };
    playlist.shares = get_shares(pool, id).await?;
    Ok(Some(playlist))
}

/// The users a playlist is shared with, in the order they were added.
pub async fn get_shares(
    pool: &SqlitePool,
    playlist_id: Uuid,
) -> Result<Vec<PlaylistUserPermissions>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, bool)>(
        "SELECT user_id, can_edit FROM playlist_shares WHERE playlist_id = ?1 ORDER BY rowid",
    )
    .bind(playlist_id.to_string())
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(user, can_edit)| {
            Some(PlaylistUserPermissions {
                user_id: Uuid::parse_str(&user).ok()?,
                can_edit,
            })
        })
        .collect())
}

/// Shares a playlist with a user, or changes what they may do. The owner needs no share, and a
/// user id that names no user is skipped.
pub async fn set_share(
    pool: &SqlitePool,
    playlist_id: Uuid,
    user_id: Uuid,
    can_edit: bool,
) -> Result<(), sqlx::Error> {
    let mut tx = crate::begin_write(pool).await?;
    insert_share(&mut tx, playlist_id, user_id, can_edit).await?;
    tx.commit().await
}

async fn insert_share(
    conn: &mut SqliteConnection,
    playlist_id: Uuid,
    user_id: Uuid,
    can_edit: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO playlist_shares (playlist_id, user_id, can_edit)
         SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM users WHERE id = ?2)
            AND NOT EXISTS (SELECT 1 FROM playlists WHERE id = ?1 AND user_id = ?2)
         ON CONFLICT (playlist_id, user_id) DO UPDATE SET can_edit = excluded.can_edit",
    )
    .bind(playlist_id.to_string())
    .bind(user_id.to_string())
    .bind(can_edit)
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn remove_share(
    pool: &SqlitePool,
    playlist_id: Uuid,
    user_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM playlist_shares WHERE playlist_id = ?1 AND user_id = ?2")
        .bind(playlist_id.to_string())
        .bind(user_id.to_string())
        .execute(pool)
        .await?;
    Ok(())
}

/// Replaces a playlist's shares with `shares`, as Jellyfin's create and update do with `Users`,
/// in one transaction.
pub async fn replace_shares(
    pool: &SqlitePool,
    playlist_id: Uuid,
    shares: &[PlaylistUserPermissions],
) -> Result<(), sqlx::Error> {
    let mut tx = crate::begin_write(pool).await?;
    replace_shares_on(&mut tx, playlist_id, shares).await?;
    tx.commit().await
}

async fn replace_shares_on(
    conn: &mut SqliteConnection,
    playlist_id: Uuid,
    shares: &[PlaylistUserPermissions],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM playlist_shares WHERE playlist_id = ?1")
        .bind(playlist_id.to_string())
        .execute(&mut *conn)
        .await?;
    for share in shares {
        insert_share(conn, playlist_id, share.user_id, share.can_edit).await?;
    }
    Ok(())
}

/// Makes a playlist public (Jellyfin's `IsPublic`, shown as `OpenAccess`) or private.
pub async fn set_open_access(
    pool: &SqlitePool,
    playlist_id: Uuid,
    open: bool,
) -> Result<(), sqlx::Error> {
    let mut tx = crate::begin_write(pool).await?;
    set_open_access_on(&mut tx, playlist_id, open).await?;
    tx.commit().await
}

async fn set_open_access_on(
    conn: &mut SqliteConnection,
    playlist_id: Uuid,
    open: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE playlists SET open_access = ?1 WHERE id = ?2")
        .bind(open)
        .bind(playlist_id.to_string())
        .execute(conn)
        .await?;
    Ok(())
}

/// Delete a playlist the caller owns (any, for an administrator) and its items. Returns false
/// when no playlist row matched.
pub async fn delete_playlist(
    pool: &SqlitePool,
    id: Uuid,
    caller: PlaylistCaller,
) -> Result<bool, sqlx::Error> {
    let pid = id.to_string();
    let owner = owner_filter(caller);
    let mut tx = crate::begin_write(pool).await?;
    sqlx::query(
        "DELETE FROM playlist_items WHERE playlist_id = ?1 AND EXISTS (
            SELECT 1 FROM playlists WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2))",
    )
    .bind(&pid)
    .bind(owner.as_deref())
    .execute(&mut *tx)
    .await?;
    let deleted =
        sqlx::query("DELETE FROM playlists WHERE id = ?1 AND (?2 IS NULL OR user_id = ?2)")
            .bind(&pid)
            .bind(owner.as_deref())
            .execute(&mut *tx)
            .await?
            .rows_affected();
    tx.commit().await?;
    Ok(deleted > 0)
}

pub async fn add_item_to_playlist(
    pool: &SqlitePool,
    playlist_id: Uuid,
    item_id: Uuid,
    caller: PlaylistCaller,
) -> Result<(), sqlx::Error> {
    add_items_to_playlist(pool, playlist_id, &[item_id], caller).await
}

/// Append items to a playlist the caller may change, in one transaction, in the order given.
/// Ids that name no item, or an item the playlist already holds, are skipped.
pub async fn add_items_to_playlist(
    pool: &SqlitePool,
    playlist_id: Uuid,
    item_ids: &[Uuid],
    caller: PlaylistCaller,
) -> Result<(), sqlx::Error> {
    let pid = playlist_id.to_string();
    let owner = owner_filter(caller);
    let mut tx = crate::begin_write(pool).await?;
    let sql = format!(
        "SELECT COALESCE(MAX(pi.sort_order), -1) + 1 FROM playlists p
         LEFT JOIN playlist_items pi ON pi.playlist_id = p.id
         WHERE p.id = ?1 AND {edit}
         GROUP BY p.id",
        edit = can_edit(2),
    );
    let next = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(sql))
        .bind(&pid)
        .bind(owner.as_deref())
        .fetch_optional(&mut *tx)
        .await?;
    let Some(next) = next else {
        return Ok(());
    };
    for (i, item_id) in item_ids.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO playlist_items (playlist_id, item_id, sort_order)
             SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM media_items WHERE id = ?2)",
        )
        .bind(&pid)
        .bind(item_id.to_string())
        .bind(next + i as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The entries of a playlist in order, every one, whatever the caller sees.
async fn all_item_ids(conn: &mut SqliteConnection, pid: &str) -> Result<Vec<Uuid>, sqlx::Error> {
    let ids = sqlx::query_scalar::<_, String>(
        "SELECT item_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY sort_order, rowid",
    )
    .bind(pid)
    .fetch_all(conn)
    .await?;
    Ok(ids.iter().filter_map(|s| Uuid::parse_str(s).ok()).collect())
}

/// The entries after an update with `Ids` from a caller who doesn't see `hidden` (parental
/// limits): the hidden entries keep their places, the other places take `wanted` in order, and
/// the rest of `wanted` follows. So an editor's update never drops an entry they can't see.
fn merge_keeping_hidden(current: &[Uuid], wanted: &[Uuid], hidden: &HashSet<Uuid>) -> Vec<Uuid> {
    let mut wanted = wanted.iter().filter(|id| !hidden.contains(id));
    let mut out = Vec::with_capacity(current.len());
    for id in current {
        if hidden.contains(id) {
            out.push(*id);
        } else if let Some(next) = wanted.next() {
            out.push(*next);
        }
    }
    out.extend(wanted);
    let mut seen = HashSet::new();
    out.retain(|id| seen.insert(*id));
    out
}

/// Moves the entry at `from` to `new_index` counted among the entries the caller sees (those not
/// in `hidden`): it lands just before the visible entry now at that index, or at the end.
fn move_among_visible(ids: &mut Vec<Uuid>, from: usize, new_index: usize, hidden: &HashSet<Uuid>) {
    let moved = ids.remove(from);
    let at = ids
        .iter()
        .enumerate()
        .filter(|(_, id)| !hidden.contains(id))
        .nth(new_index)
        .map_or(ids.len(), |(i, _)| i);
    ids.insert(at, moved);
}

/// Stores `ids` as a playlist's entries, in that order. Ids that name no item are skipped.
async fn write_items(
    conn: &mut SqliteConnection,
    pid: &str,
    ids: &[Uuid],
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?1")
        .bind(pid)
        .execute(&mut *conn)
        .await?;
    for (i, item_id) in ids.iter().enumerate() {
        sqlx::query(
            "INSERT OR IGNORE INTO playlist_items (playlist_id, item_id, sort_order)
             SELECT ?1, ?2, ?3 WHERE EXISTS (SELECT 1 FROM media_items WHERE id = ?2)",
        )
        .bind(pid)
        .bind(item_id.to_string())
        .bind(i as i64)
        .execute(&mut *conn)
        .await?;
    }
    Ok(())
}

pub async fn remove_item_from_playlist(
    pool: &SqlitePool,
    playlist_id: Uuid,
    item_id: Uuid,
    caller: PlaylistCaller,
) -> Result<(), sqlx::Error> {
    let pid = playlist_id.to_string();
    let iid = item_id.to_string();
    let owner = owner_filter(caller);
    let sql = format!(
        "DELETE FROM playlist_items WHERE playlist_id = ?1 AND item_id = ?2 AND EXISTS (
            SELECT 1 FROM playlists p WHERE p.id = ?1 AND {edit})",
        edit = can_edit(3),
    );
    sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(&pid)
        .bind(&iid)
        .bind(owner.as_deref())
        .execute(pool)
        .await?;
    Ok(())
}

/// Move an item of a playlist the caller may change to `new_index`, counted among the entries the
/// caller sees (not in `hidden`), and clamped to the end; the entries are renumbered. Returns
/// false when the caller may not change the playlist or it doesn't hold the item.
pub async fn move_playlist_item(
    pool: &SqlitePool,
    playlist_id: Uuid,
    item_id: Uuid,
    new_index: usize,
    hidden: &HashSet<Uuid>,
    caller: PlaylistCaller,
) -> Result<bool, sqlx::Error> {
    let pid = playlist_id.to_string();
    // Take the write lock before reading, so a write committed between the read and the
    // renumbering can't fail the move with SQLITE_BUSY_SNAPSHOT.
    let mut tx = crate::begin_write(pool).await?;
    let owner = owner_filter(caller);
    let sql = format!(
        "SELECT pi.item_id FROM playlist_items pi
         JOIN playlists p ON p.id = pi.playlist_id
         WHERE pi.playlist_id = ?1 AND {edit}
         ORDER BY pi.sort_order, pi.rowid",
        edit = can_edit(2),
    );
    let mut ids: Vec<Uuid> = sqlx::query_scalar::<_, String>(sqlx::AssertSqlSafe(sql))
        .bind(&pid)
        .bind(owner.as_deref())
        .fetch_all(&mut *tx)
        .await?
        .iter()
        .filter_map(|s| Uuid::parse_str(s).ok())
        .collect();
    let Some(from) = ids.iter().position(|id| *id == item_id) else {
        return Ok(false);
    };
    move_among_visible(&mut ids, from, new_index, hidden);
    for (order, id) in ids.iter().enumerate() {
        sqlx::query(
            "UPDATE playlist_items SET sort_order = ?1 WHERE playlist_id = ?2 AND item_id = ?3",
        )
        .bind(order as i64)
        .bind(&pid)
        .bind(id.to_string())
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(true)
}

/// Update a playlist's name and/or media type. Returns true when the caller may change it.
pub async fn update_playlist(
    pool: &SqlitePool,
    id: Uuid,
    name: Option<&str>,
    media_type: Option<&str>,
    caller: PlaylistCaller,
) -> Result<bool, sqlx::Error> {
    let update = PlaylistUpdate {
        name,
        media_type,
        ..PlaylistUpdate::default()
    };
    update_playlist_with(pool, id, &update, &HashSet::new(), caller).await
}

/// What an update (Jellyfin's UpdatePlaylistDto) changes; None leaves a field as it is.
#[derive(Debug, Default)]
pub struct PlaylistUpdate<'a> {
    pub name: Option<&'a str>,
    pub media_type: Option<&'a str>,
    /// The new entries, in order (`Ids`).
    pub item_ids: Option<&'a [Uuid]>,
    /// The new shares (`Users`).
    pub shares: Option<&'a [PlaylistUserPermissions]>,
    /// Public or private (`IsPublic`).
    pub public: Option<bool>,
}

/// Applies an update to a playlist the caller may change, in one transaction. New `item_ids` keep
/// the entries in `hidden`, which the caller can't see (see `merge_keeping_hidden`). Returns false,
/// changing nothing, when the caller may not change it.
pub async fn update_playlist_with(
    pool: &SqlitePool,
    id: Uuid,
    update: &PlaylistUpdate<'_>,
    hidden: &HashSet<Uuid>,
    caller: PlaylistCaller,
) -> Result<bool, sqlx::Error> {
    let pid = id.to_string();
    let owner = owner_filter(caller);
    let mut tx = crate::begin_write(pool).await?;
    let sql = format!(
        "UPDATE playlists AS p SET name = COALESCE(?1, p.name), media_type = COALESCE(?2, p.media_type)
         WHERE p.id = ?3 AND {edit}",
        edit = can_edit(4),
    );
    let found = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(update.name)
        .bind(update.media_type)
        .bind(&pid)
        .bind(owner.as_deref())
        .execute(&mut *tx)
        .await?
        .rows_affected()
        > 0;
    if !found {
        return Ok(false);
    }
    if let Some(wanted) = update.item_ids {
        let current = all_item_ids(&mut tx, &pid).await?;
        let ids = merge_keeping_hidden(&current, wanted, hidden);
        write_items(&mut tx, &pid, &ids).await?;
    }
    if let Some(shares) = update.shares {
        replace_shares_on(&mut tx, id, shares).await?;
    }
    if let Some(public) = update.public {
        set_open_access_on(&mut tx, id, public).await?;
    }
    tx.commit().await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{insert_test_item, insert_test_user, test_pool};
    use sqlx::SqlitePool;
    use uuid::Uuid;

    async fn setup() -> (SqlitePool, Uuid, Uuid) {
        let pool = test_pool().await;
        let user = Uuid::new_v4();
        let item = Uuid::new_v4();
        insert_test_user(&pool, user).await;
        insert_test_item(&pool, item).await;
        (pool, user, item)
    }

    #[tokio::test]
    async fn test_create_and_get_playlist() {
        let (pool, user, item) = setup().await;
        let id = Uuid::new_v4();
        create_playlist(&pool, id, "My Playlist", user, Some("Movie"), &[item])
            .await
            .unwrap();
        let playlist = get_playlist(&pool, id, PlaylistCaller::Owner(user))
            .await
            .unwrap();
        assert!(playlist.is_some());
        let playlist = playlist.unwrap();
        assert_eq!(playlist.name, "My Playlist");
        assert_eq!(playlist.child_count, 1);
        assert!(playlist.can_edit);
        assert!(!playlist.open_access);
    }

    #[tokio::test]
    async fn test_get_playlist_not_found() {
        let pool = test_pool().await;
        assert!(
            get_playlist(&pool, Uuid::new_v4(), PlaylistCaller::Admin)
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_get_playlists_empty() {
        let (pool, user, _) = setup().await;
        assert!(get_playlists(&pool, user).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_get_playlists() {
        let (pool, user, _) = setup().await;
        let id1 = Uuid::new_v4();
        let id2 = Uuid::new_v4();
        create_playlist(&pool, id1, "Alpha", user, None, &[])
            .await
            .unwrap();
        create_playlist(&pool, id2, "Beta", user, None, &[])
            .await
            .unwrap();
        let playlists = get_playlists(&pool, user).await.unwrap();
        assert_eq!(playlists.len(), 2);
        assert_eq!(playlists[0].name, "Alpha");
    }

    #[tokio::test]
    async fn test_delete_playlist() {
        let (pool, user, _) = setup().await;
        let pid = Uuid::new_v4();
        create_playlist(&pool, pid, "To Delete", user, None, &[])
            .await
            .unwrap();
        delete_playlist(&pool, pid, PlaylistCaller::Owner(user))
            .await
            .unwrap();
        assert!(
            get_playlist(&pool, pid, PlaylistCaller::Owner(user))
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn test_add_and_remove_item() {
        let (pool, user, item) = setup().await;
        let pid = Uuid::new_v4();
        create_playlist(&pool, pid, "Test", user, None, &[])
            .await
            .unwrap();
        let caller = PlaylistCaller::Owner(user);
        add_item_to_playlist(&pool, pid, item, caller)
            .await
            .unwrap();
        let playlist = get_playlist(&pool, pid, caller).await.unwrap().unwrap();
        assert_eq!(playlist.child_count, 1);
        remove_item_from_playlist(&pool, pid, item, caller)
            .await
            .unwrap();
        let playlist = get_playlist(&pool, pid, caller).await.unwrap().unwrap();
        assert_eq!(playlist.child_count, 0);
    }

    #[tokio::test]
    async fn test_update_playlist_name() {
        let (pool, user, _) = setup().await;
        let pid = Uuid::new_v4();
        create_playlist(&pool, pid, "Old", user, Some("Video"), &[])
            .await
            .unwrap();
        let caller = PlaylistCaller::Owner(user);
        update_playlist(&pool, pid, Some("New"), None, caller)
            .await
            .unwrap();
        let playlist = get_playlist(&pool, pid, caller).await.unwrap().unwrap();
        assert_eq!(playlist.name, "New");
        assert_eq!(playlist.media_type.as_deref(), Some("Video"));
    }

    #[tokio::test]
    async fn test_get_playlist_items_empty() {
        let pool = test_pool().await;
        let items = get_playlist_items(&pool, Uuid::new_v4(), PlaylistCaller::Admin)
            .await
            .unwrap();
        assert!(items.is_empty());
    }

    #[tokio::test]
    async fn r3collections_shares_decide_who_reads_and_who_edits() {
        let (pool, owner, item) = setup().await;
        let (reader, editor, stranger) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        for user in [reader, editor, stranger] {
            insert_test_user(&pool, user).await;
        }
        let pid = Uuid::new_v4();
        create_playlist(&pool, pid, "Shared", owner, None, &[])
            .await
            .unwrap();
        set_share(&pool, pid, reader, false).await.unwrap();
        set_share(&pool, pid, editor, true).await.unwrap();
        // The owner needs no share, and an unknown user gets none.
        set_share(&pool, pid, owner, false).await.unwrap();
        set_share(&pool, pid, Uuid::new_v4(), true).await.unwrap();
        let shares = get_shares(&pool, pid).await.unwrap();
        assert_eq!(shares.len(), 2);

        let read = |u| get_playlist(&pool, pid, PlaylistCaller::Owner(u));
        assert!(!read(reader).await.unwrap().unwrap().can_edit);
        assert!(read(editor).await.unwrap().unwrap().can_edit);
        assert!(read(stranger).await.unwrap().is_none());
        assert_eq!(get_playlists(&pool, reader).await.unwrap().len(), 1);
        assert!(get_playlists(&pool, stranger).await.unwrap().is_empty());

        // A reader can't add; an editor can; neither may delete it.
        add_item_to_playlist(&pool, pid, item, PlaylistCaller::Owner(reader))
            .await
            .unwrap();
        assert_eq!(read(owner).await.unwrap().unwrap().child_count, 0);
        add_item_to_playlist(&pool, pid, item, PlaylistCaller::Owner(editor))
            .await
            .unwrap();
        assert_eq!(read(owner).await.unwrap().unwrap().child_count, 1);
        for user in [reader, editor] {
            assert!(
                !delete_playlist(&pool, pid, PlaylistCaller::Owner(user))
                    .await
                    .unwrap()
            );
        }

        // Public: everyone reads it, nobody new may edit it.
        set_open_access(&pool, pid, true).await.unwrap();
        let seen = read(stranger).await.unwrap().unwrap();
        assert!(seen.open_access && !seen.can_edit);
        remove_share(&pool, pid, editor).await.unwrap();
        assert!(!read(editor).await.unwrap().unwrap().can_edit);
    }

    fn ids(n: u128) -> Vec<Uuid> {
        (1..=n).map(Uuid::from_u128).collect()
    }

    #[test]
    fn r3collections_a_move_counts_only_the_entries_the_caller_sees() {
        let [a, b, c, d, e]: [Uuid; 5] = ids(5).try_into().unwrap();
        let hidden: HashSet<Uuid> = [b, d].into();
        // The caller sees a, c, e; moving e to their index 1 puts it before c.
        let mut list = vec![a, b, c, d, e];
        move_among_visible(&mut list, 4, 1, &hidden);
        assert_eq!(list, vec![a, b, e, c, d]);
        // To their index 0: before a.
        let mut list = vec![b, a, c, d, e];
        move_among_visible(&mut list, 2, 0, &hidden);
        assert_eq!(list, vec![b, c, a, d, e]);
        // Past their last entry: to the end.
        let mut list = vec![a, b, c, d, e];
        move_among_visible(&mut list, 0, 9, &hidden);
        assert_eq!(list, vec![b, c, d, e, a]);
        // Nothing hidden: the plain index.
        let mut list = vec![a, b, c];
        move_among_visible(&mut list, 2, 0, &HashSet::new());
        assert_eq!(list, vec![c, a, b]);
    }

    #[test]
    fn r3collections_an_update_with_ids_keeps_the_hidden_entries() {
        let [a, b, c, d, e]: [Uuid; 5] = ids(5).try_into().unwrap();
        let hidden: HashSet<Uuid> = [b].into();
        // The caller saw a, c, d; they send d, a and a new e (c removed).
        assert_eq!(
            merge_keeping_hidden(&[a, b, c, d], &[d, a, e], &hidden),
            vec![d, b, a, e]
        );
        // Fewer: the hidden entry stays even with nothing around it.
        assert_eq!(merge_keeping_hidden(&[a, b, c], &[], &hidden), vec![b]);
        // A hidden id sent again isn't doubled.
        assert_eq!(merge_keeping_hidden(&[a, b], &[b, a], &hidden), vec![a, b]);
    }
}
