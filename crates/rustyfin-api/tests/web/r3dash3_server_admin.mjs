// R3 admin dashboard 3: the Networking, API keys, Devices, Branding and Backups tabs send
// Jellyfin's requests, a new API key is shown once and never logged, and the branding CSS and login
// disclaimer are applied. Runs server-admin.js and branding.js (imports and exports stripped) in a
// vm sandbox with stubs.
// Usage: node r3dash3_server_admin.mjs (run by api_tests.rs r3dash3_web_server_admin).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const strip = code => code.replace(/^import [\s\S]*?;\n/gm, '').replace(/^export \{[\s\S]*?\};\n?/m, '');
const serverAdmin = strip(read('modules/server-admin.js'));
const branding = strip(read('modules/branding.js'));

const same = (actual, expected, message) => assert.deepEqual(JSON.parse(JSON.stringify(actual ?? null)), expected ?? null, message);
const escapeHtml = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const settle = () => new Promise(resolve => setImmediate(resolve));

function element(props = {}) {
    const classes = new Set(['hidden']);
    return {
        innerHTML: '', textContent: '', value: '', checked: false, listeners: {},
        classList: { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)), contains: c => classes.has(c) },
        addEventListener(type, fn) { this.listeners[type] = fn; },
        querySelector() { return this.form || null; },
        ...props,
    };
}

function sandbox({ api = {}, answer = true, promptAnswer = null } = {}) {
    const calls = [];
    const toasts = [];
    const logs = [];
    const powers = [];
    let brandingApplied = 0;
    const els = {};
    for (const id of ['admin-network', 'admin-apikeys', 'admin-devices', 'admin-branding', 'admin-backups', 'api-key-new', 'api-key-value']) {
        els[id] = element({ form: element() });
    }
    const record = method => async (path, body) => {
        calls.push([method, path, body]);
        const answerFor = api[`${method} ${path}`];
        if (answerFor instanceof Error) throw answerFor;
        return answerFor;
    };
    const quiet = (...args) => logs.push(args.join(' '));
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml,
        $: id => els[id] || null,
        toast: (message, kind) => toasts.push([message, kind]),
        confirmDialog: async () => answer,
        serverPower: kind => powers.push(kind),
        applyBrandingCss: () => { brandingApplied += 1; },
        prompt: () => promptAnswer,
        localStorage: { getItem: () => 'me' },
        navigator: {},
        console: { log: quiet, warn: quiet, error: quiet, info: quiet, debug: quiet },
        setTimeout,
        API: { get: record('GET'), post: record('POST'), del: record('DELETE'), base: '', headers: () => ({}) },
    });
    vm.runInContext(`${serverAdmin}\nthis.fns = { readNetwork, readBranding, networkHtml, apiKeysHtml, devicesHtml, backupsHtml,
        loadNetwork, saveNetwork, loadApiKeys, createApiKey, revokeApiKey, loadDevices, renameDevice, deleteDevice,
        loadBranding, saveBranding, loadBackups, createBackup, restoreBackup, deleteBackup, NETWORK_FIELDS,
        SERVER_ADMIN_TABS, SERVER_ADMIN_LOADERS };`, context);
    return { fns: context.fns, calls, toasts, logs, powers, els, brandingApplied: () => brandingApplied };
}

// A fake form whose elements are `values`: booleans are checkboxes, the rest values
function form(values) {
    const elements = {};
    for (const [name, value] of Object.entries(values)) {
        elements[name] = typeof value === 'boolean' ? { checked: value } : { value: String(value) };
    }
    return { elements };
}

test('the Networking tab saves its fields over the network configuration, keeping the others', async () => {
    const stored = { InternalHttpPort: 8096, KnownProxies: [], EnableRemoteAccess: true, SomeFutureField: 'kept' };
    const s = sandbox({ api: { 'GET /System/Configuration/network': stored } });
    await s.fns.loadNetwork();
    assert.equal(s.calls[0][1], '/System/Configuration/network');
    const html = s.els['admin-network'].innerHTML;
    assert.match(html, /server_admin\.needs_restart/);
    assert.match(html, /server_admin\.stored_only/);
    assert.match(html, /name="InternalHttpPort"[^>]*value="8096"/);

    const f = form({
        InternalHttpPort: '8300', KnownProxies: '10.0.0.0/8\n192.168.1.2, 172.16.0.1', LocalNetworkAddresses: '',
        EnableRemoteAccess: false, BaseUrl: ' /jf ', PublishedServerUriBySubnet: 'all=https://media.example',
    });
    await s.fns.saveNetwork({ preventDefault() {}, target: f });
    const post = s.calls.find(c => c[0] === 'POST');
    assert.equal(post[1], '/System/Configuration/network');
    assert.equal(post[2].InternalHttpPort, 8300);
    same(post[2].KnownProxies, ['10.0.0.0/8', '192.168.1.2', '172.16.0.1']);
    same(post[2].LocalNetworkAddresses, []);
    same(post[2].PublishedServerUriBySubnet, ['all=https://media.example']);
    assert.equal(post[2].EnableRemoteAccess, false);
    assert.equal(post[2].BaseUrl, '/jf');
    assert.equal(post[2].SomeFutureField, 'kept');
    same(s.toasts, [['server_admin.network_saved', null]]);
});

