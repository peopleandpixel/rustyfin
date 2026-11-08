// B21 round 4 fix 2 (spec §13, AC58): the web client follows a demotion.
// Loads api.js in a vm sandbox with a fake fetch and localStorage, signed in as carol.
// Usage: node b21_admin_recheck.mjs PATH/TO/api.js (run by api_tests.rs b21_web_client_follows_a_demotion).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(process.argv[2], 'utf8');
const ORIGIN = 'http://rustyfin.test';

// A fetch answer with the server's JSON body.
function answer(status, body) {
    const text = body === undefined ? '' : JSON.stringify(body);
    return {
        ok: status >= 200 && status < 300,
        status,
        json: async () => JSON.parse(text),
        text: async () => text,
    };
}

const FORBIDDEN = answer(403, {
    Type: 'https://tools.ietf.org/html/rfc9110',
    Title: 'Forbidden',
    Detail: 'administrator access required',
    Status: 403,
});

// /Users/Me as the server answers it.
function me(isAdministrator) {
    return answer(200, {
        Id: 'id-carol',
        Name: 'carol',
        IsAdministrator: isAdministrator,
        LastLoginDate: null,
        Configuration: {},
    });
}

// A promise that waits for release().
function held() {
    let release;
    const promise = new Promise(resolve => {
        release = resolve;
    });
    return { promise, release };
}

// Lets the fake requests and the promises waiting on them finish.
const settle = () => new Promise(resolve => setTimeout(resolve, 10));

// api.js for carol, signed in. `routes` maps 'METHOD /path' to an answer, or to a function
// that returns one (or a promise of one, or throws); other paths get a 404.
function client({ admin = true, routes = {} } = {}) {
    const store = new Map([
        ['rustyfin_token', 'token-carol'],
        ['rustyfin_user_id', 'id-carol'],
        ['rustyfin_user_name', 'carol'],
        ['rustyfin_is_admin', String(admin)],
    ]);
    const localStorage = {
        getItem: key => (store.has(key) ? store.get(key) : null),
        setItem: (key, value) => store.set(key, String(value)),
        removeItem: key => store.delete(key),
    };
    const log = [];
    async function fetch(url, options = {}) {
        assert.ok(url.startsWith(ORIGIN), url);
        const method = options.method || 'GET';
        const path = url.slice(ORIGIN.length);
        log.push({ method, path, options });
        const route = routes[`${method} ${path}`];
        if (route === undefined) return answer(404, { Title: 'Not Found', Detail: `no route ${method} ${path}` });
        return typeof route === 'function' ? route(options) : route;
    }
    const window = { location: { origin: ORIGIN, hash: '' } };
    const context = vm.createContext({ window, localStorage, fetch, console, setTimeout, clearTimeout });
    vm.runInContext(source, context, { filename: 'api.js' });
    const API = window.API;
    let lost = 0;
    API.onAdminLost = () => {
        lost += 1;
    };
    return {
        API,
        store,
        calls: (method, path) => log.filter(c => c.method === method && c.path === path),
        lost: () => lost,
    };
}

// Expects the request to fail as a 403 does today: an Error with the body's Detail and the status.
async function refused(request) {
    await assert.rejects(request, err => {
        assert.equal(err.status, 403);
        assert.equal(err.message, 'administrator access required');
        return true;
    });
}

test('a GET 403 while an administrator re-reads the user and follows the demotion', async () => {
    const c = client({
        routes: {
            'GET /Users': FORBIDDEN,
            'GET /Users/Me': me(false),
            'GET /Items/Latest': answer(200, [{ Id: 'item-1' }]),
        },
    });
    await c.API.cachedGet('/Items/Latest');
    await refused(c.API.get('/Users'));
    await settle();

    const reads = c.calls('GET', '/Users/Me');
    assert.equal(reads.length, 1, 'one re-read of /Users/Me');
    assert.equal(reads[0].options.cache, 'no-store', 'past the browser cache');
    assert.equal(reads[0].options.headers['X-Emby-Token'], 'token-carol', 'as carol');
    assert.equal(c.API.isAdmin, false);
    assert.equal(c.store.get('rustyfin_is_admin'), 'false');
    assert.equal(c.lost(), 1, 'onAdminLost once');

    await c.API.cachedGet('/Items/Latest');
    assert.equal(c.calls('GET', '/Items/Latest').length, 2, 'the response cache was dropped');

    await refused(c.API.get('/Users'));
    await settle();
    assert.equal(c.calls('GET', '/Users/Me').length, 1, 'no re-read once the client counts as a user');
    assert.equal(c.lost(), 1);
});

test('POST, PUT and DELETE 403s re-read the user too', async () => {
    for (const [method, path, call] of [
        ['POST', '/Users/id-dan/Policy', (api, path) => api.post(path, { IsAdministrator: true })],
        ['PUT', '/Users/id-dan', (api, path) => api.put(path, { Name: 'dan' })],
        ['DELETE', '/ScheduledTasks/Running/task-1', (api, path) => api.del(path)],
    ]) {
        const c = client({ routes: { [`${method} ${path}`]: FORBIDDEN, 'GET /Users/Me': me(false) } });
        await refused(call(c.API, path));
        await settle();
        assert.equal(c.calls('GET', '/Users/Me').length, 1, `${method}: one re-read`);
        assert.equal(c.API.isAdmin, false, method);
        assert.equal(c.store.get('rustyfin_is_admin'), 'false', method);
        assert.equal(c.lost(), 1, method);
    }
});

