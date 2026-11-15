// R3 tasks: the admin tasks tab shows a running task's state and progress, and reloads while open.
// Runs admin.js's task card and polling functions in a vm sandbox with fake timers and elements.
// Usage: node r3tasks_admin.mjs (run by api_tests.rs r3tasks_web_task_cards_and_polling).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const source = fs.readFileSync(new URL('modules/admin.js', src), 'utf8');

// A top-level declaration, from its head to the end given
function slice(head, end = '\n}\n') {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `admin.js has ${head}`);
    return source.slice(start, source.indexOf(end, start) + end.length);
}

// A fake element with the parts the functions use.
function element(hidden = false) {
    const classes = new Set(hidden ? ['hidden'] : []);
    return {
        isConnected: true,
        innerHTML: '',
        className: '',
        classList: {
            contains: c => classes.has(c),
            toggle: (c, on) => (on ? classes.add(c) : classes.delete(c)),
        },
    };
}

const TAB_IDS = ['admin-overview', 'admin-users', 'admin-libs', 'admin-tasks', 'admin-plugins',
    'admin-sessions', 'admin-activity', 'admin-livelog', 'admin-signin', 'admin-import'];

function sandbox(answers) {
    const elements = Object.fromEntries(TAB_IDS.map(id => [id, element(id !== 'admin-tasks')]));
    elements['admin-tasks-list'] = element();
    const timers = [];
    const fetches = [];
    const toasts = [];
    const context = vm.createContext({
        t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        icon: name => `<i data-icon="${name}"></i>`,
        uiLocale: () => 'en',
        $: id => elements[id] || null,
        toast: (message, kind) => toasts.push([message, kind]),
        API: {
            getTasks: async () => {
                fetches.push(Date.now());
                const next = answers.shift();
                if (next instanceof Error) throw next;
                return next;
            },
        },
        setTimeout: (fn, ms) => {
            const timer = { fn, ms, cleared: false };
            timers.push(timer);
            return timer;
        },
        clearTimeout: timer => {
            if (timer) timer.cleared = true;
        },
        window: { listeners: {}, addEventListener(type, fn) { this.listeners[type] = fn; } },
        loadActivityLog: () => {},
        initLiveLog: () => {},
        loadOidcAdminTab: () => {},
        loadJellyfinImport: () => {},
    });
    vm.runInContext(
        `${slice('function getTaskStateLabel(')}
         ${slice('function getTaskCategoryIcon(')}
         ${slice('function renderTaskCard(')}
         ${slice('function sortTasks(')}
         ${slice('const TASK_POLL_BUSY_MS', ';\n')}
         ${slice('const TASK_POLL_IDLE_MS', ';\n')}
         var taskPollTimer = null;
         ${slice('function tasksBusy(')}
         ${slice('function tasksTabOpen(')}
         ${slice('function stopTaskPolling(')}
         ${slice('function scheduleTaskPoll(')}
         ${slice('async function refreshTasks(')}
         ${slice("window.addEventListener('hashchange'", '\n});\n')}
         ${slice('function showAdminTab(')}
         this.fns = { renderTaskCard, refreshTasks, showAdminTab, stopTaskPolling };`,
        context,
    );
    // The timers still pending
    const pending = () => timers.filter(timer => !timer.cleared && !timer.fired);
    // Runs the one pending timer and waits for the reload it starts
    const tick = async () => {
        const [timer] = pending();
        assert.ok(timer, 'a reload is planned');
        timer.fired = true;
        timer.fn();
        await new Promise(resolve => setImmediate(resolve));
    };
    return { context, elements, fetches, toasts, pending, tick };
}

function task(state, progress, extra = {}) {
    return {
        Id: 'LibraryScan',
        Name: 'Scan Media Library',
        Description: 'Scans the libraries.',
        Category: 'Library',
        State: state,
        CurrentProgressPercentage: progress,
        Triggers: [],
        LastExecutionResult: {
            Status: 'Completed',
            EndTimeUtc: '2026-11-15T10:00:00Z',
            StartTimeUtc: '2026-11-15T09:59:00Z',
        },
        ...extra,
    };
}

const idleTask = { ...task('Idle', null), Id: 'CleanActivityLog', Name: 'Clean Activity Log', Category: 'Maintenance' };

test('an idle task shows Ready, its last result and Start, with no progress bar', () => {
    const { context } = sandbox([]);
    const html = context.fns.renderTaskCard(task('Idle', null));
    assert.match(html, /admin\.state_ready/);
    assert.match(html, /admin\.last_run\|.*Completed/);
    assert.match(html, /data-action="start-task"/);
    assert.doesNotMatch(html, /data-action="stop-task"/);
    assert.doesNotMatch(html, /role="progressbar"/);
});

