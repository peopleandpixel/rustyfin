// R3 admin dashboard 2: shut down and restart ask first, the Overview lists the server's state
// and paths, the General tab saves over the whole configuration, the library editor and the
// triggers editor send Jellyfin's requests, and a transcoding session shows why it transcodes.
// Runs dashboard.js (imports and exports stripped) and sessions.js parts in a vm sandbox with stubs.
// Usage: node r3admin_dashboard.mjs (run by api_tests.rs r3admin_web_dashboard).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const dashboard = read('modules/dashboard.js');
const sessions = read('modules/sessions.js');
const admin = read('modules/admin.js');
const router = read('modules/router.js');
const app = read('app.js');

// Objects made in the sandbox have its prototypes: they are compared as plain JSON
const same = (actual, expected, message) => assert.deepEqual(JSON.parse(JSON.stringify(actual ?? null)), expected ?? null, message);

const escapeHtml = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

function element(props = {}) {
    const classes = new Set();
    return {
        innerHTML: '', textContent: '', value: '', checked: false, open: false, listeners: {},
        classList: { add: c => classes.add(c), remove: c => classes.delete(c), toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)), contains: c => classes.has(c) },
        addEventListener(type, fn) { this.listeners[type] = fn; },
        ...props,
    };
}

// dashboard.js in a sandbox: `answer` is the confirm dialog's answer, `api` the stubbed API calls
function sandbox({ answer = 'ok', api = {}, elements = {} } = {}) {
    const calls = [];
    const toasts = [];
    const renders = [];
    const dialog = element({
        showModal() { this.open = true; this.returnValue = answer; this.open = false; this.listeners.close?.(); },
        close() { this.open = false; this.listeners.close?.(); },
    });
    const els = { 'dashboard-confirm': dialog, ...elements };
    const record = (method) => async (path, body) => { calls.push([method, path, body]); return api[`${method} ${path}`]; };
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml,
        icon: name => `<i data-icon="${name}"></i>`,
        $: id => els[id] || null,
        show: id => els[id]?.classList.remove('hidden'),
        hide: id => els[id]?.classList.add('hidden'),
        toast: (message, kind) => toasts.push([message, kind]),
        renderAdmin: tab => renders.push(tab),
        refreshTasks: () => renders.push('tasks'),
        document: { body: { appendChild() {} }, createElement: () => element(), documentElement: { lang: 'en' }, querySelectorAll: () => [] },
        URLSearchParams,
        API: {
            get: record('GET'), post: record('POST'), del: record('DELETE'),
            getConfig: async () => { calls.push(['GET', '/System/Configuration']); return api['GET /System/Configuration']; },
            updateConfig: async body => { calls.push(['POST', '/System/Configuration', body]); },
            restart: async () => { calls.push(['POST', '/System/Restart']); },
            shutdown: async () => { calls.push(['POST', '/System/Shutdown']); },
            getFullInfo: async () => api['GET /System/Info'] || {},
        },
    });
    const body = dashboard.replace(/^import .*\n/gm, '').replace(/^export \{[\s\S]*?\};\n?/m, '');
    vm.runInContext(`${body}\nthis.fns = { serverPower, serverDetailsHtml, readGeneralSettings, loadGeneralSettings, saveGeneralSettings, generalSettingsHtml,
        openLibraryEditor, addLibraryFolder, removeLibraryFolder, renameLibrary, triggerSummary, readTrigger,
        openTriggerEditor, addTaskTrigger, removeTaskTrigger };`, context);
    return { fns: context.fns, calls, toasts, renders };
}

test('shut down and restart ask first, and only a confirmed one is sent', async () => {
    let s = sandbox({ answer: 'cancel' });
    await s.fns.serverPower('shutdown');
    same(s.calls, []);
    s = sandbox({ answer: 'ok' });
    await s.fns.serverPower('shutdown');
    same(s.calls, [['POST', '/System/Shutdown']]);
    same(s.toasts, [['dashboard.shutting_down', null]]);
    s = sandbox({ answer: 'ok' });
    await s.fns.serverPower('restart');
    same(s.calls, [['POST', '/System/Restart']]);
    // The header has both buttons, wired through the router's actions
    assert.match(admin, /data-action="server-restart"/);
    assert.match(admin, /data-action="server-shutdown"/);
    assert.match(router, /'server-shutdown': \(\) => serverPower\('shutdown'\)/);
});

