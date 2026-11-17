// R3 users 2: the sign-in page's user tiles (a user without a password signs in at a click, the others get
// the form with their name), Manual login, Forgot password and its PIN step, and the user menu's Switch user,
// as Jellyfin Web's login, forgotpassword and forgotpasswordpin pages. Runs modules/auth.js in a vm sandbox
// with the DOM and API stubbed. Usage: node r3users2_login.mjs (run by api_tests.rs r3users2_login).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const WEB_SRC = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = file => fs.readFileSync(new URL(file, WEB_SRC), 'utf8');
const strip = source => source.replace(/^import [^;]*;$/gms, '').replace(/^export \{[^}]*\};$/gms, '');
const escapeHtml = s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

function fakeElement(props = {}) {
    return {
        value: '', hidden: false, disabled: false, textContent: '', className: '',
        focus() {}, addEventListener() {}, querySelector: () => fakeElement(), ...props,
    };
}

function sandbox({ users = [], api = {} } = {}) {
    const log = [];
    const keys = new Set();
    const elements = {};
    const pages = [];
    const consoleLines = [];
    const context = vm.createContext({
        URLSearchParams,
        t: (key, opts) => { keys.add(key); return opts ? `${key}|${JSON.stringify(opts)}` : key; },
        escapeHtml,
        $: id => elements[id] || (elements[id] = fakeElement()),
        html: (id, markup) => { pages.push(markup); for (const k of Object.keys(elements)) delete elements[k]; },
        show: () => {}, hide: () => {},
        toast: (msg, kind) => log.push(['toast', msg, kind || 'ok']),
        navigate: (page, data) => log.push(['navigate', page, data]),
        closeUserMenu: () => log.push(['close-menu']),
        loadUserSettings: () => {}, loadSidebar: async () => {},
        initScanProgressWebSocket: () => {}, stopScanProgressWebSocket: () => {},
        jiShownCodes: new Set(), jiChoice: new Map(), jiConfirm: new Map(),
        LiveLog: { disconnect() {} },
        console: { log: (...a) => consoleLines.push(a.join(' ')), warn: (...a) => consoleLines.push(a.join(' ')), error: (...a) => consoleLines.push(a.join(' ')) },
        window: { addEventListener() {} },
        document: { addEventListener() {}, querySelector: () => null, getElementById: () => null },
        Date,
        API: {
            base: '', userName: '', isAdmin: false,
            token: null,
            isLoggedIn() { return !!this.token; },
            async publicUsers() { return users; },
            async login(name, pw) { log.push(['login', name, pw]); this.token = 'tok'; this.userName = name; },
            logout() { log.push(['logout']); this.token = null; },
            async oidcInfo() { return { Enabled: false }; },
            ...api,
        },
    });
    vm.runInContext(`${strip(read('modules/auth.js'))}
        this.fns = { renderLogin, signInFromTile, loginTileHtml, submitForgotPassword, submitForgotPin, switchUser,
            renderForgotPin, getUsers: () => loginUsers };`, context);
    return { fns: context.fns, log, keys, elements, pages, consoleLines, api: context.API };
}

// As Jellyfin 12.4 sends them: HasPassword is always true
const USERS = [
    { Id: 'u1', Name: 'Karl', HasPassword: true, PrimaryImageTag: 'tag1' },
    { Id: 'u2', Name: 'Kid <3', HasPassword: true },
];

test('the sign-in page shows the public users as tiles, with Manual login and Forgot password', async () => {
    const box = sandbox({ users: USERS });
    box.fns.renderLogin();
    assert.match(box.pages[0], /id="login-form"/, 'the form comes first, so the page works at once');
    await tick();
    const tiles = box.pages.at(-1);
    assert.match(tiles, /data-login="user" data-user-id="u1"/);
    assert.match(tiles, /data-user-id="u2"/);
    assert.match(tiles, /Kid &lt;3/, 'names are escaped');
    assert.match(tiles, /\/UserImage\?userId=u1&amp;tag=tag1/, 'a tile shows the profile picture');
    assert.match(tiles, /data-login="manual"/);
    assert.match(tiles, /data-login="forgot"/);
    assert.doesNotMatch(tiles, /onclick="[^"]*Karl/, 'no name reaches inline JavaScript');
});

test('no public users: only the form, which still links Forgot password', async () => {
    const box = sandbox({ users: [] });
    box.fns.renderLogin();
    await tick();
    assert.equal(box.pages.length, 1);
    assert.match(box.pages[0], /id="login-form"/);
    assert.match(box.pages[0], /data-login="forgot"/);
    assert.doesNotMatch(box.pages[0], /data-login="users"/, 'no way back to tiles that are not there');
});

test('someone already typing keeps the form when the tiles arrive', async () => {
    let answer;
    const box = sandbox({ api: { publicUsers: () => new Promise(resolve => { answer = resolve; }) } });
    box.fns.renderLogin();
    box.elements['login-user'] = fakeElement({ value: 'kar' });
    answer(USERS);
    await tick();
    assert.equal(box.pages.length, 1);
});

test('every tile opens the form with the name filled in, as Jellyfin Web 12.4', async () => {
    for (const [id, name] of [['u1', 'Karl'], ['u2', 'Kid &lt;3']]) {
        const box = sandbox({ users: USERS });
        box.fns.renderLogin();
        await tick();
        // Even a user once known to have no password: the empty password is typed (left empty) in the form
        box.fns.getUsers()[1].HasPassword = false;
        await box.fns.signInFromTile(id);
        assert.ok(!box.log.some(e => e[0] === 'login'), 'no sign-in from the tile itself');
        const form = box.pages.at(-1);
        assert.match(form, new RegExp(`id="login-user"[^>]*value="${name}"`));
        assert.match(form, /id="login-pass"/);
        assert.match(form, /data-login="users"/, 'the form leads back to the tiles');
    }
});