test('a 403 for an account that is still an administrator changes nothing', async () => {
    const c = client({
        routes: {
            'GET /Users': FORBIDDEN,
            'GET /Users/Me': me(true),
            'GET /Items/Latest': answer(200, [{ Id: 'item-1' }]),
        },
    });
    await c.API.cachedGet('/Items/Latest');
    await refused(c.API.get('/Users'));
    await settle();
    assert.equal(c.calls('GET', '/Users/Me').length, 1, 'one re-read');
    assert.equal(c.API.isAdmin, true);
    assert.equal(c.store.get('rustyfin_is_admin'), 'true');
    assert.equal(c.lost(), 0);
    await c.API.cachedGet('/Items/Latest');
    assert.equal(c.calls('GET', '/Items/Latest').length, 1, 'the response cache is kept');

    assert.equal(await c.API.recheckAdmin(), true, 'recheckAdmin() resolves to the flag');
    assert.equal(c.calls('GET', '/Users/Me').length, 2, 'a later re-read reads again');
});

test('a failed re-read changes nothing', async () => {
    const failures = {
        'a 500': () => answer(500, { Title: 'Internal Server Error', Detail: 'database is locked' }),
        'a 401': () => answer(401, { Title: 'Unauthorized', Detail: 'unauthorized' }),
        'a network error': () => {
            throw new TypeError('Failed to fetch');
        },
    };
    for (const [name, failure] of Object.entries(failures)) {
        const c = client({ routes: { 'GET /Users': FORBIDDEN, 'GET /Users/Me': failure } });
        await refused(c.API.get('/Users'));
        await settle();
        assert.equal(c.calls('GET', '/Users/Me').length, 1, `${name}: one re-read`);
        assert.equal(c.API.isAdmin, true, name);
        assert.equal(c.store.get('rustyfin_is_admin'), 'true', name);
        assert.equal(c.store.get('rustyfin_token'), 'token-carol', `${name}: still signed in`);
        assert.equal(c.lost(), 0, name);
        assert.equal(await c.API.recheckAdmin(), true, `${name}: recheckAdmin() resolves, it never rejects`);
    }
});

test('one re-read at a time', async () => {
    const hold = held();
    const c = client({
        routes: {
            'GET /Users': FORBIDDEN,
            'POST /Users/id-dan/Policy': FORBIDDEN,
            'GET /Users/Me': () => hold.promise.then(() => me(false)),
        },
    });
    const first = c.API.recheckAdmin();
    const second = c.API.recheckAdmin();
    await refused(c.API.get('/Users'));
    await refused(c.API.post('/Users/id-dan/Policy', {}));
    await settle();
    assert.equal(c.calls('GET', '/Users/Me').length, 1, 'the re-reads share one request');
    hold.release();
    assert.equal(await first, false);
    assert.equal(await second, false);
    await settle();
    assert.equal(c.calls('GET', '/Users/Me').length, 1);
    assert.equal(c.lost(), 1, 'onAdminLost once');
});

test('recheckAdmin() reads past the response cache', async () => {
    let admin = true;
    const c = client({ routes: { 'GET /Users/Me': () => me(admin) } });
    await c.API.cachedGet('/Users/Me');
    admin = false;
    assert.equal(await c.API.recheckAdmin(), false);
    const reads = c.calls('GET', '/Users/Me');
    assert.equal(reads.length, 2, 'not answered from the response cache');
    assert.equal(reads[1].options.cache, 'no-store');
    assert.equal(c.API.isAdmin, false);
    assert.equal(c.store.get('rustyfin_is_admin'), 'false');
    assert.equal(c.lost(), 1);
});

test('a 403 for a user who is not an administrator reads nothing', async () => {
    const c = client({ admin: false, routes: { 'GET /Users': FORBIDDEN, 'GET /Users/Me': me(false) } });
    await refused(c.API.get('/Users'));
    await settle();
    assert.equal(c.calls('GET', '/Users/Me').length, 0);
    assert.equal(c.lost(), 0);
});

test('responses in flight at the demotion are not cached or shared', async () => {
    const holds = [held(), held()];
    let served = 0;
    const c = client({
        routes: {
            'GET /System/Info': FORBIDDEN,
            'GET /Users/Me': me(false),
            'GET /Users': () => {
                const n = served++;
                assert.ok(n < holds.length, 'at most two requests for /Users');
                return holds[n].promise.then(() => answer(200, [{ Name: `list ${n}` }]));
            },
        },
    });
    const early = c.API.cachedGet('/Users');
    await refused(c.API.get('/System/Info'));
    await settle();
    assert.equal(c.lost(), 1, 'demoted while /Users is in flight');

    const late = c.API.cachedGet('/Users');
    await settle();
    assert.equal(c.calls('GET', '/Users').length, 2, 'a request after the demotion is not merged with one from before');

    holds[0].release();
    assert.equal((await early)[0].Name, 'list 0', 'the early caller still gets its answer');
    await settle();
    const third = c.API.cachedGet('/Users');
    await settle();
    assert.equal(c.calls('GET', '/Users').length, 2, 'the early answer neither was cached nor dropped the late request');

    holds[1].release();
    assert.equal((await late)[0].Name, 'list 1');
    assert.equal((await third)[0].Name, 'list 1');
    assert.equal((await c.API.cachedGet('/Users'))[0].Name, 'list 1', 'the late answer is cached');
    assert.equal(c.calls('GET', '/Users').length, 2);
});
