// An administrator's metadata tools on the item page, as Jellyfin Web has them: the refresh dialog with
// its modes, Identify, the image editor and the metadata editor. They share one dialog element.
import { $, icon, toast, escapeHtml } from './util.js';
import { currentDetailId, renderDetail } from './item.js';

// Jellyfin's lockable metadata fields (MetadataField), in its editor's order
const LOCKABLE_FIELDS = ['Name', 'Overview', 'Genres', 'OfficialRating', 'Cast', 'ProductionLocations', 'Runtime', 'Studios', 'Tags'];
// The image types an admin can upload, and the ones the provider offers
const IMAGE_TYPES = ['Primary', 'Backdrop', 'Logo', 'Thumb', 'Banner', 'Art', 'Disc'];
const REMOTE_IMAGE_TYPES = ['Primary', 'Backdrop', 'Logo'];

// The person kinds the editor offers (Jellyfin's PersonKind, the common ones)
const PERSON_TYPES = ['Actor', 'Director', 'Writer', 'Producer', 'GuestStar', 'Composer'];

const LABEL = 'block text-xs font-semibold text-white/70 mb-1';
const CHECKBOX = 'w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]';

// The refresh query for the dialog's mode, as Jellyfin Web's refresh dialog sends it. "Replace existing
// images" counts only for the two full modes.
function refreshQuery(mode, replaceImages) {
    if (mode === 'scan') {
        return { MetadataRefreshMode: 'Default', ImageRefreshMode: 'Default', ReplaceAllMetadata: false, ReplaceAllImages: false };
    }
    return {
        MetadataRefreshMode: 'FullRefresh', ImageRefreshMode: 'FullRefresh',
        ReplaceAllMetadata: mode === 'all', ReplaceAllImages: !!replaceImages,
    };
}

// "Drama, comedy, , drama" -> ['Drama', 'comedy']: no blanks, no repeats (ignoring case)
function splitList(text) {
    const out = [];
    for (const part of String(text || '').split(',')) {
        const entry = part.trim();
        if (entry && !out.some(seen => seen.toLowerCase() === entry.toLowerCase())) out.push(entry);
    }
    return out;
}

// The metadata editor's form values as Jellyfin's item update (POST /Items/{id}). A blank field clears it;
// an unticked field in "Fields a refresh may change" is locked. The sort title goes only when it was
// changed, as a blank one sets the sort name back to the title's. The provider ids the form shows go over
// the item's others; people without a name are dropped. The studios go as the item had them unless the
// field was edited, so a studio with a comma in its name stays whole.
function itemUpdateBody(values) {
    const text = v => String(v ?? '').trim() || null;
    const number = v => {
        const n = Number.parseFloat(v);
        return Number.isFinite(n) ? n : null;
    };
    const year = Number.parseInt(values.year, 10);
    const sortChanged = text(values.sortName) !== text(values.sortNameBefore);
    const day = v => (v ? `${v}T00:00:00.0000000Z` : null);
    const providerIds = { ...(values.providerIdsBefore || {}) };
    for (const [key, id] of Object.entries(values.providerIds || {})) providerIds[key] = String(id ?? '').trim();
    return {
        Name: text(values.name),
        OriginalTitle: text(values.originalTitle),
        ...(sortChanged ? { ForcedSortName: text(values.sortName) } : {}),
        Overview: text(values.overview),
        Taglines: text(values.tagline) ? [text(values.tagline)] : [],
        ProductionYear: Number.isFinite(year) ? year : null,
        PremiereDate: day(values.premiereDate),
        ...('endDate' in values ? { EndDate: day(values.endDate) } : {}),
        CommunityRating: number(values.communityRating),
        CriticRating: number(values.criticRating),
        OfficialRating: text(values.officialRating),
        Genres: splitList(values.genres),
        Tags: splitList(values.tags),
        Studios: (Array.isArray(values.studioNames) && String(values.studios ?? '') === String(values.studiosBefore ?? '')
            ? values.studioNames : splitList(values.studios)).map(Name => ({ Name })),
        People: (values.people || [])
            .map(p => ({ Name: text(p.name), Role: text(p.role), Type: p.type || 'Actor' }))
            .filter(p => p.Name),
        ProviderIds: providerIds,
        LockData: !!values.lockData,
        LockedFields: LOCKABLE_FIELDS.filter(field => !values.enabled?.[field]),
    };
}

