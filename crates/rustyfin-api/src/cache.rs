use moka::sync::Cache;
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::Arc;

/// Shared in-memory response caches for API endpoints.
/// Uses Moka synchronous Cache (thread-safe, LRU eviction).
/// Supports L2 SQLite-backed cache for persistence across restarts.
#[derive(Clone)]
pub struct ApiCache {
    /// Library views — change only on scan.
    /// Key: "views" (singleton), Value: JSON QueryResult of library views.
    pub library_views: Cache<String, Value>,

    /// Item details — change only on metadata refresh.
    /// Key: item UUID string, Value: JSON MediaItem.
    pub item_details: Cache<String, Value>,

    /// Filter lists — change only on scan.
    /// Key: library_id UUID string or "all", Value: JSON FiltersResult.
    pub filter_lists: Cache<String, Value>,

    /// Search results — short TTL (30 seconds).
    /// Key: normalized search term, Value: JSON QueryResult.
    pub search_results: Cache<String, Value>,

    /// Branding settings — splashscreen, CSS, etc.
    /// Key: setting name, Value: setting value.
    pub branding: Cache<String, String>,

    /// Optional L2 SQLite cache pool for persistence across restarts
    pub db: Option<SqlitePool>,
}

impl ApiCache {
    pub fn new() -> Self {
        Self {
            library_views: Cache::builder()
                .max_capacity(128)
                .time_to_live(std::time::Duration::from_secs(60))
                .build(),
            item_details: Cache::builder()
                .max_capacity(1024)
                .time_to_live(std::time::Duration::from_secs(30))
                .build(),
            filter_lists: Cache::builder()
                .max_capacity(128)
                .time_to_live(std::time::Duration::from_secs(120))
                .build(),
            search_results: Cache::builder()
                .max_capacity(256)
                .time_to_live(std::time::Duration::from_secs(30))
                .time_to_idle(std::time::Duration::from_secs(10))
                .build(),
            branding: Cache::builder()
                .max_capacity(64)
                .time_to_live(std::time::Duration::from_secs(3600))
                .build(),
            db: None,
        }
    }

    /// Invalidate caches when a library scan completes.
    pub fn invalidate_on_scan(&self) {
        self.library_views.invalidate("views");
        self.filter_lists.invalidate_all();
    }

    /// Invalidate a specific item's cache when metadata is refreshed.
    pub fn invalidate_item(&self, item_id: &str) {
        self.item_details.invalidate(item_id);
    }

    /// Set the database pool for L2 cache persistence.
    pub fn with_db(mut self, db: SqlitePool) -> Self {
        self.db = Some(db);
        self
    }

    /// Load L2 cache from SQLite on startup.
    /// Restores filter lists and library views from the persistent cache table.
    pub async fn load_l2_cache(&self) {
        let Some(pool) = &self.db else { return };

        // Clean up expired entries
        let now = chrono::Utc::now().to_rfc3339();
        if let Err(e) = sqlx::query("DELETE FROM api_cache WHERE expires_at < ?")
            .bind(&now)
            .execute(pool)
            .await
        {
            tracing::warn!("Failed to clean expired L2 cache entries: {}", e);
        }

        // Load filter lists
        match sqlx::query_as::<_, (String, String)>(
            "SELECT cache_key, cache_value FROM api_cache WHERE cache_key LIKE 'filters:%'",
        )
        .fetch_all(pool)
        .await
        {
            Ok(rows) => {
                let count = rows.len();
                for (key, value) in rows {
                    if let Ok(json) = serde_json::from_str(&value) {
                        let cache_key = key.strip_prefix("filters:").unwrap_or(&key).to_string();
                        self.filter_lists.insert(cache_key, json);
                    }
                }
                tracing::info!("Loaded {} filter entries from L2 cache", count);
            }
            Err(e) => {
                tracing::warn!("Failed to load L2 filter cache: {}", e);
            }
        }

        // Load library views
        match sqlx::query_as::<_, (String, String)>(
            "SELECT cache_key, cache_value FROM api_cache WHERE cache_key = 'views:library'",
        )
        .fetch_all(pool)
        .await
        {
            Ok(rows) => {
                let count = rows.len();
                for (_key, value) in rows {
                    if let Ok(json) = serde_json::from_str(&value) {
                        self.library_views.insert("views".to_string(), json);
                    }
                }
                tracing::info!("Loaded {} library view entries from L2 cache", count);
            }
            Err(e) => {
                tracing::warn!("Failed to load L2 library views cache: {}", e);
            }
        }
    }

