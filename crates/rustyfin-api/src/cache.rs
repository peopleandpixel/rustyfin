use moka::sync::Cache;
use serde_json::Value;
use std::sync::Arc;

/// Shared in-memory response caches for API endpoints.
/// Uses Moka synchronous Cache (thread-safe, LRU eviction).
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
}

impl Default for ApiCache {
    fn default() -> Self {
        Self::new()
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
    /// The item ID prefix matches the cache key format: "{item_id}:{image_type}:..."
    pub fn invalidate_item(&self, _item_id: &str) {
        // Moka doesn't support prefix-based invalidation, so we invalidate all.
        // In practice, item image cache misses are cheap (disk cache fallback),
        // and the TTL handles expiration naturally.
        self.inner.invalidate_all();
    }
}

impl Default for ImageCache {
    fn default() -> Self {
        Self::new()
    }
}
