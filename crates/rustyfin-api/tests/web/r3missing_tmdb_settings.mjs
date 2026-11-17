// The TMDb plugin settings: the unaired/missing episode options are in the configuration modal and
// their strings are translated. Reads the client sources; no server needed.
// Usage: node r3missing_tmdb_settings.mjs (run from the crate's test suite).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const plugins = fs.readFileSync(new URL('modules/plugins.js', src), 'utf8');
const en = JSON.parse(fs.readFileSync(new URL('translations/en.json', src), 'utf8'));
const de = JSON.parse(fs.readFileSync(new URL('translations/de.json', src), 'utf8'));

const CONFIG_KEYS = [
    'ImportUnairedEpisodes',
    'ImportMissingEpisodes',
    'ImportSpecials',
    'MissingEpisodeRefreshIntervalDays',
    'UpcomingEpisodeGracePeriodDays',
    'EnabledMissingEpisodeLibraries',
];

const I18N_KEYS = [
    'tmdb_import_unaired', 'tmdb_import_unaired_desc',
    'tmdb_import_missing', 'tmdb_import_missing_desc',
    'tmdb_import_specials', 'tmdb_import_specials_desc',
    'tmdb_refresh_days', 'tmdb_refresh_days_desc',
    'tmdb_grace_days', 'tmdb_grace_days_desc',
    'tmdb_libraries', 'tmdb_libraries_desc',
    'tmdb_no_libraries',
];

test('the modal reads and saves the six TMDb episode config keys', () => {
    for (const key of CONFIG_KEYS) {
        assert.ok(plugins.includes(key), `plugins.js uses ${key}`);
    }
    assert.match(plugins, /Object\.assign\(parsed, readEpisodeOptions\(\)\)/);
    assert.match(plugins, /pluginId === 'tmdb'/);
});

test('the episode option strings are in en', () => {
    for (const key of I18N_KEYS) {
        assert.ok(typeof en.admin?.[key] === 'string' && en.admin[key] !== '', `en admin.${key}`);
    }
});

test('the episode option strings are in de', () => {
    for (const key of I18N_KEYS) {
        assert.ok(typeof de.admin?.[key] === 'string' && de.admin[key] !== '', `de admin.${key}`);
    }
});

test('the plugins.js i18n keys all exist in en', () => {
    const used = [...plugins.matchAll(/'admin\.(tmdb_[a-z_]+)'/g)].map(m => m[1]);
    assert.ok(used.length > 0, 'plugins.js names tmdb_ keys');
    for (const key of used) {
        assert.ok(en.admin?.[key], `en admin.${key}`);
    }
});
