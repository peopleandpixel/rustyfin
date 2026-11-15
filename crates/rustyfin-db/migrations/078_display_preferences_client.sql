-- Display preferences are kept per user, id and client, as in Jellyfin, so one app saving its
-- settings for an id (such as a library's) doesn't replace another app's. Rows saved before carry
-- no client (''); they are kept as they are, and a client without a row of its own reads them.
CREATE TABLE display_preferences_by_client (
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    client TEXT NOT NULL DEFAULT '',
    value TEXT NOT NULL,
    PRIMARY KEY (user_id, key, client),
    FOREIGN KEY (user_id) REFERENCES users(id)
);

INSERT INTO display_preferences_by_client (user_id, key, client, value)
SELECT user_id, key, '', value FROM display_preferences;

DROP TABLE display_preferences;

ALTER TABLE display_preferences_by_client RENAME TO display_preferences;