test('the fields marked "after a restart" are the ones the server reads at its start', () => {
    const { fns } = sandbox();
    const restart = fns.NETWORK_FIELDS.filter(f => f[2] === 'restart').map(f => f[0]).sort();
    same(restart, ['InternalHttpPort', 'KnownProxies', 'LocalNetworkAddresses', 'PublishedServerUriBySubnet']);
    const server = fs.readFileSync(new URL('../../src/named_config.rs', import.meta.url), 'utf8');
    for (const field of restart) assert.ok(server.includes(`"${field}"`), field);
});

test('a new API key is shown once, as text, and never logged; a key is revoked by its id', async () => {
    const secret = 'abcdef0123456789abcdef0123456789';
    const s = sandbox({
        api: {
            'POST /Auth/Keys?app=My%20App': { Id: 'k1', AccessToken: secret },
            'GET /Auth/Keys': { Items: [{ Id: 'k1', AccessToken: '…6789', AppName: 'My App', DateCreated: '2026-11-16T10:00:00Z' }] },
        },
    });
    await s.fns.createApiKey({ preventDefault() {}, target: form({ app: ' My App ' }) });
    assert.ok(s.calls.some(c => c[0] === 'POST' && c[1] === '/Auth/Keys?app=My%20App'));
    assert.equal(s.els['api-key-value'].textContent, secret);
    assert.ok(!s.els['api-key-new'].innerHTML.includes(secret), 'the key is set as text, not markup');
    assert.ok(!s.els['admin-apikeys'].innerHTML.includes(secret), 'the list shows only the hint');
    assert.match(s.els['admin-apikeys'].innerHTML, /…6789/);
    assert.ok(!s.logs.some(l => l.includes(secret)), 'never logged');
    // Loading the tab again shows no key
    await s.fns.loadApiKeys();
    assert.ok(!s.els['admin-apikeys'].innerHTML.includes(secret));

    await s.fns.revokeApiKey({ dataset: { id: 'k1', name: 'My App' } });
    assert.ok(s.calls.some(c => c[0] === 'DELETE' && c[1] === '/Auth/Keys/k1'));
    const cancelled = sandbox({ answer: false });
    await cancelled.fns.revokeApiKey({ dataset: { id: 'k1', name: 'My App' } });
    same(cancelled.calls, []);
});

test('the Devices tab lists devices, renames one and deletes another, never this browser', async () => {
    const devices = [
        { Id: 'web-me', Name: 'Browser', AppName: 'Rustyfin Web', AppVersion: '12.0.0', LastUserName: 'admin', DateLastActivated: '2026-11-16T10:00:00Z' },
        { Id: 'tv-1', Name: 'Living room TV', CustomName: 'TV', AppName: 'Jellyfin Android TV', LastUserName: 'kid' },
    ];
    const s = sandbox({ api: { 'GET /Devices': { Items: devices, TotalRecordCount: 2 } }, promptAnswer: ' Bedroom ' });
    await s.fns.loadDevices();
    const html = s.els['admin-devices'].innerHTML;
    assert.match(html, /server_admin\.this_device/);
    assert.match(html, /data-sa="delete-device" data-id="tv-1"/);
    assert.doesNotMatch(html, /data-sa="delete-device" data-id="web-me"/);
    assert.match(html, />TV/);
    assert.match(html, /Living room TV/);

    await s.fns.renameDevice({ dataset: { id: 'tv-1', name: 'TV' } });
    same(s.calls.find(c => c[0] === 'POST'), ['POST', '/Devices/Options?id=tv-1', { CustomName: 'Bedroom' }]);
    await s.fns.deleteDevice({ dataset: { id: 'tv-1', name: 'TV' } });
    assert.ok(s.calls.some(c => c[0] === 'DELETE' && c[1] === '/Devices?id=tv-1'));
});

