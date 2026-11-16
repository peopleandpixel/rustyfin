// The item page's second round: the metadata editor's studios, people, end date and provider ids;
// Download (only with CanDownload) and Share; the subtitle editor's upload body, stream list and the
// remote search left off with a note; and their wiring, icons and translations.
// Runs the helpers in a vm sandbox and checks the rest as text.
// Usage: node r3item2_web.mjs (run by api_tests.rs r3item2_web).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const read = path => fs.readFileSync(new URL(path, src), 'utf8');
const metadata = read('modules/metadata-admin.js');
const subtitles = read('modules/subtitle-admin.js');
const item = read('modules/item.js');
const router = read('modules/router.js');
const api = read('api.js');
const icons = read('icons.js');
const app = read('app.js');
const en = JSON.parse(read('translations/en.json'));
const de = JSON.parse(read('translations/de.json'));

// A top-level declaration of `text`, from its head to the end given
function slice(text, head, end = '\n}\n') {
    const start = text.indexOf(head);
    assert.ok(start >= 0, `has ${head}`);
    return text.slice(start, text.indexOf(end, start) + end.length);
}

const shared = [];
const context = vm.createContext({
    t: (key, opts) => (opts ? `${key}|${JSON.stringify(opts)}` : key),
    escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
    icon: name => `<i:${name}>`,
    toast: (...args) => shared.push(['toast', ...args]),
    API: { downloadUrl: id => `/base/Items/${id}/Download?api_key=tok` },
    location: { origin: 'https://media.example', pathname: '/web/index.html' },
    navigator: {},
    window: { prompt: (...args) => shared.push(['prompt', ...args]) },
});
vm.runInContext(
    `${slice(metadata, 'const LOCKABLE_FIELDS = ', ';\n')}
     ${slice(metadata, 'const PERSON_TYPES = ', ';\n')}
     ${slice(metadata, 'const LABEL = ', ';\n')}
     ${slice(metadata, 'function splitList(')}
     ${slice(metadata, 'function itemUpdateBody(')}
     ${slice(metadata, 'function personRowHtml(')}
     ${slice(metadata, 'function providerIdsHtml(')}
     ${slice(subtitles, 'const UPLOAD_FORMATS = ', ';\n')}
     ${slice(subtitles, 'function subtitleFormat(')}
     ${slice(subtitles, 'function uploadBody(')}
     ${slice(subtitles, 'function subtitleStreams(')}
     ${slice(subtitles, 'function subtitleRowHtml(')}
     ${slice(item, 'function downloadShareButtons(')}
     ${slice(item, 'function itemShareUrl(')}
     ${slice(item, 'async function shareItem(')}
     ${slice(item, 'function metadataToolButtons(')}
     this.fns = { itemUpdateBody, personRowHtml, providerIdsHtml, subtitleFormat, uploadBody, subtitleStreams,
        subtitleRowHtml, downloadShareButtons, itemShareUrl, shareItem, metadataToolButtons };`,
    context,
);
const fns = context.fns;
const plain = value => JSON.parse(JSON.stringify(value));

test('the editor sends end date, studios, people and provider ids as Jellyfin does', () => {
    const body = plain(fns.itemUpdateBody({
        name: 'Quill Harbor', endDate: '2032-02-03', studios: 'Lantern Works, , lantern works, Ferry Co',
        people: [
            { name: ' Pat Example ', role: 'Keeper', type: 'Actor' },
            { name: '', role: 'nobody', type: 'Writer' },
            { name: 'Sam Sample', role: '', type: 'Director' },
        ],
        providerIdsBefore: { Tvdb: '77', Tmdb: '1' },
        providerIds: { Tmdb: ' 424242 ', Imdb: '' },
        enabled: {},
    }));
    assert.equal(body.EndDate, '2032-02-03T00:00:00.0000000Z');
    assert.deepEqual(body.Studios, [{ Name: 'Lantern Works' }, { Name: 'Ferry Co' }]);
    assert.deepEqual(body.People, [
        { Name: 'Pat Example', Role: 'Keeper', Type: 'Actor' },
        { Name: 'Sam Sample', Role: null, Type: 'Director' },
    ]);
    // The ids the form shows go over the item's others; a blank one clears it
    assert.deepEqual(body.ProviderIds, { Tvdb: '77', Tmdb: '424242', Imdb: '' });
    // No end date field (not a series): none is sent, so the stored one stays
    assert.ok(!('EndDate' in plain(fns.itemUpdateBody({ name: 'A', enabled: {} }))));
});

