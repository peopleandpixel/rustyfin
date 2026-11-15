// B152 round 2: the library's position-based page fetching. Runs app.js's libraryCellRange and
// libraryPagePlan in a vm sandbox.
// Usage: node b152_library_pages.mjs PATH/TO/app.js (run by api_tests.rs b152_library_pages).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
const fn = head => {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `app.js has ${head}`);
    return source.slice(start, source.indexOf('\n}\n', start) + 3);
};
const context = vm.createContext({});
vm.runInContext(fn('function libraryCellRange(') + fn('function libraryPagePlan('), context);
const { libraryCellRange, libraryPagePlan } = context;
const range = (...a) => ({ ...libraryCellRange(...a) });
const plan = (...a) => [...libraryPagePlan(...a)];

// 7 columns, rows 300 px apart, 3,654 items (522 rows), 50-item pages (74 pages)
const COLS = 7, ROW = 300, TOTAL = 3654, PAGE = 50;

test('the cells of a band are whole rows, clamped to the list', () => {
    assert.deepEqual(range(0, 800, ROW, COLS, TOTAL), { first: 0, last: 20 }); // rows 0-2
    assert.deepEqual(range(-500, 100, ROW, COLS, TOTAL), { first: 0, last: 6 });
    assert.deepEqual(range(30_000, 30_800, ROW, COLS, TOTAL), { first: 700, last: 720 }); // rows 100-102
    assert.deepEqual(range(156_000, 160_000, ROW, COLS, TOTAL), { first: 3640, last: 3653 }); // past the end
    assert.equal(libraryCellRange(0, 800, 0, COLS, TOTAL), null);
    assert.equal(libraryCellRange(0, 800, ROW, COLS, 0), null);
});

test('the pages under the screen come first, nearest its middle, then ahead, then one behind', () => {
    // Cells 700-720 lie on pages 14; the next pages down are 15 and 16, the one behind 13
    assert.deepEqual(plan({ first: 700, last: 720 }, 1, PAGE, TOTAL, new Set()), [14, 15, 16, 13]);
    // Scrolling up: ahead is above
    assert.deepEqual(plan({ first: 700, last: 720 }, -1, PAGE, TOTAL, new Set()), [14, 13, 12, 15]);
    // A screen across three pages: the middle one first, then the one ahead
    assert.deepEqual(plan({ first: 60, last: 160 }, 1, PAGE, TOTAL, new Set()), [2, 3, 1, 4, 5, 0]);
    assert.deepEqual(plan({ first: 60, last: 160 }, -1, PAGE, TOTAL, new Set()), [2, 1, 3, 0, 4]);
});

test('held pages are skipped, and nothing is asked past either end', () => {
    assert.deepEqual(plan({ first: 700, last: 720 }, 1, PAGE, TOTAL, new Set([14, 15])), [16, 13]);
    assert.deepEqual(plan({ first: 0, last: 20 }, -1, PAGE, TOTAL, new Set()), [0, 1]);
    assert.deepEqual(plan({ first: 3640, last: 3653 }, 1, PAGE, TOTAL, new Set()), [73, 72, 71]);
});
