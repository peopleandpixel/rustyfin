// R3 parental: the dashboard's DLNA tab shows the DLNA switch and a user picker, warns while no
// user is chosen, and saves both into the whole server configuration it loaded.
// Runs dlna-admin.js in a vm sandbox with its imports stubbed.
// Usage: node r3parental_dlna_admin.mjs (run by api_tests.rs r3parental_web_dlna_admin).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const module = read('modules/dlna-admin.js')
    .replace(/^import .*$/gm, '')
    .replace(/^export \{[\s\S]*?\};$/m, '');
const en = JSON.parse(read('translations/en.json'));

const KID = '0a1b2c3d-0000-4000-8000-000000000001';
const users = [
    { Id: 'aaaaaaaa-0000-4000-8000-000000000001', Name: 'Owner <1>', Policy: { IsDisabled: false } },
    { Id: KID, Name: 'Kid', Policy: { IsDisabled: false } },
    { Id: 'aaaaaaaa-0000-4000-8000-000000000003', Name: 'Gone', Policy: { IsDisabled: true } },
];

// A sandbox with the module loaded; `elements` are what `$` finds by id, `api` stands in for API.
function sandbox(elements = {}, api = {}) {
    const toasts = [];
    const context = vm.createContext({
        t: (key, vars = {}) => {
            const [section, name] = key.split('.');
            const text = en[section]?.[name];
            assert.ok(text !== undefined, `en.json has ${key}`);
            return text.replace(/\{\{(\w+)\}\}/g, (_, k) => vars[k] ?? '');
        },
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        $: id => elements[id] ?? null,
        show: id => elements[id]?.classList.toggle('hidden', false),
        hide: id => elements[id]?.classList.toggle('hidden', true),
        toast: message => toasts.push(message),
        API: api,
    });
    vm.runInContext(
        `${module}\nthis.mod = { dlnaPanelHtml, dlnaSettingsHtml, readDlnaSettings, loadDlnaAdmin, saveDlnaSettings };`,
        context,
    );
    return { mod: context.mod, toasts };
}

// The selected option's value in the markup
function selectedUser(markup) {
    const options = [...markup.matchAll(/<option value="([^"]*)"( selected)?>/g)];
    return options.filter(o => o[2]).map(o => o[1]);
}

// A form the reader reads: the checkbox and the select
function form(enabled, user) {
    const fields = { '#dlna-enabled': { checked: enabled }, '#dlna-user': { value: user } };
    return { querySelector: sel => fields[sel] };
}

function element() {
    const classes = new Set(['hidden']);
    return {
        innerHTML: '',
        textContent: '',
        classList: { toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)), contains: c => classes.has(c) },
    };
}

test('the form lists every user, escaped, with no user first and the stored one selected', () => {
    const { mod } = sandbox();
    const html = mod.dlnaSettingsHtml({ EnableDlnaServer: true, DlnaDefaultUserId: KID.replace(/-/g, '').toUpperCase() }, users);
    assert.match(html, /id="dlna-enabled"[^>]* checked/);
    assert.deepEqual(selectedUser(html), [KID], 'the stored id matches whatever its spelling');
    assert.ok(html.includes('Owner &lt;1&gt;') && !html.includes('Owner <1>'));
    assert.ok(html.includes('Gone (disabled)'));
    assert.ok(html.indexOf('value=""') < html.indexOf(`value="${users[0].Id}"`), 'no user comes first');
    assert.ok(html.includes(en.dlna_admin.user_hint));
    assert.ok(!html.includes('dlna-no-user'), 'no warning with a user chosen');
    assert.ok(html.includes('data-action="dlna-save"'));
});

test('without a user, or with one that is gone, no user is selected and the tab warns', () => {
    const { mod } = sandbox();
    for (const id of [null, 'bbbbbbbb-0000-4000-8000-000000000009']) {
        const html = mod.dlnaSettingsHtml({ EnableDlnaServer: false, DlnaDefaultUserId: id }, users);
        assert.deepEqual(selectedUser(html), [''], String(id));
        assert.ok(html.includes(en.dlna_admin.no_user_warning), String(id));
        assert.doesNotMatch(html, /id="dlna-enabled"[^>]* checked/);
    }
});

test('the reader keeps the configuration and stores no user as null', () => {
    const { mod } = sandbox();
    const config = { ServerName: 'Box', EnableDlnaServer: false, DlnaDefaultUserId: null, QuickConnectAvailable: true };
    const values = mod.readDlnaSettings(form(true, KID), config);
    assert.deepEqual({ ...values }, { ...config, EnableDlnaServer: true, DlnaDefaultUserId: KID });
    const cleared = mod.readDlnaSettings(form(false, ''), { ...config, DlnaDefaultUserId: KID });
    assert.equal(cleared.DlnaDefaultUserId, null);
    assert.equal(cleared.ServerName, 'Box');
});

test('load and save post the whole configuration back through the config route', async () => {
    const box = element();
    const error = element();
    const warning = element();
    warning.classList.toggle('hidden', false);
    const posted = [];
    const config = { ServerName: 'Box', EnableDlnaServer: false, DlnaDefaultUserId: null };
    const api = {
        getConfig: async () => ({ ...config }),
        getUsers: async () => users,
        updateConfig: async cfg => { posted.push(cfg); },
    };
    const elements = { 'admin-dlna': box, 'dlna-settings-error': error, 'dlna-no-user': warning };
    const { mod, toasts } = sandbox(elements, api);
    await mod.loadDlnaAdmin();
    assert.ok(box.innerHTML.includes('id="dlna-settings"'));
    elements['dlna-settings'] = form(true, KID);
    await mod.saveDlnaSettings();
    assert.deepEqual(posted.map(p => ({ ...p })), [{ ServerName: 'Box', EnableDlnaServer: true, DlnaDefaultUserId: KID }]);
    assert.deepEqual(toasts, [en.dlna_admin.saved]);
    assert.ok(warning.classList.contains('hidden'), 'the warning goes once a user is saved');

    // A refused save says so and keeps the form.
    api.updateConfig = async () => { throw new Error('nope'); };
    await mod.saveDlnaSettings();
    assert.ok(!error.classList.contains('hidden'));
    assert.ok(error.textContent.includes('nope'));
});

test('the dashboard has the tab, loads it, and the save button is wired', () => {
    const admin = read('modules/admin.js');
    assert.match(admin, /import \{ dlnaPanelHtml, loadDlnaAdmin \} from '\.\/dlna-admin\.js';/);
    assert.match(admin, /'admin-dlna',\s*\];/, 'last in the tab list');
    assert.match(admin, /t\('dlna_admin\.tab'\)\];/, 'its label last too');
    assert.match(admin, /\$\{dlnaPanelHtml\(\)\}/);
    assert.match(admin, /if \(tabId === 'admin-dlna'\) loadDlnaAdmin\(\);/);
    assert.match(read('modules/router.js'), /'dlna-save': \(\) => saveDlnaSettings\(\),/);
    assert.match(read('app.js'), /import '\.\/modules\/dlna-admin\.js';/);
    const de = JSON.parse(read('translations/de.json'));
    assert.deepEqual(Object.keys(de.dlna_admin).sort(), Object.keys(en.dlna_admin).sort());
});