// One search result of Identify, as a button that applies it
function searchResultHtml(result, index) {
    const year = result.ProductionYear ? ` (${result.ProductionYear})` : '';
    const poster = result.ImageUrl
        ? `<img src="${escapeHtml(result.ImageUrl)}" alt="" loading="lazy" class="w-16 h-24 object-cover rounded bg-white/5 shrink-0">`
        : `<div class="w-16 h-24 rounded bg-white/5 shrink-0"></div>`;
    return `<button type="button" data-result="${index}" class="flex gap-3 w-full text-left p-2 rounded-lg hover:bg-white/10">
        ${poster}
        <span class="min-w-0">
            <span class="block text-sm font-semibold text-white">${escapeHtml(result.Name || '')}${escapeHtml(year)}</span>
            <span class="block text-xs text-white/40">${escapeHtml(result.SearchProviderName || '')}</span>
            ${result.Overview ? `<span class="block text-xs text-white/60 line-clamp-3 mt-1">${escapeHtml(result.Overview)}</span>` : ''}
        </span>
    </button>`;
}

// One image the provider offers, with its size, language and votes, and a button that makes it the item's
function remoteImageHtml(image, index) {
    const facts = [];
    if (image.Width && image.Height) facts.push(t('metadata.image_size', { width: image.Width, height: image.Height }));
    if (image.Language) facts.push(image.Language);
    if (image.CommunityRating != null) {
        facts.push(t('metadata.votes', { rating: Number(image.CommunityRating).toFixed(1), count: image.VoteCount ?? 0 }));
    }
    return `<div class="flex flex-col gap-1">
        <img src="${escapeHtml(image.ThumbnailUrl || image.Url)}" alt="" loading="lazy" class="w-full aspect-[2/3] object-contain rounded bg-white/5">
        <span class="text-[11px] text-white/50">${escapeHtml(facts.join(' · '))}</span>
        <button type="button" data-download="${index}" class="jf-btn-ghost text-xs">${escapeHtml(t('metadata.download'))}</button>
    </div>`;
}

// The item's current images, each with a delete button
function currentImagesHtml(id, images) {
    if (!images.length) return `<p class="text-xs text-white/50">${escapeHtml(t('metadata.no_images'))}</p>`;
    return `<div class="grid grid-cols-3 sm:grid-cols-4 gap-3">${images.map(image => `<div class="flex flex-col gap-1">
        <img src="${escapeHtml(API.imageUrl(id, `${image.ImageType}/${image.ImageIndex ?? 0}`, { maxWidth: 240, tag: image.ImageTag }))}" alt="" loading="lazy" class="w-full aspect-[2/3] object-contain rounded bg-white/5">
        <span class="flex items-center justify-between gap-1 text-[11px] text-white/60">
            ${escapeHtml(typeLabel(image.ImageType))}${image.ImageIndex ? ` ${image.ImageIndex + 1}` : ''}
            <button type="button" class="hover:text-rose-400" title="${escapeHtml(t('metadata.delete_image'))}" aria-label="${escapeHtml(t('metadata.delete_image'))}" data-delete-type="${escapeHtml(image.ImageType)}" data-delete-index="${image.ImageIndex ?? 0}">${icon('Trash2', { size: 14 })}</button>
        </span>
    </div>`).join('')}</div>`;
}

function typeLabel(type) {
    const key = `metadata.type_${type}`;
    const label = t(key);
    return label === key ? type : label;
}

function typeOptions(types, selected) {
    return types.map(type => `<option value="${type}"${type === selected ? ' selected' : ''}>${escapeHtml(typeLabel(type))}</option>`).join('');
}

