use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SyncPlayGroupRow {
    pub id: String,
    pub name: String,
    pub owner_user_id: String,
    pub is_locked: bool,
    pub play_method: String,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SyncPlayMemberRow {
    pub id: String,
    pub group_id: String,
    pub user_id: String,
    pub session_id: String,
}

pub async fn get_groups(pool: &SqlitePool) -> Result<Vec<SyncPlayGroupRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, SyncPlayGroupRow>(
        "SELECT id, name, owner_user_id, is_locked, play_method FROM sync_play_groups",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn get_group(
    pool: &SqlitePool,
    group_id: &str,
) -> Result<Option<SyncPlayGroupRow>, sqlx::Error> {
    let row = sqlx::query_as::<_, SyncPlayGroupRow>(
        "SELECT id, name, owner_user_id, is_locked, play_method FROM sync_play_groups WHERE id = ?1",
    )
    .bind(group_id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

pub async fn create_group(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    owner_user_id: &str,
    play_method: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sync_play_groups (id, name, owner_user_id, is_locked, play_method)
         VALUES (?1, ?2, ?3, 0, ?4)",
    )
    .bind(id)
    .bind(name)
    .bind(owner_user_id)
    .bind(play_method)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_group(pool: &SqlitePool, group_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sync_play_groups WHERE id = ?1")
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_members(
    pool: &SqlitePool,
    group_id: &str,
) -> Result<Vec<SyncPlayMemberRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, SyncPlayMemberRow>(
        "SELECT id, group_id, user_id, session_id FROM sync_play_members WHERE group_id = ?1",
    )
    .bind(group_id)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn add_member(
    pool: &SqlitePool,
    id: &str,
    group_id: &str,
    user_id: &str,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO sync_play_members (id, group_id, user_id, session_id)
         VALUES (?1, ?2, ?3, ?4)",
    )
    .bind(id)
    .bind(group_id)
    .bind(user_id)
    .bind(session_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove_member(pool: &SqlitePool, member_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sync_play_members WHERE id = ?1")
        .bind(member_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_member_by_session(
    pool: &SqlitePool,
    group_id: &str,
    session_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM sync_play_members WHERE group_id = ?1 AND session_id = ?2")
        .bind(group_id)
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn lock_group(
    pool: &SqlitePool,
    group_id: &str,
    locked: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sync_play_groups SET is_locked = ?1 WHERE id = ?2")
        .bind(locked)
        .bind(group_id)
        .execute(pool)
        .await?;
    Ok(())
}