test('the Overview lists the version, OS, paths and a pending restart or update', () => {
    const { fns } = sandbox();
    const info = {
        ServerName: 'Box', Version: '10.11.0', OperatingSystem: 'linux', SystemArchitecture: 'X64',
        ProgramDataPath: '/data', CachePath: '/data/cache', InternalMetadataPath: '/data/metadata',
        TranscodingTempPath: '/data/cache/transcode', LogPath: '/data/logs', WebPath: '/web',
    };
    let html = fns.serverDetailsHtml(info, '0.9.0');
    for (const value of ['Box', '0.9.0', '10.11.0', 'linux · X64', '/data/cache', '/data/metadata', '/data/cache/transcode', '/data/logs', '/web']) {
        assert.ok(html.includes(value), value);
    }
    assert.match(html, /dashboard\.status_running/);
    html = fns.serverDetailsHtml({ ...info, HasPendingRestart: true, HasUpdateAvailable: true }, '0.9.0');
    assert.match(html, /dashboard\.restart_pending/);
    assert.match(html, /dashboard\.update_available/);
    assert.doesNotMatch(html, /dashboard\.status_running/);
});

test('the General tab saves its fields over the configuration as it is now', async () => {
    const stored = { ServerName: 'Old', UICulture: 'en-US', PreferredMetadataLanguage: 'en', MetadataCountryCode: 'US', QuickConnectAvailable: true, MinResumePct: 7 };
    const form = { elements: {
        ServerName: { value: ' New name ' }, UICulture: { value: 'de' }, PreferredMetadataLanguage: { value: 'de' },
        MetadataCountryCode: { value: 'DE' }, QuickConnectAvailable: { checked: false },
    } };
    const s = sandbox({ elements: { 'general-settings': form, 'general-settings-error': element() } });
    // A save before the tab loaded sends nothing
    await s.fns.saveGeneralSettings({ preventDefault() {} });
    same(s.calls, []);

    same(s.fns.readGeneralSettings(form, stored), {
        ...stored, ServerName: 'New name', UICulture: 'de', PreferredMetadataLanguage: 'de', MetadataCountryCode: 'DE', QuickConnectAvailable: false,
    });
    const html = s.fns.generalSettingsHtml(stored, { CachePath: '/data/cache', InternalMetadataPath: '/data/metadata' },
        [{ DisplayName: 'German', TwoLetterISOLanguageName: 'de' }], [{ DisplayName: 'Germany', TwoLetterISORegionName: 'DE' }], [{ Name: 'English', Value: 'en-US' }]);
    assert.match(html, /id="gen-cache-path"[^>]*value="\/data\/cache" readonly/);
    assert.match(html, /id="gen-metadata-path"[^>]*value="\/data\/metadata" readonly/);
    assert.match(html, /id="gen-quick-connect"[^>]* checked/);
    assert.match(html, /<option value="en-US" selected>English<\/option>/);
    // A stored value missing from the list stays selectable
    assert.match(html, /<option value="en" selected>en<\/option>/);
});

test('a General save posts the fresh configuration with the form on top', async () => {
    const form = { elements: {
        ServerName: { value: 'N' }, UICulture: { value: 'en-US' }, PreferredMetadataLanguage: { value: 'fr' },
        MetadataCountryCode: { value: 'FR' }, QuickConnectAvailable: { checked: true },
    } };
    const api = { 'GET /System/Configuration': { ServerName: 'Old', MinResumePct: 7 }, 'GET /System/Info': {} };
    const s = sandbox({ api, elements: { 'admin-general': element(), 'general-settings': form, 'general-settings-error': element() } });
    await s.fns.loadGeneralSettings();
    // Another tab saves meanwhile: its change is kept
    api['GET /System/Configuration'] = { ServerName: 'Old', MinResumePct: 9 };
    await s.fns.saveGeneralSettings({ preventDefault() {} });
    const posted = s.calls.filter(c => c[0] === 'POST');
    same(posted, [['POST', '/System/Configuration', {
        ServerName: 'N', MinResumePct: 9, UICulture: 'en-US', PreferredMetadataLanguage: 'fr', MetadataCountryCode: 'FR', QuickConnectAvailable: true,
    }]]);
    same(s.toasts, [['dashboard.saved', null]]);
});

