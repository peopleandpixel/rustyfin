use chrono::{DateTime, Utc};
use rustyfin_model::media::{MediaItem, MediaType};
use sqlx::SqlitePool;
use uuid::Uuid;

pub async fn upsert_media_item(pool: &SqlitePool, item: &MediaItem) -> Result<(), sqlx::Error> {
    let id = item.id.to_string();
    let parent_id = item.parent_id.map(|p| p.to_string());
    let library_id = item.library_id.to_string();
    let media_type = serde_json::to_string(&item.media_type).unwrap_or_default();
    let genres = serde_json::to_string(&item.genres).unwrap_or_default();
    let created = item.date_created.to_rfc3339();
    let premiere = item.premiere_date.map(|d| d.to_rfc3339());

    sqlx::query(
        "INSERT OR REPLACE INTO media_items \
         (id, name, sort_name, overview, media_type, path, parent_id, library_id, \
          production_year, community_rating, official_rating, genres, date_created, \
          premiere_date, runtime_ticks, index_number, parent_index_number) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(&item.name)
    .bind(&item.sort_name)
    .bind(&item.overview)
    .bind(&media_type)
    .bind(&item.path)
    .bind(&parent_id)
    .bind(&library_id)
    .bind(item.production_year)
    .bind(item.community_rating)
    .bind(&item.official_rating)
    .bind(&genres)
    .bind(&created)
    .bind(&premiere)
    .bind(item.runtime_ticks)
    .bind(item.index_number)
    .bind(item.parent_index_number)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn get_items_by_library(
    pool: &SqlitePool,
    library_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT * FROM media_items WHERE library_id = ? ORDER BY sort_name, name",
    )
    .bind(library_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_item_by_id(
    pool: &SqlitePool,
    id: Uuid,
) -> Result<Option<MediaItem>, sqlx::Error> {
    let row = sqlx::query_as::<_, MediaItemRow>("SELECT * FROM media_items WHERE id = ?")
        .bind(id.to_string())
        .fetch_optional(pool)
        .await?;

    Ok(row.map(Into::into))
}

pub async fn get_children(
    pool: &SqlitePool,
    parent_id: Uuid,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT * FROM media_items WHERE parent_id = ? ORDER BY index_number, sort_name, name",
    )
    .bind(parent_id.to_string())
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn delete_item(pool: &SqlitePool, id: Uuid) -> Result<bool, sqlx::Error> {
    // Delete children first (recursive via foreign key would be nice, but let's be explicit)
    sqlx::query("DELETE FROM media_items WHERE parent_id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    let result = sqlx::query("DELETE FROM media_items WHERE id = ?")
        .bind(id.to_string())
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn get_latest_items(
    pool: &SqlitePool,
    limit: i64,
    media_type: Option<&str>,
    library_id: Option<Uuid>,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let mut sql = String::from("SELECT * FROM media_items WHERE 1=1");
    if media_type.is_some() {
        sql.push_str(" AND media_type = ?");
    }
    if library_id.is_some() {
        sql.push_str(" AND library_id = ?");
    }
    sql.push_str(" ORDER BY date_created DESC LIMIT ?");

    let mut q = sqlx::query_as::<_, MediaItemRow>(&sql);
    if let Some(mt) = media_type {
        q = q.bind(mt);
    }
    if let Some(lid) = library_id {
        q = q.bind(lid.to_string());
    }
    q = q.bind(limit);

    let rows = q.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_items_filtered(
    pool: &SqlitePool,
    parent_id: Option<Uuid>,
    library_id: Option<Uuid>,
    media_types: Option<&[String]>,
    genres: Option<&[String]>,
    years: Option<&[i32]>,
    search_term: Option<&str>,
    sort_by: Option<&str>,
    sort_order: Option<&str>,
    start_index: i64,
    limit: i64,
) -> Result<(Vec<MediaItem>, i64), sqlx::Error> {
    let mut where_clause = String::from(" WHERE 1=1");
    if parent_id.is_some() {
        where_clause.push_str(" AND parent_id = ?");
    }
    if library_id.is_some() {
        where_clause.push_str(" AND library_id = ?");
    }
    if let Some(types) = media_types {
        if !types.is_empty() {
            let placeholders: Vec<&str> = types.iter().map(|_| "?").collect();
            where_clause.push_str(&format!(" AND media_type IN ({})", placeholders.join(",")));
        }
    }
    if let Some(genre_list) = genres {
        for _ in genre_list {
            where_clause.push_str(" AND genres LIKE ?");
        }
    }
    if let Some(year_list) = years {
        if !year_list.is_empty() {
            let placeholders: Vec<&str> = year_list.iter().map(|_| "?").collect();
            where_clause.push_str(&format!(
                " AND production_year IN ({})",
                placeholders.join(",")
            ));
        }
    }
    if search_term.is_some() {
        where_clause.push_str(" AND name LIKE ?");
    }

    let count_sql = format!("SELECT COUNT(*) as cnt FROM media_items{}", where_clause);
    let mut sql = format!("SELECT * FROM media_items{}", where_clause);

    let order = match sort_by.unwrap_or("SortName") {
        "DateCreated" => "date_created",
        "PremiereDate" => "premiere_date",
        "ProductionYear" => "production_year",
        "CommunityRating" => "community_rating",
        "Name" => "name",
        "Random" => "RANDOM()",
        _ => "sort_name",
    };
    let dir = if sort_order.unwrap_or("Ascending") == "Descending" {
        "DESC"
    } else {
        "ASC"
    };
    sql.push_str(&format!(" ORDER BY {} {} LIMIT ? OFFSET ?", order, dir));

    // Bind parameters for count query
    let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
    if let Some(pid) = parent_id {
        count_q = count_q.bind(pid.to_string());
    }
    if let Some(lid) = library_id {
        count_q = count_q.bind(lid.to_string());
    }
    if let Some(types) = media_types {
        for t in types {
            count_q = count_q.bind(t.clone());
        }
    }
    if let Some(genre_list) = genres {
        for g in genre_list {
            count_q = count_q.bind(format!("%{}%", g));
        }
    }
    if let Some(year_list) = years {
        for y in year_list {
            count_q = count_q.bind(*y);
        }
    }
    if let Some(term) = search_term {
        count_q = count_q.bind(format!("%{}%", term));
    }

    let total: i64 = count_q.fetch_one(pool).await.unwrap_or(0);

    // Bind parameters for data query
    let mut data_q = sqlx::query_as::<_, MediaItemRow>(&sql);
    if let Some(pid) = parent_id {
        data_q = data_q.bind(pid.to_string());
    }
    if let Some(lid) = library_id {
        data_q = data_q.bind(lid.to_string());
    }
    if let Some(types) = media_types {
        for t in types {
            data_q = data_q.bind(t.clone());
        }
    }
    if let Some(genre_list) = genres {
        for g in genre_list {
            data_q = data_q.bind(format!("%{}%", g));
        }
    }
    if let Some(year_list) = years {
        for y in year_list {
            data_q = data_q.bind(*y);
        }
    }
    if let Some(term) = search_term {
        data_q = data_q.bind(format!("%{}%", term));
    }
    data_q = data_q.bind(limit).bind(start_index);

    let rows = data_q.fetch_all(pool).await?;
    Ok((rows.into_iter().map(Into::into).collect(), total))
}

pub async fn get_similar_items(
    pool: &SqlitePool,
    item: &MediaItem,
    limit: i64,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    let media_type = serde_json::to_string(&item.media_type).unwrap_or_default();
    let rows = sqlx::query_as::<_, MediaItemRow>(
        "SELECT * FROM media_items WHERE media_type = ? AND id != ? AND library_id = ? \
         ORDER BY RANDOM() LIMIT ?",
    )
    .bind(&media_type)
    .bind(item.id.to_string())
    .bind(item.library_id.to_string())
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn get_filters(
    pool: &SqlitePool,
    library_id: Option<Uuid>,
) -> Result<(Vec<String>, Vec<i32>, Vec<String>), sqlx::Error> {
    // Genres
    let genre_sql = if library_id.is_some() {
        "SELECT DISTINCT genres FROM media_items WHERE library_id = ?"
    } else {
        "SELECT DISTINCT genres FROM media_items"
    };
    let mut q = sqlx::query_scalar::<_, String>(genre_sql);
    if let Some(lid) = library_id {
        q = q.bind(lid.to_string());
    }
    let genre_jsons: Vec<String> = q.fetch_all(pool).await?;
    let mut all_genres: Vec<String> = Vec::new();
    for gj in &genre_jsons {
        if let Ok(gs) = serde_json::from_str::<Vec<String>>(gj) {
            for g in gs {
                if !all_genres.contains(&g) {
                    all_genres.push(g);
                }
            }
        }
    }
    all_genres.sort();

    // Years
    let year_sql = if library_id.is_some() {
        "SELECT DISTINCT production_year FROM media_items WHERE production_year IS NOT NULL AND library_id = ? ORDER BY production_year DESC"
    } else {
        "SELECT DISTINCT production_year FROM media_items WHERE production_year IS NOT NULL ORDER BY production_year DESC"
    };
    let mut q2 = sqlx::query_scalar::<_, i32>(year_sql);
    if let Some(lid) = library_id {
        q2 = q2.bind(lid.to_string());
    }
    let years: Vec<i32> = q2.fetch_all(pool).await?;

    // Official ratings
    let rating_sql = if library_id.is_some() {
        "SELECT DISTINCT official_rating FROM media_items WHERE official_rating IS NOT NULL AND library_id = ? ORDER BY official_rating"
    } else {
        "SELECT DISTINCT official_rating FROM media_items WHERE official_rating IS NOT NULL ORDER BY official_rating"
    };
    let mut q3 = sqlx::query_scalar::<_, String>(rating_sql);
    if let Some(lid) = library_id {
        q3 = q3.bind(lid.to_string());
    }
    let ratings: Vec<String> = q3.fetch_all(pool).await?;

    Ok((all_genres, years, ratings))
}

pub async fn get_item_by_path(
    pool: &SqlitePool,
    path: &str,
) -> Result<Option<MediaItem>, sqlx::Error> {
    let row = sqlx::query_as::<_, MediaItemRow>("SELECT * FROM media_items WHERE path = ?")
        .bind(path)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(Into::into))
}

#[derive(sqlx::FromRow)]
struct MediaItemRow {
    id: String,
    name: String,
    sort_name: Option<String>,
    overview: Option<String>,
    media_type: String,
    path: String,
    parent_id: Option<String>,
    library_id: String,
    production_year: Option<i32>,
    community_rating: Option<f64>,
    official_rating: Option<String>,
    genres: String,
    date_created: String,
    premiere_date: Option<String>,
    runtime_ticks: Option<i64>,
    index_number: Option<i32>,
    parent_index_number: Option<i32>,
}

impl From<MediaItemRow> for MediaItem {
    fn from(r: MediaItemRow) -> Self {
        Self {
            id: Uuid::parse_str(&r.id).unwrap_or_default(),
            name: r.name,
            sort_name: r.sort_name,
            overview: r.overview,
            media_type: serde_json::from_str(&r.media_type).unwrap_or(MediaType::Unknown),
            path: r.path,
            parent_id: r.parent_id.and_then(|s| Uuid::parse_str(&s).ok()),
            library_id: Uuid::parse_str(&r.library_id).unwrap_or_default(),
            production_year: r.production_year,
            community_rating: r.community_rating,
            official_rating: r.official_rating,
            genres: serde_json::from_str(&r.genres).unwrap_or_default(),
            date_created: r.date_created.parse::<DateTime<Utc>>().unwrap_or_default(),
            premiere_date: r.premiere_date.and_then(|s| s.parse::<DateTime<Utc>>().ok()),
            runtime_ticks: r.runtime_ticks,
            index_number: r.index_number,
            parent_index_number: r.parent_index_number,
        }
    }
}
