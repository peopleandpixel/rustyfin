// The item and person pages drop a response for a page the user has already left: two quick
// navigations, the first response arriving last, and the second page stays.
// Runs renderDetail and renderPerson in a vm sandbox with a controllable API.
// Usage: node r3item2_stale.mjs (run by api_tests.rs r3item2_web_stale).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const item = fs.readFileSync(new URL('modules/item.js', src), 'utf8');

function slice(text, head, end = '\n}\n') {
    const start = text.indexOf(head);
    assert.ok(start >= 0, `has ${head}`);
    return text.slice(start, text.indexOf(end, start) + end.length);
}

// A promise the test settles by hand
function deferred() {
    let resolve;
    let reject;
    const promise = new Promise((res, rej) => { resolve = res; reject = rej; });
    return { promise, resolve, reject };
}

const pages = { 'page-detail': { innerHTML: '' }, 'page-person': { innerHTML: '' } };
const pending = { item: {}, person: {}, personItems: {} };
const want = (kind, id) => (pending[kind][id] = deferred()).promise;
const context = vm.createContext({
    $: id => pages[id],
    t: key => key,
    escapeHtml: s => String(s),
    API: {
        token: 'tok',
        getItem: id => want('item', id),
        getPerson: id => want('person', id),
        getPersonItems: id => want('personItems', id),
    },
});
vm.runInContext(
    `let currentDetailId = null;
     let activeItem = null;
     let detailChoice = null;
     ${slice(item, 'let detailRender = ', ';\n')}
     ${slice(item, 'let personRender = ', ';\n')}
     ${slice(item, 'async function renderDetail(')}
     ${slice(item, 'async function renderPerson(')}
     this.fns = { renderDetail, renderPerson, active: () => activeItem };`,
    context,
);
const fns = context.fns;
const tick = () => new Promise(resolve => setImmediate(resolve));

test('a slow item response does not draw over the next item', async () => {
    const first = fns.renderDetail('a');
    const second = fns.renderDetail('b');
    // The second item's response comes first (a failure, which draws without other helpers)
    pending.item.b.reject(new Error('second item drawn'));
    await second;
    assert.match(pages['page-detail'].innerHTML, /second item drawn/);
    // The first item's response comes last: dropped, success or failure
    pending.item.a.resolve({ Id: 'a', Name: 'First', Type: 'Movie' });
    await first;
    await tick();
    assert.match(pages['page-detail'].innerHTML, /second item drawn/);
    assert.equal(fns.active(), null, 'the left item is not made the active one');

    const third = fns.renderDetail('c');
    const fourth = fns.renderDetail('d');
    pending.item.d.reject(new Error('fourth item drawn'));
    await fourth;
    pending.item.c.reject(new Error('third item failed late'));
    await third;
    assert.match(pages['page-detail'].innerHTML, /fourth item drawn/);
});

test('a slow person response does not draw over the next person', async () => {
    const first = fns.renderPerson('p1');
    const second = fns.renderPerson('p2');
    pending.personItems.p2.resolve({ Items: [] });
    pending.person.p2.reject(new Error('second person drawn'));
    await second;
    assert.match(pages['page-person'].innerHTML, /second person drawn/);
    pending.personItems.p1.resolve({ Items: [] });
    pending.person.p1.reject(new Error('first person failed late'));
    await first;
    await tick();
    assert.match(pages['page-person'].innerHTML, /second person drawn/);
});

test('the guards are in the page code', () => {
    assert.equal((slice(item, 'async function renderDetail(').match(/if \(stale\(\)\) return;/g) || []).length, 3);
    assert.equal((slice(item, 'async function renderPerson(').match(/if \(stale\(\)\) return;/g) || []).length, 2);
});
