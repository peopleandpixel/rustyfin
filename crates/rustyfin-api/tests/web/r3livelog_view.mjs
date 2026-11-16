// R3 live log: the view shows each line's time, level, target and message, escaped; filters by level;
// keeps at most 2,000 rows; follows new lines unless scrolled up; and the client asks for the
// server's recent lines on each connect and starts over from them.
// Runs admin.js's live log functions and api.js's LiveLog in a vm sandbox with fake elements.
// Usage: node r3livelog_view.mjs (run by api_tests.rs r3livelog_web_view).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const admin = fs.readFileSync(new URL('modules/admin.js', src), 'utf8');
const api = fs.readFileSync(new URL('api.js', src), 'utf8');

// A top-level declaration, from its head to the end given
function slice(source, head, end = '\n}\n') {
    const start = source.indexOf(head);
    assert.ok(start >= 0, `source has ${head}`);
    return source.slice(start, source.indexOf(end, start) + end.length);
}

// A fake element with the parts the functions use; a row's text is its innerHTML.
function element() {
    const el = {
        children: [],
        className: '',
        scrollTop: 0,
        clientHeight: 100,
        value: '',
        html: '',
        get scrollHeight() { return this.children.length * 10; },
        get firstChild() { return this.children[0]; },
        get innerHTML() { return this.html; },
        set innerHTML(v) { this.html = v; this.children = []; },
        appendChild(c) { this.children.push(c); return c; },
        removeChild(c) { this.children.splice(this.children.indexOf(c), 1); return c; },
        replaceChildren(...c) { this.html = ''; this.children = c; },
        querySelector(sel) { return sel === 'p' && this.html.includes('<p') ? {} : null; },
    };
    return el;
}

class FakeSocket {
    static last = null;
    constructor(url) { this.url = url; this.sent = []; this.readyState = 0; FakeSocket.last = this; }
    send(text) { this.sent.push(JSON.parse(text)); }
    close() { this.readyState = 3; this.onclose?.(); }
    open() { this.readyState = 1; this.onopen(); }
    frame(data) { this.onmessage({ data: JSON.stringify({ MessageType: 'LogMessage', Data: data }) }); }
}

function sandbox(serverLevel = 'info') {
    const elements = { 'livelog-container': element(), 'livelog-level': element(), 'livelog-status': element() };
    const posted = [];
    const context = vm.createContext({
        t: key => key,
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        uiLocale: () => 'en',
        $: id => elements[id] || null,
        toast: () => {},
        document: { createElement: () => element() },
        localStorage: { getItem: () => 'token' },
        WebSocket: FakeSocket,
        window: { location: { protocol: 'http:', host: 'localhost' } },
        setTimeout: () => {},
        setInterval: () => 0,
        clearInterval: () => {},
        API: {
            getLogLevel: async () => ({ LogLevel: serverLevel }),
            setLogLevel: async level => posted.push(level),
        },
    });
    vm.runInContext(
        `${slice(api, 'const LiveLog = {', '\n};\n')}
         ${slice(admin, 'const LIVELOG_MAX_ROWS', ';\n')}
         ${slice(admin, 'const LOG_LEVEL_RANK', ';\n')}
         let liveLogLevel = 'info';
         let liveLogInitialized = false;
         ${slice(admin, 'function initLiveLog(')}
         ${slice(admin, 'async function loadLiveLogLevel(')}
         ${slice(admin, 'function logShown(')}
         ${slice(admin, 'function logRow(')}
         ${slice(admin, 'function liveLogAtEnd(')}
         ${slice(admin, 'function renderLiveLog(')}
         ${slice(admin, 'function appendLogEntry(')}
         ${slice(admin, 'function clearLiveLog(')}
         ${slice(admin, 'async function setLogLevel(')}
         globalThis.LiveLog = LiveLog;
         globalThis.level = () => liveLogLevel;`,
        context,
    );
    return { context, container: elements['livelog-container'], select: elements['livelog-level'], posted };
}

const line = (n, level = 'INFO', extra = {}) => ({
    Timestamp: new Date(Date.UTC(2026, 10, 15, 10, 0, 0, n)).toISOString(),
    Level: level,
    Target: 'rustyfin_core::scanner',
    Message: `line ${n}`,
    Seq: 5000 + n,
    ...extra,
});

