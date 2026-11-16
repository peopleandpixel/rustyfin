// R3 parental: the administrator's user editor shows a user's policy (switches, libraries, rating,
// unrated kinds, tags and schedules) and reads the form back into the whole policy Jellyfin Web
// posts, keeping the fields it doesn't show.
// Runs user-editor.js in a vm sandbox with its imports stubbed.
// Usage: node r3parental_user_editor.mjs (run by api_tests.rs r3parental_web_user_editor).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const module = fs.readFileSync(new URL('modules/user-editor.js', src), 'utf8')
    .replace(/^import .*$/gm, '')
    .replace(/^export \{[\s\S]*?\};$/m, '');
const en = JSON.parse(fs.readFileSync(new URL('translations/en.json', src), 'utf8'));

function sandbox() {
    const context = vm.createContext({
        t: (key, vars = {}) => {
            const [section, name] = key.split('.');
            const text = en[section]?.[name];
            assert.ok(text !== undefined, `en.json has ${key}`);
            return text.replace(/\{\{(\w+)\}\}/g, (_, k) => vars[k] ?? '');
        },
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        icon: name => `<svg data-icon="${name}"></svg>`,
        $: () => null,
        toast: () => {},
    });
    vm.runInContext(`${module}\nthis.api = { userEditorHtml, readUserEditor, hourLabel };`, context);
    return context.api;
}

const LIB_A = '0a1b2c3d-0000-4000-8000-00000000000a';
const LIB_B = '0a1b2c3d-0000-4000-8000-00000000000b';
const libs = [
    { Name: 'Kids Movies', ItemId: LIB_A.replace(/-/g, '') },
    { Name: 'Grown-up <Films>', ItemId: LIB_B },
];
const ratings = [
    { Name: 'Unrated', Value: null, RatingScore: null },
    { Name: 'G', Value: 0, RatingScore: { score: 0, subScore: null } },
    { Name: 'PG-13', Value: 13, RatingScore: { score: 13, subScore: null } },
    { Name: 'TV-14', Value: 14, RatingScore: { score: 14, subScore: 1 } },
];
const user = {
    Id: 'u1',
    Name: 'Kid',
    Policy: {
        IsAdministrator: false, IsHidden: true, IsDisabled: false, EnableRemoteAccess: false,
        EnableMediaPlayback: true, EnableAudioPlaybackTranscoding: true, EnableVideoPlaybackTranscoding: false,
        EnablePlaybackRemuxing: true, ForceRemoteSourceTranscoding: false, MaxActiveSessions: 2,
        RemoteClientBitrateLimit: 4000000, EnableAllFolders: false, EnabledFolders: [LIB_A],
        MaxParentalRating: 13, MaxParentalSubRating: null, BlockUnratedItems: ['Movie'],
        AllowedTags: ['Kids'], BlockedTags: [], AccessSchedules: [{ DayOfWeek: 'Weekday', StartHour: 7.5, EndHour: 20 }],
        EnableLiveTvAccess: true, SyncPlayAccess: 'CreateAndJoinGroups',
    },
};

// The checkbox named `name` (and with `value`) in the markup: whether it's there, checked and disabled
function box(html, name, value) {
    const re = new RegExp(`<input type="checkbox" name="${name}"([^>]*)>`, 'g');
    for (const m of html.matchAll(re)) {
        if (value === undefined || m[1].includes(`value="${value}"`)) {
            return { checked: / checked /.test(` ${m[1]} `), disabled: /\bdisabled\b/.test(m[1]) };
        }
    }
    return null;
}