test('the Branding tab saves the disclaimer, CSS and splash screen, and applies the CSS', async () => {
    const s = sandbox({ api: { 'GET /System/Configuration/branding': { LoginDisclaimer: 'Hi', CustomCss: '', SplashscreenEnabled: false } } });
    await s.fns.loadBranding();
    assert.match(s.els['admin-branding'].innerHTML, /name="LoginDisclaimer"[^>]*>Hi</);
    const f = form({ LoginDisclaimer: 'Private <server>', CustomCss: 'body { color: red; }', SplashscreenEnabled: true });
    await s.fns.saveBranding({ preventDefault() {}, target: f });
    same(s.calls.find(c => c[0] === 'POST'), ['POST', '/System/Configuration/branding',
        { LoginDisclaimer: 'Private <server>', CustomCss: 'body { color: red; }', SplashscreenEnabled: true }]);
    assert.equal(s.brandingApplied(), 1);
});

test('the Backups tab creates, restores (then offers a restart) and deletes backups', async () => {
    const backups = [{ Path: 'rustyfin-backup-20261116100000.db', DateCreated: '2026-11-16T10:00:00Z', ServerVersion: '0.9.0' }];
    const s = sandbox({ api: { 'GET /Backup': backups } });
    await s.fns.loadBackups();
    assert.match(s.els['admin-backups'].innerHTML, /data-sa="download-backup" data-path="rustyfin-backup-20261116100000\.db"/);
    await s.fns.createBackup({ disabled: false });
    same(s.calls.find(c => c[0] === 'POST'), ['POST', '/Backup/Create', { Metadata: false, Trickplay: false, Subtitles: false, Database: true }]);
    await s.fns.restoreBackup({ dataset: { path: backups[0].Path } });
    same(s.calls.filter(c => c[1] === '/Backup/Restore'), [['POST', '/Backup/Restore', { ArchiveFileName: backups[0].Path }]]);
    same(s.powers, ['restart']);
    await s.fns.deleteBackup({ dataset: { path: backups[0].Path } });
    assert.ok(s.calls.some(c => c[0] === 'DELETE' && c[1] === `/Backup/${backups[0].Path}`));
});

test('the dashboard has the five tabs, each loaded when shown', () => {
    const { fns } = sandbox();
    same(fns.SERVER_ADMIN_TABS, ['admin-network', 'admin-apikeys', 'admin-devices', 'admin-branding', 'admin-backups']);
    same(Object.keys(fns.SERVER_ADMIN_LOADERS), [...JSON.parse(JSON.stringify(fns.SERVER_ADMIN_TABS))]);
    const admin = read('modules/admin.js');
    const tabs = admin.slice(admin.indexOf('const ADMIN_TABS = ['), admin.indexOf('];', admin.indexOf('const ADMIN_TABS = [')));
    for (const id of fns.SERVER_ADMIN_TABS) assert.ok(tabs.includes(`'${id}'`), id);
    assert.match(admin, /SERVER_ADMIN_LOADERS\[tabId\]\?\.\(\)/);
    assert.match(admin, /serverAdminPanelsHtml\(\)/);
    assert.match(read('app.js'), /import '\.\/modules\/server-admin\.js';/);
});

test('the branding CSS goes into a style element, and the login disclaimer is shown as text', async () => {
    const head = [];
    const byId = {};
    const document = {
        head: { appendChild: el => { head.push(el); byId[el.id] = el; } },
        getElementById: id => byId[id] || null,
        createElement: () => ({ id: '', textContent: '' }),
    };
    const answers = {
        '/Branding/Css': { ok: true, text: async () => 'body { background: black; }' },
        '/Branding/Configuration': { ok: true, json: async () => ({ LoginDisclaimer: '<b>Only family</b>' }) },
    };
    const context = vm.createContext({ document, API: { base: '' }, fetch: async url => answers[url] });
    vm.runInContext(`${branding}\nthis.fns = { applyBrandingCss, showLoginDisclaimer };`, context);
    await settle();
    assert.equal(head.length, 1);
    assert.equal(head[0].id, 'rf-branding-css');
    assert.equal(head[0].textContent, 'body { background: black; }');
    await context.fns.applyBrandingCss();
    assert.equal(head.length, 1, 'one style element, updated');
    const el = element();
    await context.fns.showLoginDisclaimer(el);
    assert.equal(el.textContent, '<b>Only family</b>');
    assert.equal(el.innerHTML, '');
    assert.equal(el.classList.contains('hidden'), false);
    const auth = read('modules/auth.js');
    // In the sign-in card, filled each time the sign-in step is drawn
    assert.match(auth, /<p id="login-disclaimer"/);
    assert.match(auth, /loadOidcSignIn\(\);\n\s*showLoginDisclaimer\(\$\('login-disclaimer'\)\);/);
});

