// An administrator's subtitle editor on a video's item page, as Jellyfin Web's: the video's subtitle
// streams with a delete button on each external file, and an upload form. The remote search stays off
// with a note: the server has no subtitle provider, so Jellyfin's search would always come back empty.
import { toast, escapeHtml, icon } from './util.js';
import { openDialog, loadItem, itemChanged, LABEL, CHECKBOX } from './metadata-admin.js';

// The formats the server saves beside a video
const UPLOAD_FORMATS = ['srt', 'vtt', 'ass', 'ssa'];
// The largest file the server takes
const UPLOAD_MAX_BYTES = 10 * 1024 * 1024;

// A subtitle file's format, from its name; '' for one the server doesn't take
function subtitleFormat(fileName) {
    const name = String(fileName || '');
    const dot = name.lastIndexOf('.');
    const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
    return UPLOAD_FORMATS.includes(ext) ? ext : '';
}

// Jellyfin's UploadSubtitleDto
function uploadBody({ language, format, forced, hearingImpaired, data }) {
    return {
        Language: String(language || '').trim(), Format: format, IsForced: !!forced,
        IsHearingImpaired: !!hearingImpaired, Data: data,
    };
}

// The video's subtitle streams, numbered as PlaybackInfo numbers them (external files first)
function subtitleStreams(item) {
    return (item.MediaSources?.[0]?.MediaStreams || item.MediaStreams || []).filter(s => s.Type === 'Subtitle');
}

// One stream: its title, kind and format; an external file has a delete button
function subtitleRowHtml(stream) {
    const title = stream.DisplayTitle || stream.Title || stream.Language || t('subtitles.unknown_language');
    const facts = [
        stream.IsExternal ? t('subtitles.external') : t('subtitles.embedded'),
        stream.IsForced ? t('subtitles.forced') : '',
        stream.Codec ? String(stream.Codec).toUpperCase() : '',
    ].filter(Boolean).join(' · ');
    const del = stream.IsExternal ? `<button type="button" class="jf-btn-ghost !px-2 text-xs hover:!text-rose-400" data-delete-subtitle="${Number(stream.Index)}" title="${escapeHtml(t('subtitles.delete'))}" aria-label="${escapeHtml(t('subtitles.delete'))}: ${escapeHtml(title)}">${icon('Trash2', { size: 14 })}</button>` : '';
    return `<li class="flex items-center justify-between gap-3 py-2 border-b border-white/5">
        <div class="min-w-0"><div class="text-sm text-white truncate">${escapeHtml(title)}</div>
        <div class="text-xs text-white/40">${escapeHtml(facts)}</div></div>${del}</li>`;
}

// The file's contents in base64, as the upload sends them
function fileBase64(file) {
    return new Promise((resolve, reject) => {
        const reader = new FileReader();
        reader.onload = () => resolve(String(reader.result).split(',')[1] || '');
        reader.onerror = () => reject(reader.error);
        reader.readAsDataURL(file);
    });
}

async function openSubtitleEditor(id) {
    const [item, cultures] = await Promise.all([loadItem(id), API.getCultures().catch(() => [])]);
    if (!item) return;
    const streams = subtitleStreams(item);
    const languages = (cultures || []).filter(c => c.ThreeLetterISOLanguageName)
        .map(c => `<option value="${escapeHtml(c.ThreeLetterISOLanguageName)}">${escapeHtml(c.DisplayName || c.Name || '')}</option>`).join('');
    const dlg = openDialog(t('subtitles.edit'), `<div class="space-y-4 max-h-[70vh] overflow-y-auto pr-1">
        <section>
            <h4 class="${LABEL}">${escapeHtml(t('subtitles.current'))}</h4>
            ${streams.length ? `<ul data-subtitle-list>${streams.map(subtitleRowHtml).join('')}</ul>`
                : `<p class="text-sm text-white/50">${escapeHtml(t('subtitles.none'))}</p>`}
        </section>
        <section class="pt-3 border-t border-white/10">
            <h4 class="${LABEL}">${escapeHtml(t('subtitles.search'))}</h4>
            <p class="text-xs text-white/50" data-search-note>${escapeHtml(t('subtitles.no_provider'))}</p>
        </section>
        <form class="space-y-3 pt-3 border-t border-white/10" data-upload-form>
            <h4 class="${LABEL}">${escapeHtml(t('subtitles.upload'))}</h4>
            <div><label class="${LABEL}" for="sub-up-file">${escapeHtml(t('subtitles.file'))}</label>
                <input id="sub-up-file" name="file" type="file" accept=".srt,.vtt,.ass,.ssa" required class="jf-input w-full"></div>
            <div><label class="${LABEL}" for="sub-up-language">${escapeHtml(t('subtitles.language'))}</label>
                ${languages ? `<select id="sub-up-language" name="language" required class="jf-input w-full"><option value=""></option>${languages}</select>`
                    : `<input id="sub-up-language" name="language" required class="jf-input w-full" placeholder="eng">`}</div>
            <label class="flex items-center gap-3 text-sm text-white/70"><input type="checkbox" name="forced" class="${CHECKBOX}"> ${escapeHtml(t('subtitles.forced'))}</label>
            <label class="flex items-center gap-3 text-sm text-white/70"><input type="checkbox" name="hearingImpaired" class="${CHECKBOX}"> ${escapeHtml(t('subtitles.hearing_impaired'))}</label>
            <div class="flex justify-end gap-2 pt-3 border-t border-white/10">
                <button type="button" data-close class="jf-btn-ghost text-xs">${escapeHtml(t('common.close'))}</button>
                <button type="submit" class="jf-btn-primary text-xs">${icon('Upload', { size: 14 })} ${escapeHtml(t('subtitles.upload'))}</button>
            </div>
        </form>
    </div>`);

    dlg.querySelector('[data-subtitle-list]')?.addEventListener('click', async e => {
        const button = e.target.closest('[data-delete-subtitle]');
        if (!button || !confirm(t('subtitles.delete_confirm'))) return;
        try {
            await API.deleteSubtitle(id, button.dataset.deleteSubtitle);
            toast(t('subtitles.deleted'));
            await itemChanged(id);
            await openSubtitleEditor(id);
        } catch (err) {
            toast(t('subtitles.delete_failed', { message: err.message }), 'error');
        }
    });

    const form = dlg.querySelector('[data-upload-form]');
    form.addEventListener('submit', async e => {
        e.preventDefault();
        const file = form.file.files?.[0];
        const format = subtitleFormat(file?.name);
        if (!file || !format) {
            toast(t('subtitles.bad_format'), 'error');
            return;
        }
        if (file.size > UPLOAD_MAX_BYTES) {
            toast(t('subtitles.too_large'), 'error');
            return;
        }
        try {
            const data = await fileBase64(file);
            await API.uploadSubtitle(id, uploadBody({
                language: form.language.value, format, forced: form.forced.checked,
                hearingImpaired: form.hearingImpaired.checked, data,
            }));
            toast(t('subtitles.uploaded'));
            await itemChanged(id);
            await openSubtitleEditor(id);
        } catch (err) {
            toast(t('subtitles.upload_failed', { message: err.message }), 'error');
        }
    });
}

export { openSubtitleEditor, subtitleFormat, uploadBody, subtitleStreams, subtitleRowHtml };
