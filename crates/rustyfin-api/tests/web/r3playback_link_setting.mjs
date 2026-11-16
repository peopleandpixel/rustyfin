// The server settings form: its RefuseLinksOutOfLibraries checkbox shows the saved value and is
// sent back with the configuration. Runs handleSaveConfig in a vm sandbox with stubs.
// Usage: node r3playback_link_setting.mjs (run by api_tests.rs r3playback_web_link_setting).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const settings = fs.readFileSync(new URL('modules/settings.js', src), 'utf8');

function slice(text, head, end = '\n}\n') {
    const start = text.indexOf(head);
    assert.ok(start >= 0, `the client has ${head}`);
    return text.slice(start, text.indexOf(end, start) + end.length);
}

async function saved(checked) {
    const sent = [];
    const fields = { 'cfg-server-name': { value: 'R3' }, 'cfg-refuse-links': { checked } };
    const context = vm.createContext({
        $: id => fields[id],
        API: { updateConfig: async body => sent.push(body) },
        toast() {},
        t: key => key,
        renderSettings() {},
    });
    vm.runInContext(`${slice(settings, 'function providerKeyField(')}\n${slice(settings, 'async function handleSaveConfig(')}`, context);
    await vm.runInContext('handleSaveConfig', context)({ preventDefault() {} });
    assert.equal(sent.length, 1);
    return sent[0].RefuseLinksOutOfLibraries;
}

test('the checkbox is sent with the configuration', async () => {
    assert.equal(await saved(true), true);
    assert.equal(await saved(false), false);
});

test('the checkbox shows the saved value, with its help text in en and de', () => {
    assert.match(settings, /id="cfg-refuse-links"[^>]*\$\{config\.RefuseLinksOutOfLibraries \? 'checked' : ''\}/);
    for (const lang of ['en', 'de']) {
        const strings = JSON.parse(fs.readFileSync(new URL(`translations/${lang}.json`, src), 'utf8'));
        assert.ok(strings.settings.refuse_links_out, lang);
        assert.ok(strings.settings.refuse_links_out_help, lang);
    }
});