test('a running task with a percent shows Running, its bar at that percent and Stop', () => {
    const { context } = sandbox([]);
    const html = context.fns.renderTaskCard(task('Running', 42.4));
    assert.match(html, /admin\.state_running/);
    assert.match(html, /data-task-progress="42"/);
    assert.match(html, />42%</);
    assert.match(html, /style="width:42%"/);
    assert.match(html, /aria-valuenow="42"/);
    assert.match(html, /data-action="stop-task" data-id="LibraryScan" data-name="Scan Media Library">/);
    assert.doesNotMatch(html, /jf-progress-indeterminate/);
});

test('a running task at 0 percent shows 0%, not an unknown percent', () => {
    const { context } = sandbox([]);
    const html = context.fns.renderTaskCard(task('Running', 0));
    assert.match(html, /data-task-progress="0"/);
    assert.match(html, />0%</);
});

test('a running task with no percent shows Running and an indeterminate bar', () => {
    const { context } = sandbox([]);
    for (const progress of [null, undefined]) {
        const html = context.fns.renderTaskCard(task('Running', progress));
        assert.match(html, /admin\.state_running/);
        assert.match(html, /data-task-progress="indeterminate"/);
        assert.match(html, /jf-progress-indeterminate/);
        assert.doesNotMatch(html, /aria-valuenow/);
        assert.doesNotMatch(html, /\d%</);
        assert.match(html, /data-action="stop-task"[^>]*>/);
        assert.doesNotMatch(html, /data-action="stop-task"[^>]*disabled/);
    }
});

test('a cancelling task shows Stopping, keeps its bar and disables Stop', () => {
    const { context } = sandbox([]);
    const html = context.fns.renderTaskCard(task('Cancelling', 80));
    assert.match(html, /admin\.state_stopping/);
    assert.match(html, /data-task-progress="80"/);
    assert.match(html, /data-action="stop-task"[^>]*disabled>/);
    assert.doesNotMatch(html, /data-action="start-task"/);
});

test('the indeterminate bar has its animation in the stylesheet', () => {
    const css = fs.readFileSync(new URL('input.css', src), 'utf8');
    assert.match(css, /\.jf-progress-indeterminate\s*\{[^}]*animation:\s*jfIndeterminate/);
    assert.match(css, /@keyframes jfIndeterminate/);
});

test('the list reloads every 1.5 s while a task runs and every 10 s when all are idle', async () => {
    const { context, elements, fetches, pending, tick } = sandbox([
        [idleTask, task('Running', 10)],
        [idleTask, task('Running', 55)],
        [idleTask, task('Idle', null)],
    ]);
    await context.fns.refreshTasks(true);
    assert.equal(fetches.length, 1);
    assert.deepEqual(pending().map(timer => timer.ms), [1500]);
    // Sorted by category, then name
    const list = elements['admin-tasks-list'].innerHTML;
    assert.ok(list.indexOf('Scan Media Library') < list.indexOf('Clean Activity Log'), 'Library before Maintenance');
    assert.match(list, /data-task-progress="10"/);

    await tick();
    assert.equal(fetches.length, 2);
    assert.match(elements['admin-tasks-list'].innerHTML, /data-task-progress="55"/);
    assert.deepEqual(pending().map(timer => timer.ms), [1500]);

    await tick();
    assert.equal(fetches.length, 3);
    assert.doesNotMatch(elements['admin-tasks-list'].innerHTML, /role="progressbar"/);
    assert.match(elements['admin-tasks-list'].innerHTML, /admin\.last_run/);
    assert.deepEqual(pending().map(timer => timer.ms), [10000], 'slow once nothing runs');
});

test('a failed reload by the timer shows no toast and tries again', async () => {
    const { context, toasts, pending } = sandbox([new Error('offline')]);
    await context.fns.refreshTasks(true);
    assert.deepEqual(toasts, []);
    assert.deepEqual(pending().map(timer => timer.ms), [10000]);
});

test('leaving the tab or the page stops the reloads', async () => {
    const { context, elements, fetches, pending, tick } = sandbox([
        [task('Running', 10)],
        [task('Running', 20)],
        [task('Running', 30)],
    ]);
    await context.fns.refreshTasks(true);
    assert.equal(pending().length, 1);

    // Another tab: the pending reload is dropped
    const button = { parentElement: { querySelectorAll: () => [] } };
    context.fns.showAdminTab(button, 'admin-users');
    assert.ok(elements['admin-tasks'].classList.contains('hidden'));
    assert.equal(pending().length, 0);
    assert.equal(fetches.length, 1);

    // Back to the tasks tab: it reloads at once and polls again
    context.fns.showAdminTab(button, 'admin-tasks');
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(fetches.length, 2);
    assert.equal(pending().length, 1);

    // Another page: the tab is gone, so the hashchange stops the reloads
    elements['admin-tasks'].isConnected = false;
    context.window.listeners.hashchange();
    assert.equal(pending().length, 0);

    // A timer that fires after the tab is gone fetches nothing and plans nothing
    elements['admin-tasks'].isConnected = true;
    await context.fns.refreshTasks(true);
    assert.equal(fetches.length, 3);
    elements['admin-tasks'].isConnected = false;
    await tick();
    assert.equal(fetches.length, 3);
    assert.equal(pending().length, 0);
});