test('the dashboard\'s task list follows a library scan that starts or ends, wherever it started', () => {
    const libraryAdmin = read('modules/library-admin.js');
    const from = libraryAdmin.indexOf('// One server message');
    const to = libraryAdmin.indexOf('// After a sign-in');
    // remote.js's session socket: its subscribers by message type
    const listeners = new Map();
    const refreshes = [];
    const rings = [];
    const context = vm.createContext({
        onSocketMessage: (type, fn) => listeners.set(type, [...(listeners.get(type) || []), fn]),
        queueMicrotask: fn => fn(),
        updateScanRings: id => rings.push(id),
        refreshTasks: quiet => refreshes.push(quiet),
        currentPage: 'admin',
        toast: () => {}, t: k => k, libraries: [], refreshViewAfterLibraryChange: () => {},
        console,
    });
    vm.runInContext(`let scanStatuses = { stale: { percent: 5 } };\n${libraryAdmin.slice(from, to)}\nthis.fns = { statuses: () => scanStatuses };`, context);
    // No socket of its own: it listens on the session socket
    assert.doesNotMatch(libraryAdmin, /new WebSocket\(/);
    const emit = message => (listeners.get(message.MessageType) || []).forEach(fn => fn(message.Data, message));
    listeners.get('SocketOpen').forEach(fn => fn());
    same(context.fns.statuses(), {}, 'a (re)connect starts the rings over');
    const frame = (id, progress, state) => emit({ MessageType: 'RefreshProgress', Data: { ItemId: id, Progress: String(progress), State: state } });
    frame('tv', 0, 'Queued');
    same(refreshes, [true], 'a scan that starts');
    frame('tv', 40, 'Active');
    same(refreshes, [true], 'not on every percent');
    frame('tv', 100, 'Idle');
    same(refreshes, [true, true], 'a scan that ends');
    same(context.fns.statuses(), {}, 'its ring cleared');
    context.currentPage = 'home';
    frame('tv', 0, 'Queued');
    same(refreshes, [true, true], 'only on the dashboard');
    // Task events carry their fields at the top, without Data
    context.currentPage = 'admin';
    emit({ MessageType: 'ScheduledTaskCompleted', Name: 'Library scan: TV', State: 'Completed', ItemId: 'tv' });
    assert.equal(refreshes.length, 3, 'a task that ends refreshes the list');
});

test('the drawer lists the user\'s views for everyone, administrators too', async () => {
    const libraryAdmin = read('modules/library-admin.js');
    const from = libraryAdmin.indexOf('async function loadSidebar(');
    const to = libraryAdmin.indexOf('\n}\n', from) + 3;
    const calls = [];
    const list = { innerHTML: '' };
    const all = [{ id: 'a', name: 'Movies A', content_type: 'Movies' }, { id: 'h', name: 'Hidden H', content_type: 'Mixed' }];
    const context = vm.createContext({
        API: {
            isAdmin: true,
            isLoggedIn: () => true,
            getUserViews: async ({ includeHidden = false } = {}) => { calls.push(includeHidden); return includeHidden ? all : all.slice(0, 1); },
            getLibraries: async () => { throw new Error('the drawer must not read VirtualFolders'); },
        },
        $: id => (id === 'sidebar-lib-list' ? list : null),
        show: () => {}, hide: () => {}, toast: () => {}, t: k => k, escapeHtml, icon: () => '',
        scanRing: () => '', currentLibraryId: null, currentPage: 'home',
    });
    vm.runInContext(`let libraries = [];\n${libraryAdmin.slice(from, to)}\nthis.fns = { loadSidebar, libs: () => libraries };`, context);
    await context.fns.loadSidebar();
    same(calls.sort(), [false, true]);
    assert.match(list.innerHTML, /Movies A/);
    assert.doesNotMatch(list.innerHTML, /Hidden H/, 'hidden from My Media, hidden here');
    same(context.fns.libs().map(l => l.id), ['a', 'h'], 'the pages still find a hidden library');
});