// Opens the shared dialog with `title` and `bodyHtml`; [data-close] elements close it
function openDialog(title, bodyHtml) {
    let dlg = $('metadata-admin-modal');
    if (!dlg) {
        dlg = document.createElement('dialog');
        dlg.id = 'metadata-admin-modal';
        dlg.className = 'modal bg-black/70 backdrop-blur-md';
        dlg.addEventListener('close', () => dlg.classList.remove('modal-open'));
        document.body.append(dlg);
    }
    dlg.innerHTML = `<div class="modal-box max-w-2xl jf-menu rounded-2xl p-6">
        <div class="flex items-center justify-between mb-4 border-b border-white/10 pb-3">
            <h3 class="text-base font-bold text-white flex items-center gap-2">
                <span class="w-2 h-2 rounded-full bg-[#00a4dc]"></span><span>${escapeHtml(title)}</span>
            </h3>
            <button type="button" data-close class="text-white/40 hover:text-white text-xl leading-none" aria-label="${escapeHtml(t('common.close'))}">×</button>
        </div>
        ${bodyHtml}
    </div>
    <form method="dialog" class="modal-backdrop"><button>${escapeHtml(t('common.backdrop_close'))}</button></form>`;
    dlg.querySelectorAll('[data-close]').forEach(button => button.addEventListener('click', closeDialog));
    dlg.classList.add('modal-open');
    if (typeof dlg.showModal === 'function') {
        try { if (!dlg.open) dlg.showModal(); } catch { dlg.setAttribute('open', ''); }
    } else {
        dlg.setAttribute('open', '');
    }
    return dlg;
}

function closeDialog() {
    const dlg = $('metadata-admin-modal');
    if (!dlg) return;
    dlg.classList.remove('modal-open');
    if (typeof dlg.close === 'function' && dlg.open) dlg.close();
    else dlg.removeAttribute('open');
}

function actionsHtml(submitLabel) {
    return `<div class="flex justify-end gap-2 pt-3 border-t border-white/10">
        <button type="button" data-close class="jf-btn-ghost text-xs">${escapeHtml(t('common.cancel'))}</button>
        <button type="submit" class="jf-btn-primary text-xs">${escapeHtml(submitLabel)}</button>
    </div>`;
}

// The item, fresh from the server; a toast and null when it can't be read
async function loadItem(id) {
    API.invalidateCache(`/Items/${id}`);
    try {
        return await API.getItem(id);
    } catch (e) {
        toast(t('metadata.load_failed', { message: e.message }), 'error');
        return null;
    }
}

// After a change, the lists and the open detail page show the item's new metadata and images
async function itemChanged(id) {
    API.invalidateCache('/Items');
    API.invalidateCache('/Shows');
    if (currentDetailId === id) await renderDetail(id);
}

// The refresh dialog: Jellyfin's three modes and "Replace existing images". `refresh(query)` runs it.
function openRefreshDialog(refresh) {
    const dlg = openDialog(t('metadata.refresh_title'), `<form class="space-y-4" data-form>
        <div>
            <label class="${LABEL}" for="md-refresh-mode">${escapeHtml(t('metadata.refresh_mode'))}</label>
            <select id="md-refresh-mode" name="mode" class="jf-input w-full">
                <option value="scan" selected>${escapeHtml(t('metadata.mode_scan'))}</option>
                <option value="missing">${escapeHtml(t('metadata.mode_missing'))}</option>
                <option value="all">${escapeHtml(t('metadata.mode_all'))}</option>
            </select>
        </div>
        <label class="flex items-center gap-3 text-sm text-white/70" data-replace-images hidden>
            <input type="checkbox" name="replaceImages" class="${CHECKBOX}"> ${escapeHtml(t('metadata.replace_images'))}
        </label>
        <p class="text-xs text-white/50">${escapeHtml(t('metadata.refresh_help'))}</p>
        ${actionsHtml(t('metadata.refresh'))}
    </form>`);
    const form = dlg.querySelector('[data-form]');
    const syncReplace = () => { dlg.querySelector('[data-replace-images]').hidden = form.mode.value === 'scan'; };
    form.mode.addEventListener('change', syncReplace);
    form.addEventListener('submit', e => {
        e.preventDefault();
        const query = refreshQuery(form.mode.value, form.replaceImages.checked);
        closeDialog();
        refresh(query);
    });
}

