-- Migration 019: Client capabilities

ALTER TABLE sessions ADD COLUMN capabilities TEXT NOT NULL DEFAULT '{}';
