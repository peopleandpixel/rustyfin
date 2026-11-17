// What the Jellyfin Android app needs of the page itself: a classic main.*.bundle.js script it can swap for its
// loader (else it never shows the page and gives up after 10 s), no service worker between it and the
// requests it watches, no page path under /native/ (the app answers those from its own files), the app's
// device in the auth header, and the player's hand-off guarded so a browser plays as before.
// Usage: node r3android_page.mjs (run by api_tests.rs r3android_page).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const html = read('index.html');

// jellyfin-android utils/Constants.kt MAIN_BUNDLE_PATH_REGEX, matched against the lowercased path
const MAIN_BUNDLE_PATH_REGEX = /^.*\/main\.[^/\s]+\.bundle\.js$/;

test('index.html loads a main bundle the app recognises, as a classic script', () => {
    const tags = [...html.matchAll(/<script\b([^>]*)\bsrc="([^"]+)"([^>]*)>/g)]
        .filter(([, , path]) => MAIN_BUNDLE_PATH_REGEX.test(new URL(path, 'http://h/web/').pathname.toLowerCase()));
    assert.equal(tags.length, 1, 'one script matches the app');
    const [, before, path, after] = tags[0];
    assert.doesNotMatch(before + after, /type="module"|\basync\b/, 'the app loader reads document.currentScript');
    assert.ok(path.startsWith('/web/'), 'absolute, as the app keeps its query');
    const file = path.replace(/^\/web\//, 'public/');
    assert.ok(fs.existsSync(new URL(file, src)), `${file} is served at ${path}`);
    // It sits in the body: the loader appends its scripts to document.body
    assert.ok(html.indexOf(path) > html.indexOf('<body'));
});

test('no service worker in the app', () => {
    assert.match(html, /'serviceWorker' in navigator && !window\.NativeInterface/);
});

test('the page uses no /native/ path', () => {
    const files = ['api.js', 'app.js', 'index.html', ...fs.readdirSync(new URL('modules/', src)).map(f => `modules/${f}`)];
    for (const file of files) {
        const code = read(file).replace(/\/\/.*$/gm, '');
        assert.doesNotMatch(code, /['"`]\/native\//, `${file} requests nothing under /native/`);
    }
});

test("the user menu has the app's entries, hidden until the app offers them", () => {
    for (const feature of ['clientsettings', 'downloadmanagement', 'multiserver', 'exitmenu']) {
        const tag = html.match(new RegExp(`<a [^>]*data-native-feature="${feature}"[^>]*>`))?.[0];
        assert.ok(tag, feature);
        assert.match(tag, /class="hidden /);
        assert.match(tag, new RegExp(`nativeShellCommand\\('${feature}'\\)`));
    }
    assert.match(html, /id="native-menu" class="hidden /);
    assert.match(read('app.js'), /nativeShellCommand,\n\}\);/, 'the handler is a window global');
    for (const lang of ['en', 'de']) {
        const native = JSON.parse(read(`translations/${lang}.json`)).native;
        for (const key of ['client_settings', 'downloads', 'select_server', 'exit', 'download', 'download_started']) {
            assert.ok(native?.[key], `${lang}: native.${key}`);
        }
    }
});

test("the auth header names the app's device in the app, and the browser's elsewhere", () => {
    const store = new Map([['rustyfin_device_id', 'dev-1']]);
    const context = vm.createContext({
        window: { location: { origin: 'http://127.0.0.1:8250' } },
        localStorage: { getItem: k => store.get(k) ?? null, setItem: (k, v) => store.set(k, v) },
    });
    vm.runInContext(`${read('api.js')}\nthis.API = API;`, context);
    assert.match(context.API.authHeader(), /Client="Rustyfin Web", Device="Browser", DeviceId="web-dev-1"/);
    context.window.NativeShell = {
        AppHost: {
            appName: () => 'Jellyfin Android', appVersion: () => '2.6.3', deviceId: () => 'abc123', deviceName: () => 'Phone "A"',
        },
    };
    assert.equal(
        context.API.authHeader(),
        'MediaBrowser Client="Jellyfin Android", Device="Phone A", DeviceId="abc123", Version="2.6.3"',
    );
});

test("playItem hands videos to the app's player only when the app chose one", () => {
    const player = read('modules/player.js');
    const body = player.slice(player.indexOf('async function playItem('));
    const hook = body.indexOf('playNatively(');
    assert.ok(hook > 0);
    const line = body.slice(body.lastIndexOf('\n', hook), body.indexOf('\n', hook));
    assert.match(line, /streamType === 'Videos' && nativeVideoPlayer\(\) && await playNatively\(/, 'no await in a browser');
    assert.ok(hook < body.indexOf('modal.showModal'), 'before the page player opens');
    assert.ok(hook > body.indexOf('stopPlayback();'), 'after the page player stops what it played');
    // The web player's notification
    assert.match(player, /function updateMediaSession\(session, item, title\) \{\n {4}setNativeNowPlaying\(session, \(\) => mediaSessionMetadata\(item, title\)\);/);
    assert.match(player, /function clearMediaSession\(\) \{\n {4}setNativeNowPlaying\(null\);/);
});