// Identify: searches the provider by name and year, or by TMDb id, and applies the chosen result
async function openIdentifyDialog(id) {
    const item = await loadItem(id);
    if (!item) return;
    const dlg = openDialog(t('metadata.identify'), `<form class="space-y-3" data-form>
        <p class="text-xs text-white/50">${escapeHtml(t('metadata.identify_help'))}</p>
        <div class="grid grid-cols-1 sm:grid-cols-[1fr_6rem_8rem] gap-3">
            <div><label class="${LABEL}" for="md-id-name">${escapeHtml(t('metadata.name'))}</label>
                <input id="md-id-name" name="name" class="jf-input w-full" value="${escapeHtml(item.Name || '')}"></div>
            <div><label class="${LABEL}" for="md-id-year">${escapeHtml(t('metadata.year'))}</label>
                <input id="md-id-year" name="year" type="number" class="jf-input w-full" value="${escapeHtml(item.ProductionYear ?? '')}"></div>
            <div><label class="${LABEL}" for="md-id-tmdb">${escapeHtml(t('metadata.tmdb_id'))}</label>
                <input id="md-id-tmdb" name="tmdb" class="jf-input w-full" value="${escapeHtml(item.ProviderIds?.Tmdb ?? '')}"></div>
        </div>
        <label class="flex items-center gap-3 text-sm text-white/70">
            <input type="checkbox" name="replaceImages" class="${CHECKBOX}" checked> ${escapeHtml(t('metadata.replace_images'))}
        </label>
        ${actionsHtml(t('metadata.search'))}
        <div data-results class="space-y-1 max-h-96 overflow-y-auto" aria-live="polite"></div>
    </form>`);
    const form = dlg.querySelector('[data-form]');
    const box = dlg.querySelector('[data-results]');
    let results = [];
    form.addEventListener('submit', async e => {
        e.preventDefault();
        const info = { Name: form.name.value.trim() };
        const year = Number.parseInt(form.year.value, 10);
        if (Number.isFinite(year)) info.Year = year;
        if (form.tmdb.value.trim()) info.ProviderIds = { Tmdb: form.tmdb.value.trim() };
        box.innerHTML = `<p class="text-xs text-white/50">${escapeHtml(t('metadata.searching'))}</p>`;
        try {
            results = await API.remoteSearch(item.Type, info, id);
            box.innerHTML = results.length
                ? results.map(searchResultHtml).join('')
                : `<p class="text-xs text-white/50">${escapeHtml(t('metadata.no_results'))}</p>`;
        } catch (err) {
            box.innerHTML = '';
            toast(t('metadata.search_failed', { message: err.message }), 'error');
        }
    });
    box.addEventListener('click', async e => {
        const result = results[e.target.closest('[data-result]')?.dataset.result];
        if (!result || !confirm(t('metadata.apply_confirm', { name: result.Name }))) return;
        closeDialog();
        try {
            await API.applySearchResult(id, result, form.replaceImages.checked);
            toast(t('metadata.identified', { name: result.Name }));
            await itemChanged(id);
        } catch (err) {
            toast(t('metadata.identify_failed', { message: err.message }), 'error');
        }
    });
}