test('the editor shows the stored policy', () => {
    const { userEditorHtml } = sandbox();
    const html = userEditorHtml(user, libs, ratings);
    assert.deepEqual(box(html, 'IsHidden'), { checked: true, disabled: false });
    assert.equal(box(html, 'EnableRemoteAccess').checked, false);
    assert.equal(box(html, 'EnableVideoPlaybackTranscoding').checked, false);
    assert.equal(box(html, 'EnableContentDeletion').checked, false, 'unset follows the admin flag');
    assert.match(html, /name="MaxActiveSessions"[^>]*value="2"/);
    assert.match(html, /name="RemoteClientBitrateLimit"[^>]*value="4"/, 'shown in Mbps');
    // Library access: ids match in either spelling; names are escaped
    assert.equal(box(html, 'EnableAllFolders').checked, false);
    assert.equal(box(html, 'folder', LIB_A.replace(/-/g, '')).checked, true);
    assert.equal(box(html, 'folder', LIB_B).checked, false);
    assert.ok(html.includes('Grown-up &lt;Films&gt;'));
    assert.doesNotMatch(html, /id="user-editor-folders" class="hidden/, 'the list shows when not all');
    // Parental control
    assert.match(html, /<option selected value="13:">PG-13<\/option>/);
    assert.ok(!html.includes('>Unrated</option>'), 'unrated is not a limit');
    assert.equal(box(html, 'unrated', 'Movie').checked, true);
    assert.equal(box(html, 'unrated', 'Series').checked, false);
    assert.ok(html.includes('>Kids'));
    assert.ok(html.includes('Weekdays 07:30–20:00'));
    for (const tab of ['profile', 'access', 'parental', 'password']) {
        assert.ok(html.includes(`data-tab-panel="${tab}"`), tab);
    }
});

test('the server owner and the administrator themself cannot change their role or disable themselves', () => {
    const { userEditorHtml } = sandbox();
    for (const lock of [{ self: true }, { owner: true }]) {
        const html = userEditorHtml(user, libs, ratings, lock);
        assert.equal(box(html, 'IsAdministrator').disabled, true);
        assert.equal(box(html, 'IsDisabled').disabled, true);
        assert.equal(box(html, 'IsHidden').disabled, false);
    }
});

// A form with the named controls; groups of checkboxes answer querySelectorAll(':checked')
function form(values, groups) {
    const elements = {};
    for (const [name, v] of Object.entries(values)) {
        elements[name] = typeof v === 'boolean' ? { checked: v, disabled: false } : typeof v === 'object' ? v : { value: v };
    }
    return {
        elements,
        querySelectorAll(selector) {
            const name = selector.match(/name="(\w+)"/)[1];
            return (groups[name] || []).map(value => ({ value }));
        },
    };
}

test('the form is read back into the whole policy', () => {
    const { readUserEditor } = sandbox();
    const f = form({
        Name: '  Kiddo ', IsAdministrator: { checked: true, disabled: true }, IsHidden: false, IsDisabled: false,
        EnableRemoteAccess: true, EnableContentDeletion: true, EnableMediaPlayback: true,
        EnableAudioPlaybackTranscoding: false, EnableVideoPlaybackTranscoding: true, EnablePlaybackRemuxing: false,
        ForceRemoteSourceTranscoding: false, MaxActiveSessions: '3', RemoteClientBitrateLimit: '1.5',
        EnableAllFolders: false, MaxParentalRating: '14:1',
    }, { folder: [LIB_B], unrated: ['Series', 'Book'] });
    const lists = {
        AllowedTags: ['Kids'], BlockedTags: ['Horror'],
        AccessSchedules: [{ DayOfWeek: 'Sunday', StartHour: 10, EndHour: 12.5 }],
    };
    const { name, policy } = readUserEditor(f, user.Policy, lists);
    assert.equal(name, 'Kiddo');
    assert.equal(policy.IsAdministrator, false, 'a locked switch keeps the stored value');
    assert.equal(policy.IsHidden, false);
    assert.equal(policy.EnableRemoteAccess, true);
    assert.equal(policy.EnableContentDeletion, true);
    assert.equal(policy.EnableAudioPlaybackTranscoding, false);
    assert.equal(policy.EnablePlaybackRemuxing, false);
    assert.equal(policy.MaxActiveSessions, 3);
    assert.equal(policy.RemoteClientBitrateLimit, 1500000);
    assert.equal(policy.EnableAllFolders, false);
    assert.deepEqual([...policy.EnabledFolders], [LIB_B]);
    assert.equal(policy.MaxParentalRating, 14);
    assert.equal(policy.MaxParentalSubRating, 1);
    assert.deepEqual([...policy.BlockUnratedItems], ['Series', 'Book']);
    assert.deepEqual([...policy.BlockedTags], ['Horror']);
    assert.equal(policy.AccessSchedules[0].EndHour, 12.5);
    assert.equal(policy.EnableLiveTvAccess, true, 'fields the editor does not show are kept');
    assert.equal(policy.SyncPlayAccess, 'CreateAndJoinGroups');

    // No limit, no bitrate and a negative session count
    const open = readUserEditor(form({
        Name: 'Kid', IsAdministrator: false, IsHidden: false, IsDisabled: false, EnableRemoteAccess: true,
        EnableContentDeletion: false, EnableMediaPlayback: true, EnableAudioPlaybackTranscoding: true,
        EnableVideoPlaybackTranscoding: true, EnablePlaybackRemuxing: true, ForceRemoteSourceTranscoding: false,
        MaxActiveSessions: '-2', RemoteClientBitrateLimit: '', EnableAllFolders: true, MaxParentalRating: '',
    }, {}), user.Policy, { AllowedTags: [], BlockedTags: [], AccessSchedules: [] }).policy;
    assert.equal(open.MaxParentalRating, null);
    assert.equal(open.MaxParentalSubRating, null);
    assert.equal(open.RemoteClientBitrateLimit, 0);
    assert.equal(open.MaxActiveSessions, 0);
    assert.equal(open.EnableAllFolders, true);
    assert.equal(open.AccessSchedules.length, 0);
});

test('hours show as clock times', () => {
    const { hourLabel } = sandbox();
    assert.equal(hourLabel(0), '00:00');
    assert.equal(hourLabel(7.5), '07:30');
    assert.equal(hourLabel(24), '24:00');
});
