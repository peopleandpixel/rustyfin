-- 021: Performance optimizations
-- Materialized filter index tables to avoid expensive DISTINCT queries
-- API cache table for L2 caching across restarts

-- Materialized genre index: pre-computed distinct genres with counts
CREATE TABLE IF NOT EXISTS filter_genre_index (
    genre TEXT PRIMARY KEY,
    item_count INTEGER NOT NULL DEFAULT 1
);

-- Materialized year index: pre-computed distinct years with counts
CREATE TABLE IF NOT EXISTS filter_year_index (
    year INTEGER PRIMARY KEY,
    item_count INTEGER NOT NULL DEFAULT 1
);

-- Materialized rating index: pre-computed distinct ratings with counts
CREATE TABLE IF NOT EXISTS filter_rating_index (
    rating TEXT PRIMARY KEY,
    item_count INTEGER NOT NULL DEFAULT 1
);

-- L2 API cache: persists expensive query results across restarts
CREATE TABLE IF NOT EXISTS api_cache (
    cache_key TEXT PRIMARY KEY,
    cache_value TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_api_cache_expires ON api_cache(expires_at);