// The image editor: the item's images with delete, an upload, and the provider's images to choose from
async function openImageEditor(id) {
    const item = await loadItem(id);
    if (!item) return;
    const canSearch = item.Type === 'Movie' || item.Type === 'Series';
    const dlg = openDialog(t('metadata.edit_images'), `<div class="space-y-5">
        <section>
            <h4 class="text-sm font-semibold text-white mb-2">${escapeHtml(t('metadata.current_images'))}</h4>
            <div data-current></div>
        </section>
        <form data-upload class="flex flex-wrap items-end gap-3 border-t border-white/10 pt-4">
            <div><label class="${LABEL}" for="md-up-type">${escapeHtml(t('metadata.image_type'))}</label>
                <select id="md-up-type" name="type" class="jf-input">${typeOptions(IMAGE_TYPES, 'Primary')}</select></div>
            <div class="flex-1 min-w-[12rem]"><label class="${LABEL}" for="md-up-file">${escapeHtml(t('metadata.upload_image'))}</label>
                <input id="md-up-file" name="file" type="file" accept="image/*" required class="jf-input w-full"></div>
            <button type="submit" class="jf-btn-primary text-xs">${escapeHtml(t('metadata.upload_image'))}</button>
        </form>
        ${canSearch ? `<section class="border-t border-white/10 pt-4">
            <form data-remote class="flex flex-wrap items-end gap-3 mb-3">
                <div><label class="${LABEL}" for="md-rm-type">${escapeHtml(t('metadata.image_type'))}</label>
                    <select id="md-rm-type" name="type" class="jf-input">${typeOptions(REMOTE_IMAGE_TYPES, 'Primary')}</select></div>
                <label class="flex items-center gap-2 text-xs text-white/70 pb-2">
                    <input type="checkbox" name="allLanguages" class="${CHECKBOX}"> ${escapeHtml(t('metadata.all_languages'))}</label>
                <button type="submit" class="jf-btn-ghost text-xs">${escapeHtml(t('metadata.browse_online'))}</button>
            </form>
            <div data-remote-list class="grid grid-cols-3 sm:grid-cols-4 gap-3 max-h-96 overflow-y-auto" aria-live="polite"></div>
        </section>` : ''}
    </div>`);
    const current = dlg.querySelector('[data-current]');
    const showCurrent = async () => {
        try {
            current.innerHTML = currentImagesHtml(id, await API.getItemImages(id));
        } catch (err) {
            current.innerHTML = '';
            toast(t('metadata.image_failed', { message: err.message }), 'error');
        }
    };
    const changed = async message => {
        toast(message);
        await showCurrent();
        await itemChanged(id);
    };
    current.addEventListener('click', async e => {
        const button = e.target.closest('[data-delete-type]');
        if (!button || !confirm(t('metadata.delete_image_confirm'))) return;
        try {
            await API.deleteItemImage(id, button.dataset.deleteType, button.dataset.deleteIndex);
            await changed(t('metadata.image_deleted'));
        } catch (err) {
            toast(t('metadata.image_failed', { message: err.message }), 'error');
        }
    });
    const upload = dlg.querySelector('[data-upload]');
    upload.addEventListener('submit', async e => {
        e.preventDefault();
        const file = upload.file.files[0];
        if (!file) return;
        try {
            await API.uploadItemImage(id, upload.type.value, file);
            upload.reset();
            await changed(t('metadata.uploaded'));
        } catch (err) {
            toast(t('metadata.upload_failed', { message: err.message }), 'error');
        }
    });
    const remote = dlg.querySelector('[data-remote]');
    if (remote) {
        const list = dlg.querySelector('[data-remote-list]');
        let images = [];
        remote.addEventListener('submit', async e => {
            e.preventDefault();
            list.innerHTML = `<p class="text-xs text-white/50 col-span-full">${escapeHtml(t('metadata.searching'))}</p>`;
            try {
                const answer = await API.getRemoteImages(id, {
                    type: remote.type.value, includeAllLanguages: remote.allLanguages.checked, limit: 60,
                });
                images = answer.Images || [];
                list.innerHTML = images.length
                    ? images.map(remoteImageHtml).join('')
                    : `<p class="text-xs text-white/50 col-span-full">${escapeHtml(t('metadata.no_remote_images'))}</p>`;
            } catch (err) {
                list.innerHTML = '';
                toast(t('metadata.image_failed', { message: err.message }), 'error');
            }
        });
        list.addEventListener('click', async e => {
            const image = images[e.target.closest('[data-download]')?.dataset.download];
            if (!image) return;
            try {
                await API.downloadRemoteImage(id, image.Type, image.Url);
                await changed(t('metadata.downloaded'));
            } catch (err) {
                toast(t('metadata.image_failed', { message: err.message }), 'error');
            }
        });
    }
    await showCurrent();
}

// The metadata editor: title, overview, dates, ratings, genres, tags and the locks
// A person kind's label; a kind without a translation shows as the server names it
function personTypeLabel(kind) {
    const key = `metadata.person_${kind}`;
    const label = t(key);
    return label === key ? kind : label;
}