test('the display languages fall back to the cultures when /Localization/Options is no list', async () => {
    const box = element();
    const s = sandbox({
        api: {
            'GET /System/Configuration': { UICulture: 'de' }, 'GET /System/Info': {},
            'GET /Localization/Cultures': [{ DisplayName: 'German', TwoLetterISOLanguageName: 'de' }],
            'GET /Localization/Countries': [], 'GET /Localization/Options': { SubtitleLanguages: [] },
        },
        elements: { 'admin-general': box },
    });
    await s.fns.loadGeneralSettings();
    assert.match(box.innerHTML, /id="gen-ui-culture"[^>]*><option value="de" selected>German<\/option>/);
});

test('the library editor sends Jellyfin\'s folder and rename requests', async () => {
    const lib = { Name: 'Films', ItemId: 'abcd-1', Locations: ['/m/a', '/m/b'] };
    const editor = element({ open: false, showModal() { this.open = true; } });
    const s = sandbox({
        api: { 'GET /Library/VirtualFolders': [lib] },
        elements: { 'library-editor': editor, 'lib-edit-new-path': { value: ' /m/c ' }, 'lib-edit-name': { value: 'Movies' } },
    });
    await s.fns.openLibraryEditor('ABCD1');
    assert.match(editor.innerHTML, /data-action="library-remove-path" data-path="\/m\/a"/);
    assert.match(editor.innerHTML, /data-action="browse-path"/);
    s.calls.length = 0;
    await s.fns.addLibraryFolder({ preventDefault() {} });
    same(s.calls[0], ['POST', '/Library/VirtualFolders/Paths?refreshLibrary=true', { Name: 'Films', PathInfo: { Path: '/m/c' } }]);
    s.calls.length = 0;
    await s.fns.removeLibraryFolder('/m/b');
    same(s.calls[0], ['DELETE', '/Library/VirtualFolders/Paths?name=Films&path=%2Fm%2Fb&refreshLibrary=true', null]);
    s.calls.length = 0;
    await s.fns.renameLibrary({ preventDefault() {} });
    same(s.calls[0], ['POST', '/Library/VirtualFolders/Name?name=Films&newName=Movies', null]);
    // Closing the editor shows the libraries tab again
    editor.listeners.close();
    same(s.renders, ['libraries']);

    // The last folder can't be removed
    const last = element({ showModal() { this.open = true; } });
    const one = sandbox({ api: { 'GET /Library/VirtualFolders': [{ ...lib, Locations: ['/m/a'] }] }, elements: { 'library-editor': last } });
    await one.fns.openLibraryEditor('abcd-1');
    assert.match(last.innerHTML, /data-path="\/m\/a" disabled/);
    assert.match(admin, /data-action="edit-library"/);
});

test('triggers are read from the form in Jellyfin\'s shape and shown in words', () => {
    const { fns } = sandbox();
    const form = type => ({ elements: { Type: { value: type }, TimeOfDay: { value: '03:30' }, DayOfWeek: { value: 'Sunday' }, Interval: { value: '45' } } });
    const MIN = 600000000;
    same(fns.readTrigger(form('DailyTrigger')), { Type: 'DailyTrigger', TimeOfDayTicks: 210 * MIN });
    same(fns.readTrigger(form('WeeklyTrigger')), { Type: 'WeeklyTrigger', TimeOfDayTicks: 210 * MIN, DayOfWeek: 'Sunday' });
    same(fns.readTrigger(form('IntervalTrigger')), { Type: 'IntervalTrigger', IntervalTicks: 45 * MIN });
    same(fns.readTrigger(form('StartupTrigger')), { Type: 'StartupTrigger' });

    assert.equal(fns.triggerSummary({ Type: 'DailyTrigger', TimeOfDayTicks: 210 * MIN }), 'dashboard.daily_at|{"time":"03:30"}');
    assert.equal(fns.triggerSummary({ Type: 'WeeklyTrigger', TimeOfDayTicks: 0, DayOfWeek: 'Sunday' }), 'dashboard.weekly_at|{"day":"Sunday","time":"00:00"}');
    assert.equal(fns.triggerSummary({ Type: 'IntervalTrigger', IntervalTicks: 30 * MIN }), 'dashboard.every_minutes|{"count":30}');
    assert.equal(fns.triggerSummary({ Type: 'IntervalTrigger', IntervalTicks: 120 * MIN }), 'admin.every_hours|{"count":2}');
    assert.equal(fns.triggerSummary({ Type: 'StartupTrigger' }), 'dashboard.on_startup');
    assert.equal(fns.triggerSummary({ Type: '<b>' }), '&lt;b&gt;');
});

