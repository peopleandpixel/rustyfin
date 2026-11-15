-- A metadata refresh finds each credited person by exact name (repo_person::set_item_people_if_none)
-- while it holds the write lock. The index uses the column's (default BINARY) collation, the one
-- that lookup compares with.
CREATE INDEX IF NOT EXISTS idx_people_name ON people(name);