// One row of the editor's people: name, role and kind, and a button that removes the row. A kind
// the list doesn't have (Conductor, Lyricist, ...) is added as an option, so it stays as it was.
function personRowHtml(person = {}) {
    const type = String(person.Type || 'Actor');
    const kinds = PERSON_TYPES.includes(type) ? PERSON_TYPES : [...PERSON_TYPES, type];
    return `<div class="grid grid-cols-[1fr_1fr_auto_auto] gap-2 items-center" data-person>
        <input class="jf-input w-full" data-person-name aria-label="${escapeHtml(t('metadata.person_name'))}" placeholder="${escapeHtml(t('metadata.person_name'))}" value="${escapeHtml(person.Name ?? '')}">
        <input class="jf-input w-full" data-person-role aria-label="${escapeHtml(t('metadata.person_role'))}" placeholder="${escapeHtml(t('metadata.person_role'))}" value="${escapeHtml(person.Role ?? '')}">
        <select class="jf-input" data-person-type aria-label="${escapeHtml(t('metadata.person_type'))}">${kinds.map(kind =>
            `<option value="${escapeHtml(kind)}"${kind === type ? ' selected' : ''}>${escapeHtml(personTypeLabel(kind))}</option>`).join('')}</select>
        <button type="button" class="jf-btn-ghost !px-2 text-xs" data-remove-person title="${escapeHtml(t('metadata.remove_person'))}" aria-label="${escapeHtml(t('metadata.remove_person'))}">${icon('X', { size: 14 })}</button>
    </div>`;
}

// The provider id fields the server offers for the item (MetadataEditorInfo.ExternalIdInfos)
function providerIdsHtml(infos, ids = {}) {
    if (!infos.length) return '';
    const value = key => Object.entries(ids).find(([k]) => k.toLowerCase() === key.toLowerCase())?.[1] ?? '';
    return `<fieldset class="space-y-2 pt-2 border-t border-white/10">
        <legend class="${LABEL}">${escapeHtml(t('metadata.provider_ids'))}</legend>
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">${infos.map(info => `<div>
            <label class="${LABEL}" for="md-ed-pid-${escapeHtml(info.Key)}">${escapeHtml(info.Name)}</label>
            <input id="md-ed-pid-${escapeHtml(info.Key)}" class="jf-input w-full" data-provider-id="${escapeHtml(info.Key)}" value="${escapeHtml(value(info.Key))}"></div>`).join('')}</div>
    </fieldset>`;
}

