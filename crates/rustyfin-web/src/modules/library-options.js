// The library options dialog's editor, as Jellyfin Web's library options: metadata language and
// country, metadata savers, metadata and image fetchers per item type with their order, embedded
// titles, refresh interval, subtitle options and the content-type ones. The choices come from
// /Libraries/AvailableOptions. Fetcher lists live in `editor.lists`, so moving and ticking need
// no DOM reads; the plain fields are read from the form by name.
import { $, escapeHtml } from './util.js';

const LABEL = 'block text-xs font-semibold text-white/70 mb-1.5';
const CHECKBOX = 'w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]';
const REFRESH_DAYS = [0, 30, 60, 90, 180, 365];
const EMBEDDED_SUBTITLES = ['AllowAll', 'AllowText', 'AllowImage', 'AllowNone'];

// [option, kind, collection types it shows for (empty: all)]
const FIELDS = [
    ['EnableRealtimeMonitor', 'bool', []],
    ['SaveLocalMetadata', 'bool', []],
    ['AutomaticRefreshIntervalDays', 'days', []],
    ['EnableEmbeddedTitles', 'bool', ['movies', 'homevideos', 'mixed', '']],
    ['EnableEmbeddedExtrasTitles', 'bool', ['movies', 'tvshows', 'mixed', '']],
    ['EnableEmbeddedEpisodeInfos', 'bool', ['tvshows', 'mixed', '']],
    ['EnableAutomaticSeriesGrouping', 'bool', ['tvshows', 'mixed', '']],
    ['SeasonZeroDisplayName', 'text', ['tvshows', 'mixed', '']],
    ['AutomaticallyAddToCollection', 'bool', ['movies', 'mixed', '']],
    ['EnableLUFSScan', 'bool', ['music']],
    ['EnablePhotos', 'bool', ['homevideos', 'mixed', '']],
    ['SaveTrickplayWithMedia', 'bool', []],
    ['SaveSubtitlesWithMedia', 'bool', []],
    ['SkipSubtitlesIfEmbeddedSubtitlesPresent', 'bool', []],
    ['SkipSubtitlesIfAudioTrackMatches', 'bool', []],
    ['RequirePerfectSubtitleMatch', 'bool', []],
    ['AllowEmbeddedSubtitles', 'embedded', []],
];

// { available, lists: { key: [{ name, enabled }] }, saved } of the open dialog
let editor = null;

// A fetcher list in its saved order, then the ones not saved yet; ticked as saved, else by default
function orderedList(available, order, enabled) {
    const names = available.map(f => f.Name);
    const ordered = [...(order || []).filter(n => names.includes(n)), ...names.filter(n => !(order || []).includes(n))];
    return ordered.map(name => ({
        name,
        enabled: enabled ? enabled.includes(name) : !!available.find(f => f.Name === name)?.DefaultEnabled,
    }));
}

// The editor's model for saved options `o` and the server's choices `available`
function buildEditor(o, available) {
    const lists = {};
    lists.savers = (available.MetadataSavers || []).map(f => ({
        name: f.Name,
        enabled: Array.isArray(o.MetadataSavers) ? o.MetadataSavers.includes(f.Name) : !!f.DefaultEnabled,
    }));
    for (const type of available.TypeOptions || []) {
        const saved = (o.TypeOptions || []).find(x => x.Type === type.Type);
        lists[`meta:${type.Type}`] = orderedList(type.MetadataFetchers || [], saved?.MetadataFetcherOrder, saved?.MetadataFetchers);
        lists[`image:${type.Type}`] = orderedList(type.ImageFetchers || [], saved?.ImageFetcherOrder, saved?.ImageFetchers);
    }
    return { available, lists, saved: o };
}