    /// Save filter lists to L2 cache for persistence across restarts.
    pub async fn save_filters_to_l2(&self, library_id: &str, filters: &Value) {
        let Some(pool) = &self.db else { return };
        let key = format!("filters:{}", library_id);
        let value = serde_json::to_string(filters).unwrap_or_default();
        // 2 hour TTL
        let expires_at = chrono::Utc::now()
            .checked_add_days(chrono::Days::new(0))
            .unwrap()
            .checked_add_signed(chrono::TimeDelta::seconds(7200))
            .unwrap()
            .to_rfc3339();

        if let Err(e) = sqlx::query(
            "INSERT OR REPLACE INTO api_cache (cache_key, cache_value, expires_at) VALUES (?, ?, ?)",
        )
        .bind(&key)
        .bind(&value)
        .bind(&expires_at)
        .execute(pool)
        .await
        {
            tracing::warn!("Failed to save L2 cache: {}", e);
        }
    }
}

impl Default for ApiCache {
    fn default() -> Self {
        Self {
            library_views: Cache::builder()
                .max_capacity(128)
                .time_to_live(std::time::Duration::from_secs(60))
                .build(),
            item_details: Cache::builder()
                .max_capacity(1024)
                .time_to_live(std::time::Duration::from_secs(30))
                .build(),
            filter_lists: Cache::builder()
                .max_capacity(128)
                .time_to_live(std::time::Duration::from_secs(120))
                .build(),
            search_results: Cache::builder()
                .max_capacity(256)
                .time_to_live(std::time::Duration::from_secs(30))
                .time_to_idle(std::time::Duration::from_secs(10))
                .build(),
            branding: Cache::builder()
                .max_capacity(64)
                .time_to_live(std::time::Duration::from_secs(3600))
                .build(),
            db: None,
        }
    }
}

/// In-memory cache for image proxy results (small images < 1MB).
/// Shared across requests via AppState.
/// Uses Moka synchronous Cache (thread-safe, LRU eviction).
#[derive(Clone)]
pub struct ImageCache {
    inner: Arc<Cache<String, Vec<u8>>>,
}

impl ImageCache {
    /// Maximum size of a single cached image (1MB).
    const MAX_ENTRY_SIZE: usize = 1024 * 1024;

    pub fn new() -> Self {
        Self {
            inner: Arc::new(
                Cache::builder()
                    .max_capacity(512)
                    .time_to_live(std::time::Duration::from_secs(300))
                    .eviction_listener(
                        |key: Arc<String>,
                         _value: Vec<u8>,
                         reason: moka::notification::RemovalCause| {
                            match reason {
                                moka::notification::RemovalCause::Expired => {
                                    tracing::trace!("Image cache expired: {}", key);
                                }
                                moka::notification::RemovalCause::Size => {
                                    tracing::debug!(
                                        "Image cache capacity exceeded, evicting: {}",
                                        key
                                    );
                                }
                                moka::notification::RemovalCause::Explicit => {
                                    tracing::trace!("Image cache manually removed: {}", key);
                                }
                                moka::notification::RemovalCause::Replaced => {}
                            }
                        },
                    )
                    .build(),
            ),
        }
    }

    /// Get a cached image by key. Returns `None` if not found or expired.
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.inner.get(key)
    }

    /// Insert an image into the cache, only if it's under the size limit.
    /// Large images (>1MB) are skipped to prevent memory pressure.
    pub fn insert(&self, key: String, bytes: Vec<u8>) {
        if bytes.len() <= Self::MAX_ENTRY_SIZE {
            self.inner.insert(key, bytes);
        } else {
            tracing::trace!("Image too large for in-memory cache: {} bytes", bytes.len());
        }
    }

    /// Invalidate all cached images — called when a library scan completes.
    pub fn invalidate_all(&self) {
        self.inner.invalidate_all();
    }

    /// Invalidate cached images for a specific item — called when metadata is refreshed.
    /// Iterates over snapshot of keys and removes only those matching the item ID prefix.
    pub fn invalidate_item(&self, item_id: &str) {
        let prefix = format!("{}:", item_id);
        let keys: Vec<String> = self.inner.iter().map(|(k, _)| k.to_string()).collect();
        for key in keys {
            if key.starts_with(&prefix) {
                self.inner.invalidate(&key);
            }
        }
    }
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}