const submitEvent = () => ({ preventDefault() {}, target: { querySelector: () => ({ disabled: false }) } });

test('Forgot password: on the local network the PIN step names the file the server wrote', async () => {
    const sent = [];
    const box = sandbox({
        api: {
            async forgotPassword(name) {
                sent.push(name);
                return { Action: 'PinCode', PinFile: '/data/passwordreset-ab.json', PinExpirationDate: '2026-11-16T12:00:00Z' };
            },
        },
    });
    box.fns.renderLogin('forgot');
    assert.match(box.pages.at(-1), /id="forgot-form"/);
    box.elements['forgot-user'] = fakeElement({ value: ' karl ' });
    await box.fns.submitForgotPassword(submitEvent());
    assert.deepEqual(sent, ['karl']);
    const pinPage = box.pages.at(-1);
    assert.match(pinPage, /id="pin-form"/);
    assert.match(pinPage, /passwordreset-ab\.json/);
});

test('Forgot password from outside the network asks to try again at home; 429 says to wait', async () => {
    for (const [answer, key] of [
        [async () => ({ Action: 'InNetworkRequired', PinFile: '' }), 'login.forgot_in_network'],
        [async () => ({ Action: 'ContactAdmin' }), 'login.forgot_contact_admin'],
        [async () => { const e = new Error('slow down'); e.status = 429; throw e; }, 'login.too_many_attempts'],
    ]) {
        const box = sandbox({ api: { forgotPassword: answer } });
        box.fns.renderLogin('forgot');
        const message = fakeElement();
        box.elements['forgot-user'] = fakeElement({ value: 'karl' });
        box.elements['forgot-message'] = message;
        await box.fns.submitForgotPassword(submitEvent());
        assert.equal(message.textContent, key);
        assert.equal(message.hidden, false);
        assert.doesNotMatch(box.pages.at(-1), /id="pin-form"/);
    }
});

test('the PIN step resets the password, clears the field, and never logs the PIN', async () => {
    const pins = [];
    const box = sandbox({ api: { async redeemResetPin(pin) { pins.push(pin); return { Success: true, UsersReset: ['karl'] }; } } });
    box.fns.renderLogin('forgot-pin');
    const input = fakeElement({ value: ' 1A-2B-3C-4D ' });
    box.elements['forgot-pin'] = input;
    await box.fns.submitForgotPin(submitEvent());
    assert.deepEqual(pins, ['1A-2B-3C-4D']);
    assert.equal(input.value, '', 'the PIN leaves the field');
    assert.match(box.pages.at(-1), /login\.pin_done_text\|\{&quot;names&quot;:&quot;karl&quot;\}/);
    assert.ok(!box.consoleLines.some(l => l.includes('1A-2B')), 'the PIN is never logged');
});

test('a failed PIN result reads as an invalid PIN, as in Jellyfin Web', async () => {
    const box = sandbox({ api: { async redeemResetPin() { return { Success: false, UsersReset: [] }; } } });
    box.fns.renderLogin('forgot-pin');
    const message = fakeElement();
    const input = fakeElement({ value: 'AA-BB-CC-DD' });
    box.elements['forgot-pin'] = input;
    box.elements['forgot-message'] = message;
    await box.fns.submitForgotPin(submitEvent());
    assert.equal(message.textContent, 'login.pin_invalid');
    assert.equal(input.value, '');
    assert.doesNotMatch(box.pages.at(-1), /pin_done/);
});

test('a wrong, expired or used PIN says so', async () => {
    const box = sandbox({ api: { async redeemResetPin() { const e = new Error('invalid or expired PIN'); e.status = 404; throw e; } } });
    box.fns.renderLogin('forgot-pin');
    const message = fakeElement();
    box.elements['forgot-pin'] = fakeElement({ value: 'AA-BB-CC-DD' });
    box.elements['forgot-message'] = message;
    await box.fns.submitForgotPin(submitEvent());
    assert.equal(message.textContent, 'login.pin_invalid');
});

test('Switch user signs out and opens the sign-in page', () => {
    const box = sandbox();
    box.api.token = 'tok';
    box.fns.switchUser();
    assert.deepEqual(box.log.map(e => e[0]), ['close-menu', 'logout', 'navigate']);
    assert.deepEqual(box.log.at(-1), ['navigate', 'login', undefined]);
    const index = read('index.html');
    assert.match(index, /onclick="switchUser\(\)"/);
    assert.match(index, /data-i18n="nav\.switch_user"/);
    assert.match(read('app.js'), /switchUser,/);
});

test('every string of the sign-in steps is in English and German', async () => {
    const box = sandbox({ users: USERS });
    box.fns.renderLogin();
    await tick();
    box.fns.renderLogin('forgot');
    box.fns.renderForgotPin('note');
    box.fns.loginTileHtml(USERS[0]);
    for (const extra of ['login.forgot_in_network', 'login.forgot_contact_admin', 'login.too_many_attempts', 'login.pin_invalid',
        'login.pin_done_title', 'login.pin_done_text', 'login.forgot_pin_written', 'login.choose_user', 'nav.switch_user']) {
        box.keys.add(extra);
    }
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(read(`translations/${lang}.json`));
        for (const key of box.keys) {
            assert.equal(typeof key.split('.').reduce((o, k) => o?.[k], strings), 'string', `${lang}: ${key}`);
        }
    }
    // Product text names no single sign-on vendor
    const en = JSON.stringify(JSON.parse(read('translations/en.json')).login);
    assert.doesNotMatch(en, /authelia|authentik|keycloak|okta|google/i);
});
