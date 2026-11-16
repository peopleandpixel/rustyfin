// The web client's TV extras: series, season and episode pages load their special features and local
// trailers like movie pages, and show the Special Features row and the Trailer button.
// Checks item.js's wiring as text and runs its row renderer in a vm sandbox.
// Usage: node r3extras2_web.mjs (run by api_tests.rs r3extras2_web).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const item = fs.readFileSync(new URL('modules/item.js', src), 'utf8');

function slice(source, head, end = '\n}\n') {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `has ${head}`);
    return source.slice(start, source.indexOf(end, start) + end.length);
}

test('series, season and episode pages fetch their special features', () => {
    const line = item.match(/const showsSpecials = (\[[^\]]*\])\.includes\(type\);/);
    assert.ok(line, 'showsSpecials lists the item types');
    const types = JSON.parse(line[1].replace(/'/g, '"'));
    for (const type of ['Movie', 'Series', 'Season', 'Episode']) {
        assert.ok(types.includes(type), `${type} pages show special features`);
    }
    assert.match(item, /showsSpecials && item\.SpecialFeatureCount > 0 \? API\.getSpecialFeatures\(id\)/);
    // The Trailer button reads the local trailers of any item that has some.
    assert.match(item, /item\.LocalTrailerCount > 0 \? API\.getLocalTrailers\(id\)/);
});

test("an episode's special features render as a row that plays each one", () => {
    const context = vm.createContext({
        t: key => key,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        icon: name => `<i:${name}>`,
        formatRuntime: ticks => (ticks ? `${Math.round(ticks / 6e8)}m` : ''),
        API: { imageUrl: (id, type) => `/Items/${id}/Images/${type}` },
    });
    vm.runInContext(
        `${slice(item, 'const EXTRA_TYPE_KEYS = ', '\n};\n')}
         ${slice(item, 'function specialFeaturesHtml(')}
         this.render = specialFeaturesHtml;`,
        context,
    );
    const html = context.render([
        { Id: 'e1', Name: 'S01E01-featurette', ExtraType: 'Featurette', RunTimeTicks: 6e9 },
        { Id: 'e2', Name: 'Gag reel', ExtraType: 'Unknown' },
    ]);
    assert.match(html, /detail\.special_features/);
    assert.match(html, /data-action="play" data-id="e1" data-stream-type="Videos"/);
    assert.match(html, /data-id="e2"/);
    assert.match(html, /detail\.extra_featurette · 10m/);
    assert.equal(context.render([]), '', 'no row without extras');
});