function listHtml(key) {
    const list = editor.lists[key] || [];
    if (!list.length) return `<p class="text-[11px] text-white/30">${escapeHtml(t('library_options.none_available'))}</p>`;
    return list.map((f, i) => `
        <div class="flex items-center gap-2 py-0.5">
            <input type="checkbox" id="lo-${escapeHtml(key)}-${i}" class="${CHECKBOX}" data-lo-toggle="${escapeHtml(key)}" data-index="${i}" ${f.enabled ? 'checked' : ''}>
            <label for="lo-${escapeHtml(key)}-${i}" class="flex-1 text-sm text-white/70">${escapeHtml(f.name)}</label>
            ${list.length > 1 && key !== 'savers' ? `
            <button type="button" class="text-white/40 hover:text-white text-xs px-1" data-lo-move="${escapeHtml(key)}" data-index="${i}" data-by="-1" ${i === 0 ? 'disabled' : ''} aria-label="${escapeHtml(t('library_options.move_up', { name: f.name }))}">▲</button>
            <button type="button" class="text-white/40 hover:text-white text-xs px-1" data-lo-move="${escapeHtml(key)}" data-index="${i}" data-by="1" ${i === list.length - 1 ? 'disabled' : ''} aria-label="${escapeHtml(t('library_options.move_down', { name: f.name }))}">▼</button>` : ''}
        </div>`).join('');
}

function section(titleKey, body) {
    return `<fieldset class="border-t border-white/10 pt-3 space-y-2"><legend class="text-xs font-bold text-white/80 pr-2">${escapeHtml(t(titleKey))}</legend>${body}</fieldset>`;
}

function selectHtml(name, values, selected, label) {
    return `<select id="lo-${name}" name="${name}" class="jf-input w-full">${values.map(v => `<option value="${escapeHtml(v)}"${String(v) === String(selected ?? '') ? ' selected' : ''}>${escapeHtml(label(v))}</option>`).join('')}</select>`;
}

function fieldHtml([name, kind], o) {
    const label = escapeHtml(t(`library_options.${name}`));
    if (kind === 'bool') {
        return `<div class="flex items-center gap-3"><input type="checkbox" id="lo-${name}" name="${name}" class="${CHECKBOX}" ${o[name] ? 'checked' : ''}>
            <label for="lo-${name}" class="text-sm text-white/70">${label}</label></div>`;
    }
    let input;
    if (kind === 'days') input = selectHtml(name, REFRESH_DAYS, o[name] ?? 0, d => (d ? t('library_options.every_days', { count: d }) : t('library_options.never')));
    else if (kind === 'embedded') input = selectHtml(name, EMBEDDED_SUBTITLES, o[name] || 'AllowAll', v => t(`library_options.${v}`));
    else input = `<input id="lo-${name}" name="${name}" class="jf-input w-full" value="${escapeHtml(o[name] ?? '')}">`;
    return `<div><label for="lo-${name}" class="${LABEL}">${label}</label>${input}</div>`;
}

function cultureSelect(name, list, selected, code) {
    const options = [['', t('library_options.server_default')], ...list.map(c => [code(c), c.DisplayName || c.Name || code(c)])];
    if (selected && !options.some(([v]) => v === selected)) options.push([selected, selected]);
    return `<select id="lo-${name}" name="${name}" class="jf-input w-full">${options.map(([v, l]) => `<option value="${escapeHtml(v)}"${v === (selected || '') ? ' selected' : ''}>${escapeHtml(l)}</option>`).join('')}</select>`;
}

// The editor's markup for the options `o` of a library of `collectionType`
function editorHtml(o, collectionType, cultures, countries) {
    const type = (collectionType || '').toLowerCase();
    const shown = FIELDS.filter(([, , types]) => !types.length || types.includes(type));
    const typeLists = (editor.available.TypeOptions || []).map(x => `
        <div class="grid grid-cols-2 gap-3">
            <div><p class="${LABEL}">${escapeHtml(t('library_options.metadata_fetchers', { type: x.Type }))}</p><div data-lo-list="meta:${escapeHtml(x.Type)}">${listHtml(`meta:${x.Type}`)}</div></div>
            <div><p class="${LABEL}">${escapeHtml(t('library_options.image_fetchers', { type: x.Type }))}</p><div data-lo-list="image:${escapeHtml(x.Type)}">${listHtml(`image:${x.Type}`)}</div></div>
        </div>`).join('');
    return `<p class="text-[11px] text-white/40">${escapeHtml(t('library_options.applied_hint'))}</p>
        ${section('library_options.metadata', `
            <div class="grid grid-cols-2 gap-3">
                <div><label for="lo-PreferredMetadataLanguage" class="${LABEL}">${escapeHtml(t('library_options.PreferredMetadataLanguage'))}</label>
                    ${cultureSelect('PreferredMetadataLanguage', cultures, o.PreferredMetadataLanguage, c => c.TwoLetterISOLanguageName)}</div>
                <div><label for="lo-MetadataCountryCode" class="${LABEL}">${escapeHtml(t('library_options.MetadataCountryCode'))}</label>
                    ${cultureSelect('MetadataCountryCode', countries, o.MetadataCountryCode, c => c.TwoLetterISORegionName)}</div>
            </div>
            <div><p class="${LABEL}">${escapeHtml(t('library_options.metadata_savers'))}</p><div data-lo-list="savers">${listHtml('savers')}</div></div>
            ${typeLists}`)}
        ${section('library_options.library', shown.map(f => fieldHtml(f, o)).join(''))}`;
}

