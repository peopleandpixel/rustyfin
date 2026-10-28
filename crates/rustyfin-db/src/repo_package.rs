use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PackageRepositoryRow {
    pub id: String,
    pub name: String,
    pub url: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct InstalledPackageRow {
    pub id: String,
    pub name: String,
    pub version: String,
    pub repository_id: Option<String>,
    pub enabled: bool,
    pub description: String,
}

pub async fn get_repositories(pool: &SqlitePool) -> Result<Vec<PackageRepositoryRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, PackageRepositoryRow>(
        "SELECT id, name, url, enabled FROM package_repositories",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn add_repository(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    url: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO package_repositories (id, name, url, enabled) VALUES (?1, ?2, ?3, 1)",
    )
    .bind(id)
    .bind(name)
    .bind(url)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn delete_repository(pool: &SqlitePool, repo_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM package_repositories WHERE id = ?1")
        .bind(repo_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_installed_packages(
    pool: &SqlitePool,
) -> Result<Vec<InstalledPackageRow>, sqlx::Error> {
    let rows = sqlx::query_as::<_, InstalledPackageRow>(
        "SELECT id, name, version, repository_id, enabled, description FROM installed_packages",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn install_package(
    pool: &SqlitePool,
    id: &str,
    name: &str,
    version: &str,
    repository_id: Option<&str>,
    description: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR REPLACE INTO installed_packages (id, name, version, repository_id, enabled, description)
         VALUES (?1, ?2, ?3, ?4, 1, ?5)",
    )
    .bind(id)
    .bind(name)
    .bind(version)
    .bind(repository_id)
    .bind(description)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn uninstall_package(pool: &SqlitePool, package_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM installed_packages WHERE id = ?1")
        .bind(package_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn toggle_package_enabled(
    pool: &SqlitePool,
    package_id: &str,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE installed_packages SET enabled = ?1 WHERE id = ?2")
        .bind(enabled)
        .bind(package_id)
        .execute(pool)
        .await?;
    Ok(())
}