test('people rows and provider id fields', () => {
    const row = fns.personRowHtml({ Name: 'Pat "P" Example', Role: 'Keeper', Type: 'Director' });
    assert.match(row, /data-person-name[^>]*value="Pat &quot;P&quot; Example"/);
    assert.match(row, /<option value="Director" selected>/);
    assert.match(row, /data-remove-person/);
    assert.match(fns.personRowHtml({ Type: 'Narrator' }), /<option value="Actor" selected>/);

    const ids = fns.providerIdsHtml(
        [{ Name: 'TheMovieDb', Key: 'Tmdb' }, { Name: 'IMDb', Key: 'Imdb' }],
        { tmdb: '424242' },
    );
    assert.match(ids, /data-provider-id="Tmdb" value="424242"/);
    assert.match(ids, /data-provider-id="Imdb" value=""/);
    assert.equal(fns.providerIdsHtml([], {}), '');
    // The editor asks the server which ids the item takes, and adds people rows without a framework
    assert.match(metadata, /API\.getMetadataEditor\(id\)/);
    assert.match(metadata, /insertAdjacentHTML\('beforeend', personRowHtml\(\)\)/);
    assert.match(api, /getMetadataEditor\(id\) \{ return this\.get\(`\/Items\/\$\{id\}\/MetadataEditor`\)/);
});

test('download shows only with CanDownload; share is always there', () => {
    const can = fns.downloadShareButtons({ Id: 'm1', Name: 'Quill', CanDownload: true });
    assert.match(can, /<a [^>]*href="\/base\/Items\/m1\/Download\?api_key=tok" download/);
    assert.match(can, /<i:Download>/);
    assert.match(can, /data-action="share-item" data-id="m1" data-name="Quill"/);
    const cannot = fns.downloadShareButtons({ Id: 'm1', Name: 'Quill', CanDownload: false });
    assert.doesNotMatch(cannot, /Download/);
    assert.match(cannot, /share-item/);
    assert.match(item, /\$\{downloadShareButtons\(item\)\}/);
    assert.match(api, /downloadUrl\(id\) \{/);
    assert.match(api, /\/Items\/\$\{encodeURIComponent\(id\)\}\/Download/);
});

test('share uses the share sheet when there is one, else copies the link', async () => {
    assert.equal(fns.itemShareUrl('a b'), 'https://media.example/web/index.html#detail/a%20b');
    const sheet = [];
    context.navigator.share = async data => { sheet.push(data); };
    await fns.shareItem({ id: 'm1', name: 'Quill' });
    assert.deepEqual(plain(sheet), [{ title: 'Quill', url: 'https://media.example/web/index.html#detail/m1' }]);

    // Cancelling the sheet does nothing more
    context.navigator.share = async () => { throw Object.assign(new Error('x'), { name: 'AbortError' }); };
    shared.length = 0;
    await fns.shareItem({ id: 'm1' });
    assert.deepEqual(shared, []);

    // No sheet: the clipboard, then a prompt when the clipboard is refused
    delete context.navigator.share;
    const copied = [];
    context.navigator.clipboard = { writeText: async text => { copied.push(text); } };
    await fns.shareItem({ id: 'm1' });
    assert.deepEqual(copied, ['https://media.example/web/index.html#detail/m1']);
    assert.deepEqual(shared.at(-1), ['toast', 'detail.share_copied']);
    context.navigator.clipboard = { writeText: async () => { throw new Error('denied'); } };
    await fns.shareItem({ id: 'm1' });
    assert.deepEqual(shared.at(-1), ['prompt', 'detail.share_copy_prompt', 'https://media.example/web/index.html#detail/m1']);
});

test('the subtitle editor uploads Jellyfin-shaped bodies and deletes only external files', () => {
    assert.equal(fns.subtitleFormat('a.b.SRT'), 'srt');
    assert.equal(fns.subtitleFormat('x.vtt'), 'vtt');
    assert.equal(fns.subtitleFormat('x.sub'), '');
    assert.equal(fns.subtitleFormat('srt'), '');
    assert.deepEqual(plain(fns.uploadBody({ language: ' eng ', format: 'srt', forced: 1, hearingImpaired: 0, data: 'aGk=' })), {
        Language: 'eng', Format: 'srt', IsForced: true, IsHearingImpaired: false, Data: 'aGk=',
    });
    const streams = fns.subtitleStreams({ MediaSources: [{ MediaStreams: [
        { Type: 'Subtitle', Index: 0, IsExternal: true, Language: 'eng', IsForced: true, Codec: 'subrip' },
        { Type: 'Video', Index: 1 },
        { Type: 'Subtitle', Index: 3, IsExternal: false, DisplayTitle: 'German', Codec: 'ass' },
    ] }] });
    assert.equal(streams.length, 2);
    const external = fns.subtitleRowHtml(streams[0]);
    assert.match(external, /data-delete-subtitle="0"/);
    assert.match(external, /subtitles\.external · subtitles\.forced · SUBRIP/);
    assert.doesNotMatch(fns.subtitleRowHtml(streams[1]), /data-delete-subtitle/);
    // The upload and delete go to Jellyfin's routes
    assert.match(api, /uploadSubtitle\(id, body\) \{ return this\.post\(`\/Videos\/\$\{id\}\/Subtitles`, body\)/);
    assert.match(api, /deleteSubtitle\(id, index\) \{ return this\.del\(`\/Videos\/\$\{id\}\/Subtitles\/\$\{index\}`\)/);
});

test('the remote search is off with a note, and asks no outside service', () => {
    assert.match(subtitles, /data-search-note>\$\{escapeHtml\(t\('subtitles\.no_provider'\)\)\}/);
    assert.doesNotMatch(subtitles, /RemoteSearch/);
    assert.match(en.subtitles.no_provider, /no subtitle provider/);
});

test('wiring, icons and translations', () => {
    // The subtitle editor is a tool on videos only
    assert.match(fns.metadataToolButtons({ Id: 'm1', Type: 'Movie' }), /data-action="edit-subtitles"/);
    assert.match(fns.metadataToolButtons({ Id: 'e1', Type: 'Episode' }), /data-action="edit-subtitles"/);
    assert.doesNotMatch(fns.metadataToolButtons({ Id: 's1', Type: 'Series' }), /edit-subtitles/);
    assert.match(router, /'edit-subtitles': el => openSubtitleEditor\(el\.dataset\.id\)/);
    assert.match(router, /'share-item': el => shareItem\(el\.dataset\)/);
    assert.match(router, /import \{ openSubtitleEditor \} from '\.\/subtitle-admin\.js'/);
    assert.match(app, /import '\.\/modules\/subtitle-admin\.js';/);
    for (const name of ['Download', 'Share2', 'Upload', 'Captions']) {
        assert.equal((icons.match(new RegExp(`\\b${name}\\b`, 'g')) || []).length, 2, name);
    }
    const keys = text => [...new Set([...text.matchAll(/t\('((?:detail|metadata|subtitles)\.[a-z_A-Z]+)'/g)].map(m => m[1]))];
    const used = [...keys(subtitles), ...keys(metadata), 'detail.download', 'detail.share', 'detail.share_copied',
        'detail.share_copy_prompt', 'subtitles.edit', 'metadata.person_Actor', 'metadata.person_GuestStar'];
    for (const key of used) {
        const [section, name] = key.split('.');
        assert.ok(en[section]?.[name], `en ${key}`);
        assert.ok(de[section]?.[name], `de ${key}`);
    }
});