function rerenderList(key) {
    const box = document.querySelector(`[data-lo-list="${CSS.escape(key)}"]`);
    if (box) box.innerHTML = listHtml(key);
}

// Ticks or moves a fetcher; `el` is the clicked checkbox or arrow
function editorClick(el) {
    if (!editor) return;
    if (el.dataset.loToggle) {
        const f = editor.lists[el.dataset.loToggle]?.[Number(el.dataset.index)];
        if (f) f.enabled = !!el.checked;
        return;
    }
    if (el.dataset.loMove) {
        const key = el.dataset.loMove;
        moveFetcher(key, Number(el.dataset.index), Number(el.dataset.by));
        rerenderList(key);
    }
}

function moveFetcher(key, index, by) {
    const list = editor?.lists[key];
    const to = index + by;
    if (!list || to < 0 || to >= list.length) return;
    [list[index], list[to]] = [list[to], list[index]];
}

// Loads the choices and fills `box` with the editor for options `o`
async function renderLibraryOptionsEditor(box, o, collectionType) {
    if (!box) return;
    const query = collectionType ? `?libraryContentType=${encodeURIComponent(collectionType)}` : '';
    const [available, cultures, countries] = await Promise.all([
        API.get(`/Libraries/AvailableOptions${query}`).catch(() => ({})),
        API.get('/Localization/Cultures').catch(() => []),
        API.get('/Localization/Countries').catch(() => []),
    ]);
    editor = buildEditor(o, available || {});
    box.innerHTML = editorHtml(o, collectionType, Array.isArray(cultures) ? cultures : [], Array.isArray(countries) ? countries : []);
    box.onclick = e => { const el = e.target.closest?.('[data-lo-toggle], [data-lo-move]'); if (el) editorClick(el); };
    box.onchange = box.onclick;
}

// The editor's options, to save over the loaded ones; {} when no editor is open
function readLibraryOptionsEditor(form) {
    if (!editor) return {};
    const out = {};
    const els = form.elements;
    for (const name of ['PreferredMetadataLanguage', 'MetadataCountryCode']) {
        if (els[name]) out[name] = els[name].value || null;
    }
    for (const [name, kind] of FIELDS) {
        const el = els[name];
        if (!el) continue;
        if (kind === 'bool') out[name] = !!el.checked;
        else if (kind === 'days') out[name] = parseInt(el.value, 10) || 0;
        else out[name] = el.value;
    }
    const on = key => (editor.lists[key] || []).filter(f => f.enabled).map(f => f.name);
    const order = key => (editor.lists[key] || []).map(f => f.name);
    if (editor.lists.savers.length) out.MetadataSavers = on('savers');
    const types = (editor.available.TypeOptions || []).map(x => x.Type);
    const kept = (editor.saved.TypeOptions || []).filter(x => !types.includes(x.Type));
    out.TypeOptions = [...kept, ...types.map(type => ({
        ...((editor.saved.TypeOptions || []).find(x => x.Type === type) || { ImageOptions: [] }),
        Type: type,
        MetadataFetchers: on(`meta:${type}`),
        MetadataFetcherOrder: order(`meta:${type}`),
        ImageFetchers: on(`image:${type}`),
        ImageFetcherOrder: order(`image:${type}`),
    }))];
    return out;
}

function closeLibraryOptionsEditor() {
    editor = null;
    const box = $('library-options-more');
    if (box) box.innerHTML = '';
}

export {
    renderLibraryOptionsEditor, readLibraryOptionsEditor, closeLibraryOptionsEditor, buildEditor, editorHtml, moveFetcher,
    FIELDS,
};
