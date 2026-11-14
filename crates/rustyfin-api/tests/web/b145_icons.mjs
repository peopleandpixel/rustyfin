// B145: the web client draws its icons with Lucide, not colour emoji or hand-copied SVG.
// Checks app.js, index.html, icons.js and the translations as text, and runs the player's
// icon switches (updatePlayIcon, updateVolumeIcon) in a vm sandbox.
// Usage: node b145_icons.mjs PATH/TO/app.js (run by api_tests.rs b145_icons).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

const appPath = process.argv[2];
const srcDir = path.dirname(appPath);
const app = fs.readFileSync(appPath, 'utf8');
const html = fs.readFileSync(path.join(srcDir, 'index.html'), 'utf8');
const iconsJs = fs.readFileSync(path.join(srcDir, 'icons.js'), 'utf8');

// Colour emoji, plus the text glyphs the client used as icons.
const ICON_GLYPH = /[\p{Extended_Pictographic}★♥♡✓]/u;

// Drops // line comments, /* */ and <!-- --> comments, so a glyph in a comment doesn't count.
function stripComments(text) {
    return text
        .replace(/<!--[\s\S]*?-->/g, '')
        .replace(/\/\*[\s\S]*?\*\//g, '')
        .split('\n')
        .map(line => line.replace(/^\s*\/\/.*$/, ''))
        .join('\n');
}

function glyphLines(name, text) {
    return stripComments(text)
        .split('\n')
        .map((line, i) => [i + 1, line])
        .filter(([, line]) => ICON_GLYPH.test(line))
        .map(([n, line]) => `${name}:${n}: ${line.trim().slice(0, 100)}`);
}

function kebabToPascal(name) {
    return name.split('-').map(p => p.charAt(0).toUpperCase() + p.slice(1)).join('');
}

test('app.js and index.html use no emoji or glyph as an icon', () => {
    const found = [...glyphLines('app.js', app), ...glyphLines('index.html', html)];
    assert.deepEqual(found, []);
});

test('index.html has no inline svg; app.js only icon() and the scan ring', () => {
    assert.equal((html.match(/<svg/g) || []).length, 0, 'index.html <svg count');
    assert.equal((app.match(/<svg/g) || []).length, 2, 'app.js <svg count (icon() and scanRing)');
});

test('every icon app.js and index.html name is imported in icons.js', () => {
    const imports = iconsJs.match(/import\s*\{([^}]*)\}\s*from\s*'lucide'/);
    assert.ok(imports, "icons.js imports from 'lucide'");
    const imported = new Set(imports[1].split(',').map(s => s.trim()).filter(Boolean));
    const used = new Set();
    // Quoted PascalCase names inside icon(...) calls, ternaries and fallbacks included.
    for (const m of app.matchAll(/\bicon\(([^)]*)\)/g)) {
        for (const n of m[1].matchAll(/'([A-Z][A-Za-z0-9]+)'/g)) used.add(n[1]);
    }
    // Names held in maps that feed icon(), e.g. typeIcons = { Movie: 'Film' }.
    for (const m of app.matchAll(/(?:Icons?|icons?|ICONS?)\s*=\s*\{([^}]*)\}/g)) {
        for (const v of m[1].matchAll(/:\s*'([A-Z][A-Za-z0-9]+)'/g)) used.add(v[1]);
    }
    for (const m of html.matchAll(/data-lucide="([a-z0-9-]+)"/g)) used.add(kebabToPascal(m[1]));
    assert.ok(used.size >= 20, `found ${used.size} icon names; expected the whole client's set`);
    const missing = [...used].filter(n => !imported.has(n)).sort();
    assert.deepEqual(missing, [], 'icon names used but not imported in icons.js');
});

test('icons.js renders the static markup with createIcons', () => {
    assert.match(iconsJs, /createIcons\s*\(/);
    assert.match(iconsJs, /window\.lucideIcons\s*=/);
});

test('no translation value holds a colour emoji', () => {
    const dir = path.join(srcDir, 'translations');
    const found = [];
    const walk = (obj, prefix, file) => {
        for (const [k, v] of Object.entries(obj)) {
            if (v && typeof v === 'object') walk(v, `${prefix}${k}.`, file);
            // A colour emoji anywhere, or a glyph icon (✓ ○ ★ ♥) leading a label.
            else if (typeof v === 'string' && /\p{Extended_Pictographic}|^[✓○★♥] /u.test(v)) found.push(`${file} ${prefix}${k}: ${v}`);
        }
    };
    const files = fs.readdirSync(dir).filter(f => f.endsWith('.json'));
    assert.equal(files.length, 12, 'translation files');
    for (const f of files) walk(JSON.parse(fs.readFileSync(path.join(dir, f), 'utf8')), '', f);
    assert.deepEqual(found, []);
});

function slice(head) {
    const start = app.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return app.slice(start, app.indexOf('\n}\n', start) + 3);
}

const ICONS = Object.fromEntries(['Play', 'Pause', 'Volume1', 'Volume2', 'VolumeX'].map(n => [n, [['path', { d: `M-${n}` }]]]));

function playerSandbox() {
    const els = { 'player-play-icon': { innerHTML: '' }, 'player-vol-icon': { innerHTML: '' }, 'video-player': { muted: false, volume: 1 } };
    const context = vm.createContext({
        window: { lucideIcons: ICONS },
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        $: id => els[id] || null,
    });
    vm.runInContext([slice('function icon('), slice('function updatePlayIcon('), slice('function updateVolumeIcon(')].join('\n'), context);
    return { context, els };
}

test('updatePlayIcon switches between Lucide Play and Pause', () => {
    const { context, els } = playerSandbox();
    context.updatePlayIcon(true);
    assert.match(els['player-play-icon'].innerHTML, /M-Pause/);
    assert.doesNotMatch(els['player-play-icon'].innerHTML, /M-Play/);
    context.updatePlayIcon(false);
    assert.match(els['player-play-icon'].innerHTML, /M-Play/);
});

test('updateVolumeIcon shows VolumeX, Volume1 and Volume2', () => {
    const { context, els } = playerSandbox();
    const v = els['video-player'];
    const shown = () => els['player-vol-icon'].innerHTML.match(/M-(Volume\w+)/)?.[1];
    v.muted = true; context.updateVolumeIcon(); assert.equal(shown(), 'VolumeX');
    v.muted = false; v.volume = 0; context.updateVolumeIcon(); assert.equal(shown(), 'VolumeX');
    v.volume = 0.3; context.updateVolumeIcon(); assert.equal(shown(), 'Volume1');
    v.volume = 0.9; context.updateVolumeIcon(); assert.equal(shown(), 'Volume2');
});