test('the triggers editor posts the whole list with a trigger added or removed', async () => {
    const task = { Id: 'LibraryScan', Name: 'Scan', Triggers: [{ Type: 'StartupTrigger' }, { Type: 'IntervalTrigger', IntervalTicks: 36000000000 }] };
    const editor = element({ showModal() { this.open = true; } });
    const s = sandbox({ api: { 'GET /ScheduledTasks/LibraryScan': task }, elements: { 'trigger-editor': editor } });
    await s.fns.openTriggerEditor('LibraryScan');
    assert.match(editor.innerHTML, /data-action="task-remove-trigger" data-index="1"/);
    assert.match(editor.innerHTML, /<option value="WeeklyTrigger">/);
    s.calls.length = 0;
    await s.fns.removeTaskTrigger(0);
    same(s.calls[0], ['POST', '/ScheduledTasks/LibraryScan/Triggers', [task.Triggers[1]]]);
    s.calls.length = 0;
    const form = { elements: { Type: { value: 'DailyTrigger' }, TimeOfDay: { value: '01:00' }, DayOfWeek: { value: 'Monday' }, Interval: { value: '60' } } };
    await s.fns.addTaskTrigger({ preventDefault() {}, target: form });
    same(s.calls[0], ['POST', '/ScheduledTasks/LibraryScan/Triggers', [...task.Triggers, { Type: 'DailyTrigger', TimeOfDayTicks: 36000000000 }]]);
    // Closing the editor reloads the task list
    editor.listeners.close();
    same(s.renders, ['tasks']);
    assert.match(admin, /data-action="edit-triggers"/);
});

test('a transcoding session shows its codecs and why it transcodes', () => {
    const slice = (head) => {
        const start = sessions.indexOf(head);
        assert.ok(start >= 0, head);
        return sessions.slice(start, sessions.indexOf('\n}\n', start) + 3);
    };
    const context = vm.createContext({ t: key => key, escapeHtml });
    vm.runInContext(`${slice('function reasonText(')}\n${slice('function transcodeDetails(')}\nthis.f = transcodeDetails;`, context);
    const html = context.f({
        IsVideoDirect: false, IsAudioDirect: true, VideoCodec: 'h264', AudioCodec: 'aac',
        HardwareAccelerationType: 'vaapi', TranscodeReasons: ['VideoCodecNotSupported', 'ContainerBitrateExceedsLimit'],
    });
    assert.match(html, /remote\.video: H264 · remote\.audio: remote\.direct · VAAPI/);
    assert.match(html, /Video codec not supported, Container bitrate exceeds limit/);
    assert.equal(context.f(undefined), '');
    assert.match(sessions, /ps\.PlayMethod === 'Transcode' \? transcodeDetails\(s\.TranscodingInfo\)/);
});

test('the inline handlers are global and every dashboard string is in en and de', () => {
    for (const name of ['saveGeneralSettings', 'addLibraryFolder', 'renameLibrary', 'addTaskTrigger', 'showTriggerFields']) {
        assert.match(app, new RegExp(`\\b${name}\\b[\\s\\S]*Object\\.assign\\(window, \\{[\\s\\S]*\\b${name}\\b`), name);
    }
    const used = new Set();
    for (const text of [dashboard, admin, sessions]) {
        for (const m of text.matchAll(/t\(['`](dashboard\.[a-z_]+|remote\.[a-z_]+)/g)) used.add(m[1]);
    }
    used.add('dashboard.restart');
    used.add('dashboard.shutdown_title');
    used.add('dashboard.restart_text');
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`));
        for (const key of used) {
            const [group, name] = key.split('.');
            const has = strings[group]?.[name] ?? strings[group]?.[`${name}_other`];
            assert.ok(has, `${lang}: ${key}`);
        }
    }
});