test('the client asks for the recent lines on connect and starts over from them', async () => {
    const { context, container } = sandbox();
    context.initLiveLog();
    const socket = FakeSocket.last;
    socket.open();
    assert.deepEqual(socket.sent, [{ MessageType: 'LiveLogStart' }]);
    assert.match(socket.url, /^ws:\/\/localhost\/socket\?/);
    socket.frame(line(1));
    socket.frame(line(2));
    assert.equal(container.children.length, 2);

    // A reconnect replays them, so the list starts over and has each line once.
    socket.readyState = 3;
    context.LiveLog.connect();
    const again = FakeSocket.last;
    assert.notEqual(again, socket);
    again.open();
    again.frame(line(1));
    again.frame(line(2));
    again.frame(line(3));
    assert.equal(context.LiveLog.messages.length, 3);
    assert.equal(container.children.length, 3);

    // After Clear, the replay of a reconnect skips the cleared lines by their Seq, also a later
    // line with the same timestamp as the last cleared one is kept, and a "lines skipped" notice
    // (no Seq) always shows.
    context.clearLiveLog();
    again.readyState = 3;
    context.LiveLog.connect();
    FakeSocket.last.open();
    for (const n of [1, 2, 3]) FakeSocket.last.frame(line(n));
    FakeSocket.last.frame(line(3, 'INFO', { Message: 'same time', Seq: 5004 }));
    FakeSocket.last.frame(line(9, 'WARN', { Message: '2 log lines skipped', Seq: undefined }));
    assert.deepEqual([...context.LiveLog.messages.map(m => m.Message)], ['same time', '2 log lines skipped']);
});

test('a row shows time, level, target and the message with its fields, all escaped', async () => {
    const { context, container } = sandbox();
    context.initLiveLog();
    FakeSocket.last.open();
    FakeSocket.last.frame(line(1, 'WARN', {
        Target: 'core<b>',
        Message: 'scan{library="<img src=x>"}: could not read task=3 error=a & b',
    }));
    const html = container.children[0].innerHTML;
    assert.match(html, /\d\d:00:00\.001/);
    assert.match(html, />WARN</);
    assert.match(html, />core&lt;b&gt;</);
    assert.match(html, /scan\{library=&quot;&lt;img src=x&gt;&quot;\}: could not read task=3 error=a &amp; b/);
    assert.doesNotMatch(html, /<img|<b>/);
});

test('the view keeps at most 2,000 rows and lines', async () => {
    const { context, container } = sandbox();
    context.initLiveLog();
    FakeSocket.last.open();
    for (let n = 0; n < 2100; n++) FakeSocket.last.frame(line(n));
    assert.equal(container.children.length, 2000);
    assert.equal(context.LiveLog.messages.length, 2000);
    assert.equal(context.LiveLog.messages[0].Message, 'line 100');
    context.renderLiveLog();
    assert.equal(container.children.length, 2000);
});

test('the level filters the rows, starts at the server level, and shows hidden lines again', async () => {
    const { context, container, select, posted } = sandbox('debug');
    context.initLiveLog();
    await new Promise(r => setImmediate(r));
    assert.equal(context.level(), 'debug');
    assert.equal(select.value, 'debug');
    FakeSocket.last.open();
    FakeSocket.last.frame(line(1, 'DEBUG'));
    FakeSocket.last.frame(line(2, 'INFO'));
    FakeSocket.last.frame(line(3, 'TRACE'));
    assert.equal(container.children.length, 2);

    await context.setLogLevel('warn');
    assert.equal(container.children.length, 0);
    assert.deepEqual(posted, ['warn']);
    await context.setLogLevel('trace');
    assert.equal(container.children.length, 3);
});

test('new lines keep the view at its end, unless the user scrolled up', async () => {
    const { context, container } = sandbox();
    context.initLiveLog();
    FakeSocket.last.open();
    for (let n = 0; n < 30; n++) FakeSocket.last.frame(line(n));
    assert.equal(container.scrollTop, container.scrollHeight);

    container.scrollTop = 50;
    FakeSocket.last.frame(line(30));
    assert.equal(container.scrollTop, 50);

    container.scrollTop = container.scrollHeight - container.clientHeight;
    FakeSocket.last.frame(line(31));
    assert.equal(container.scrollTop, container.scrollHeight);
});