async function openMetadataEditor(id) {
    const [item, editor] = await Promise.all([
        loadItem(id),
        API.getMetadataEditor(id).catch(() => ({ ExternalIdInfos: [] })),
    ]);
    if (!item) return;
    const hasEndDate = item.Type === 'Series';
    const field = (name, label, value, attrs = '') => `<div><label class="${LABEL}" for="md-ed-${name}">${escapeHtml(label)}</label>
        <input id="md-ed-${name}" name="${name}" class="jf-input w-full" value="${escapeHtml(value ?? '')}" ${attrs}></div>`;
    const locked = new Set(item.LockedFields || []);
    const dlg = openDialog(t('metadata.edit_metadata'), `<form class="space-y-3 max-h-[70vh] overflow-y-auto pr-1" data-form>
        ${field('name', t('metadata.title'), item.Name, 'required')}
        <div class="grid grid-cols-1 sm:grid-cols-2 gap-3">
            ${field('originalTitle', t('metadata.original_title'), item.OriginalTitle)}
            ${field('sortName', t('metadata.sort_title'), item.ForcedSortName,
                item.ForcedSortName ? '' : `placeholder="${escapeHtml(item.SortName ?? '')}"`)}
        </div>
        <div><label class="${LABEL}" for="md-ed-overview">${escapeHtml(t('metadata.overview'))}</label>
            <textarea id="md-ed-overview" name="overview" rows="5" class="jf-input w-full">${escapeHtml(item.Overview || '')}</textarea></div>
        ${field('tagline', t('metadata.tagline'), item.Taglines?.[0])}
        <div class="grid grid-cols-2 sm:grid-cols-3 gap-3">
            ${field('year', t('metadata.year'), item.ProductionYear, 'type="number"')}
            ${field('premiereDate', t('metadata.release_date'), item.PremiereDate?.slice(0, 10), 'type="date"')}
            ${hasEndDate ? field('endDate', t('metadata.end_date'), item.EndDate?.slice(0, 10), 'type="date"') : ''}
            ${field('officialRating', t('metadata.parental_rating'), item.OfficialRating)}
            ${field('communityRating', t('metadata.community_rating'), item.CommunityRating, 'type="number" step="0.1" min="0" max="10"')}
            ${field('criticRating', t('metadata.critic_rating'), item.CriticRating, 'type="number" step="1" min="0" max="100"')}
        </div>
        ${field('genres', t('metadata.genres'), (item.Genres || []).join(', '), `aria-describedby="md-ed-list-help"`)}
        ${field('tags', t('metadata.tags'), (item.Tags || []).join(', '), `aria-describedby="md-ed-list-help"`)}
        ${field('studios', t('metadata.studios'), (item.Studios || []).map(s => s.Name).join(', '), `aria-describedby="md-ed-list-help"`)}
        <p id="md-ed-list-help" class="text-xs text-white/40">${escapeHtml(t('metadata.list_help'))}</p>
        <fieldset class="space-y-2 pt-2 border-t border-white/10">
            <legend class="${LABEL}">${escapeHtml(t('metadata.people'))}</legend>
            <div class="space-y-2" data-people>${(item.People || []).map(personRowHtml).join('')}</div>
            <button type="button" class="jf-btn-ghost text-xs" data-add-person>${icon('Plus', { size: 14 })} ${escapeHtml(t('metadata.add_person'))}</button>
        </fieldset>
        ${providerIdsHtml(editor?.ExternalIdInfos || [], item.ProviderIds)}
        <label class="flex items-center gap-3 text-sm text-white/70 pt-2 border-t border-white/10">
            <input type="checkbox" name="lockData" class="${CHECKBOX}"${item.LockData ? ' checked' : ''}> ${escapeHtml(t('metadata.lock_item'))}
        </label>
        <fieldset data-fields${item.LockData ? ' hidden' : ''}>
            <legend class="${LABEL}">${escapeHtml(t('metadata.enabled_fields'))}</legend>
            <div class="grid grid-cols-2 sm:grid-cols-3 gap-2">${LOCKABLE_FIELDS.map(name => `<label class="flex items-center gap-2 text-xs text-white/70">
                <input type="checkbox" name="enabled-${name}" class="${CHECKBOX}"${locked.has(name) ? '' : ' checked'}> ${escapeHtml(t(`metadata.field_${name}`))}</label>`).join('')}</div>
        </fieldset>
        ${actionsHtml(t('metadata.save'))}
    </form>`);
    const form = dlg.querySelector('[data-form]');
    form.lockData.addEventListener('change', () => { dlg.querySelector('[data-fields]').hidden = form.lockData.checked; });
    const people = dlg.querySelector('[data-people]');
    dlg.querySelector('[data-add-person]').addEventListener('click', () => {
        people.insertAdjacentHTML('beforeend', personRowHtml());
        people.lastElementChild.querySelector('[data-person-name]').focus();
    });
    people.addEventListener('click', e => e.target.closest('[data-remove-person]')?.closest('[data-person]').remove());
    form.addEventListener('submit', async e => {
        e.preventDefault();
        const values = Object.fromEntries(['name', 'originalTitle', 'sortName', 'overview', 'tagline', 'year', 'premiereDate',
            'officialRating', 'communityRating', 'criticRating', 'genres', 'tags', 'studios'].map(name => [name, form[name].value]));
        if (hasEndDate) values.endDate = form.endDate.value;
        values.sortNameBefore = item.ForcedSortName;
        values.studioNames = (item.Studios || []).map(s => s.Name);
        values.studiosBefore = values.studioNames.join(', ');
        values.people = [...people.querySelectorAll('[data-person]')].map(row => ({
            name: row.querySelector('[data-person-name]').value,
            role: row.querySelector('[data-person-role]').value,
            type: row.querySelector('[data-person-type]').value,
        }));
        values.providerIdsBefore = item.ProviderIds || {};
        values.providerIds = Object.fromEntries([...form.querySelectorAll('[data-provider-id]')].map(input => [input.dataset.providerId, input.value]));
        values.lockData = form.lockData.checked;
        values.enabled = Object.fromEntries(LOCKABLE_FIELDS.map(name => [name, form[`enabled-${name}`].checked]));
        try {
            await API.updateItem(id, itemUpdateBody(values));
            closeDialog();
            toast(t('metadata.saved'));
            await itemChanged(id);
        } catch (err) {
            toast(t('metadata.save_failed', { message: err.message }), 'error');
        }
    });
}

export {
    openRefreshDialog, openIdentifyDialog, openImageEditor, openMetadataEditor, refreshQuery, itemUpdateBody,
    openDialog, closeDialog, actionsHtml, loadItem, itemChanged, LABEL, CHECKBOX,
};
