// The admin dashboard's Playback tab: Jellyfin's transcoding options (the named configuration
// `encoding`) and, from the server configuration, the resume thresholds and the remote streaming
// bitrate limit. Each save posts back the whole object the tab loaded, as Jellyfin Web does, so
// the fields the tab doesn't show are kept.
import { $, show, hide, toast, escapeHtml } from './util.js';

// Jellyfin's HardwareAccelerationType values. Rustyfin builds ffmpeg arguments for VAAPI, QSV and
// NVENC; the others are saved and transcode in software.
const HW_TYPES = [
    ['none', null],
    ['vaapi', 'Video Acceleration API (VAAPI)'],
    ['qsv', 'Intel Quick Sync (QSV)'],
    ['nvenc', 'Nvidia NVENC'],
    ['amf', 'AMD AMF'],
    ['videotoolbox', 'Apple VideoToolbox'],
    ['rkmpp', 'Rockchip MPP (RKMPP)'],
    ['v4l2m2m', 'Video4Linux2 (V4L2)'],
];
const HW_IMPLEMENTED = ['none', 'vaapi', 'qsv', 'nvenc'];
// Short names for the detected device line
const HW_SHORT = { vaapi: 'VA-API', qsv: 'Intel QSV', nvenc: 'NVIDIA NVENC', videotoolbox: 'VideoToolbox' };
const DECODE_CODECS = [
    ['h264', 'H.264'], ['hevc', 'HEVC'], ['mpeg2video', 'MPEG-2'], ['mpeg4', 'MPEG-4'],
    ['vc1', 'VC-1'], ['vp8', 'VP8'], ['vp9', 'VP9'], ['av1', 'AV1'],
];
const PRESETS = ['auto', 'veryslow', 'slower', 'slow', 'medium', 'fast', 'faster', 'veryfast', 'superfast', 'ultrafast'];
const DOWNMIX = ['None', 'Dave750', 'NightmodeDialogue', 'Rfc7845', 'Ac4'];
const TONEMAP_ALGORITHMS = ['none', 'clip', 'linear', 'gamma', 'reinhard', 'hable', 'mobius', 'bt2390'];
const TONEMAP_RANGES = ['auto', 'tv', 'pc'];

const LABEL = 'block text-xs font-semibold text-white/70 mb-1.5';
const CHECKBOX = 'w-4 h-4 rounded bg-white/10 border-white/20 text-[#00a4dc]';

// What the tab loaded: { encoding, config }, posted back with the form's values on save
let loaded = null;
// The server's hardware detection (/System/MediaEncoder/HardwareAcceleration), null when unknown
let hardware = null;

function playbackPanelHtml() {
    return '<div id="admin-playback" class="hidden jf-settings-card"></div>';
}

function same(a, b) {
    return String(a ?? '').toLowerCase() === String(b ?? '').toLowerCase();
}

function options(values, selected, label = v => v) {
    return values.map(v => `<option value="${escapeHtml(v)}"${same(v, selected) ? ' selected' : ''}>${escapeHtml(label(v))}</option>`).join('');
}

function autoLabel(hw) {
    const selected = hw?.Detection?.Selected;
    return selected ? t('playback_admin.hw_auto', { name: HW_SHORT[selected.Type] || selected.Type }) : t('playback_admin.hw_auto_none');
}

function hwLabel(type, name) {
    if (!name) return t('playback_admin.hw_none');
    return HW_IMPLEMENTED.includes(type) ? name : t('playback_admin.hw_stored_only', { name });
}

// "VA-API, /dev/dri/renderD128, decodes H.264, HEVC, HEVC 10-bit" for a detected device
function hwSummary(d) {
    const names = Object.fromEntries(DECODE_CODECS);
    const codecs = [];
    for (const c of d.HardwareDecodingCodecs || []) {
        codecs.push(names[c] || c);
        if (c === 'hevc' && d.EnableDecodingColorDepth10Hevc) codecs.push(t('playback_admin.hw_10bit', { codec: 'HEVC' }));
        if (c === 'vp9' && d.EnableDecodingColorDepth10Vp9) codecs.push(t('playback_admin.hw_10bit', { codec: 'VP9' }));
    }
    const parts = [HW_SHORT[d.Type] || d.Type];
    if (d.Device) parts.push(d.Device);
    parts.push(t('playback_admin.hw_decodes', { codecs: codecs.join(', ') }));
    return parts.join(', ');
}

// What the hardware detection found, as lines of text
function hwDetectedLines(hw) {
    const detection = hw?.Detection;
    if (!detection) return hw?.Detecting ? [t('playback_admin.hw_detecting')] : [];
    const lines = detection.Selected
        ? [t('playback_admin.hw_detected', { summary: hwSummary(detection.Selected) })]
        : [t('playback_admin.hw_none_found')];
    for (const d of detection.Devices || []) {
        if (!d.Usable) lines.push(t('playback_admin.hw_unusable', { device: [HW_SHORT[d.Type] || d.Type, d.Device].filter(Boolean).join(' '), error: d.Error || '' }));
    }
    return lines;
}

// The tab's form for the stored encoding options and server configuration. `hw` is the server's
// hardware detection: with it, the type list starts with "Automatic" (the detected device)
function playbackSettingsHtml(enc, cfg, hw = null) {
    const field = (name, key, input, hintKey) => `
        <div>
            <label for="pb-${name}" class="${LABEL}">${escapeHtml(t(key))}</label>
            ${input}
            ${hintKey ? `<p class="text-xs text-white/40 mt-1">${escapeHtml(t(hintKey))}</p>` : ''}
        </div>`;
    const number = (name, key, value, attrs = '', hintKey) => field(name, key,
        `<input id="pb-${name}" name="${name}" type="number" class="jf-input w-40" value="${escapeHtml(value ?? '')}" ${attrs}>`, hintKey);
    const text = (name, key, value, hintKey) => field(name, key,
        `<input id="pb-${name}" name="${name}" type="text" class="jf-input w-full" value="${escapeHtml(value ?? '')}" spellcheck="false" autocomplete="off">`, hintKey);
    const select = (name, key, values, selected, label, hintKey) => field(name, key,
        `<select id="pb-${name}" name="${name}" class="jf-input w-full max-w-sm">${options(values, selected, label)}</select>`, hintKey);
    const check = (name, key, checked) => `
        <div class="flex items-center gap-3">
            <input type="checkbox" id="pb-${name}" name="${name}" class="${CHECKBOX}"${checked ? ' checked' : ''}>
            <label for="pb-${name}" class="text-sm text-white/70">${escapeHtml(t(key))}</label>
        </div>`;
    const heading = key => `<h4 class="font-semibold text-white text-sm pt-2">${escapeHtml(t(key))}</h4>`;
    const decoding = (enc.HardwareDecodingCodecs || []).map(c => String(c).toLowerCase());
    const hwNames = Object.fromEntries(HW_TYPES);
    const bitrate = cfg.RemoteClientBitrateLimit ? cfg.RemoteClientBitrateLimit / 1e6 : '';

    return `
    <form id="playback-settings" class="space-y-4 max-w-2xl" onsubmit="return false">
        <h3 class="font-bold text-white text-base">${escapeHtml(t('playback_admin.transcoding'))}</h3>

        ${heading('playback_admin.hardware')}
        ${select('HardwareAccelerationType', 'playback_admin.hw_type', (hw ? ['auto'] : []).concat(HW_TYPES.map(([v]) => v)),
            hw?.Automatic ? 'auto' : (enc.HardwareAccelerationType || 'none'),
            v => (v === 'auto' ? autoLabel(hw) : hwLabel(v, hwNames[v])), hw ? 'playback_admin.hw_auto_hint' : 'playback_admin.hw_type_hint')}
        ${hw ? `<div class="text-xs text-white/60 space-y-1" id="pb-hw-detected">
            ${hwDetectedLines(hw).map(line => `<p>${escapeHtml(line)}</p>`).join('')}
            <button type="button" class="jf-btn-ghost text-xs" data-action="playback-detect-hw">${escapeHtml(t('playback_admin.hw_detect_again'))}</button>
        </div>` : ''}
        ${text('VaapiDevice', 'playback_admin.vaapi_device', enc.VaapiDevice, 'playback_admin.vaapi_device_hint')}
        ${text('QsvDevice', 'playback_admin.qsv_device', enc.QsvDevice, 'playback_admin.qsv_device_hint')}
        <fieldset>
            <legend class="${LABEL}">${escapeHtml(t('playback_admin.decoding_codecs'))}</legend>
            <div class="grid grid-cols-2 sm:grid-cols-4 gap-2">
                ${DECODE_CODECS.map(([codec, name]) => `
                <label class="flex items-center gap-2 text-sm text-white/70">
                    <input type="checkbox" name="HardwareDecodingCodecs" value="${codec}" class="${CHECKBOX}"${decoding.includes(codec) ? ' checked' : ''}>
                    ${escapeHtml(name)}
                </label>`).join('')}
            </div>
        </fieldset>
        ${check('EnableDecodingColorDepth10Hevc', 'playback_admin.decode_10bit_hevc', enc.EnableDecodingColorDepth10Hevc)}
        ${check('EnableDecodingColorDepth10Vp9', 'playback_admin.decode_10bit_vp9', enc.EnableDecodingColorDepth10Vp9)}
        ${check('EnableHardwareEncoding', 'playback_admin.hardware_encoding', enc.EnableHardwareEncoding)}

        ${heading('playback_admin.encoding')}
        ${check('AllowHevcEncoding', 'playback_admin.allow_hevc', enc.AllowHevcEncoding)}
        ${check('AllowAv1Encoding', 'playback_admin.allow_av1', enc.AllowAv1Encoding)}
        ${number('EncodingThreadCount', 'playback_admin.threads', enc.EncodingThreadCount, 'min="-1" step="1"', 'playback_admin.threads_hint')}
        ${select('EncoderPreset', 'playback_admin.preset', PRESETS, enc.EncoderPreset || 'auto',
            v => (v === 'auto' ? t('playback_admin.auto') : v))}
        ${number('H264Crf', 'playback_admin.h264_crf', enc.H264Crf, 'min="0" max="51" step="1"')}
        ${number('H265Crf', 'playback_admin.h265_crf', enc.H265Crf, 'min="0" max="51" step="1"', 'playback_admin.crf_hint')}
        ${number('MaxMuxingQueueSize', 'playback_admin.muxing_queue', enc.MaxMuxingQueueSize, 'min="128" step="1"')}
        ${text('TranscodingTempPath', 'playback_admin.temp_path', enc.TranscodingTempPath, 'playback_admin.temp_path_hint')}

        ${heading('playback_admin.audio')}
        ${number('DownMixAudioBoost', 'playback_admin.downmix_boost', enc.DownMixAudioBoost, 'min="0.5" max="3" step="0.1"')}
        ${select('DownMixStereoAlgorithm', 'playback_admin.downmix_algorithm', DOWNMIX, enc.DownMixStereoAlgorithm || 'None',
            v => (v === 'None' ? t('playback_admin.downmix_none') : v), 'playback_admin.downmix_hint')}

        ${heading('playback_admin.tonemapping')}
        ${check('EnableTonemapping', 'playback_admin.enable_tonemapping', enc.EnableTonemapping)}
        <p class="text-xs text-white/40">${escapeHtml(t('playback_admin.tonemapping_hint'))}</p>
        ${select('TonemappingAlgorithm', 'playback_admin.tonemap_algorithm', TONEMAP_ALGORITHMS, enc.TonemappingAlgorithm || 'bt2390')}
        ${select('TonemappingRange', 'playback_admin.tonemap_range', TONEMAP_RANGES, enc.TonemappingRange || 'auto',
            v => (v === 'auto' ? t('playback_admin.auto') : v.toUpperCase()))}
        ${number('TonemappingDesat', 'playback_admin.tonemap_desat', enc.TonemappingDesat, 'min="0" step="0.1"')}
        ${number('TonemappingPeak', 'playback_admin.tonemap_peak', enc.TonemappingPeak, 'min="0" step="1"')}
        ${number('TonemappingParam', 'playback_admin.tonemap_param', enc.TonemappingParam, 'step="0.01"')}

        ${heading('playback_admin.throttling')}
        <p class="text-xs text-amber-300/80">${escapeHtml(t('playback_admin.throttling_hint'))}</p>
        ${check('EnableThrottling', 'playback_admin.enable_throttling', enc.EnableThrottling)}
        ${number('ThrottleDelaySeconds', 'playback_admin.throttle_delay', enc.ThrottleDelaySeconds, 'min="60" step="1"')}
        ${check('EnableSegmentDeletion', 'playback_admin.enable_segment_deletion', enc.EnableSegmentDeletion)}
        ${number('SegmentKeepSeconds', 'playback_admin.segment_keep', enc.SegmentKeepSeconds, 'min="120" step="1"')}

        <h3 class="font-bold text-white text-base pt-4">${escapeHtml(t('playback_admin.resume'))}</h3>
        <p class="text-xs text-amber-300/80">${escapeHtml(t('playback_admin.resume_hint'))}</p>
        ${number('MinResumePct', 'playback_admin.min_resume_pct', cfg.MinResumePct, 'min="0" max="100" step="1"')}
        ${number('MaxResumePct', 'playback_admin.max_resume_pct', cfg.MaxResumePct, 'min="1" max="100" step="1"')}
        ${number('MinResumeDurationSeconds', 'playback_admin.min_resume_duration', cfg.MinResumeDurationSeconds, 'min="0" step="1"')}

        <h3 class="font-bold text-white text-base pt-4">${escapeHtml(t('playback_admin.streaming'))}</h3>
        ${number('RemoteClientBitrateLimit', 'playback_admin.remote_bitrate', bitrate, 'min="0" step="0.1"', 'playback_admin.remote_bitrate_hint')}

        <div id="playback-settings-error" class="hidden text-red-400 text-xs p-2 rounded bg-red-500/10 border border-red-500/20" role="alert"></div>
        <div class="flex justify-end">
            <button type="button" class="jf-btn-primary text-xs" data-action="playback-save">${escapeHtml(t('playback_admin.save'))}</button>
        </div>
    </form>`;
}

// The stored objects with the form's values. A number field left empty or unreadable keeps the
// stored value; the bitrate is typed in Mbps and stored in bits per second.
function readPlaybackSettings(form, enc, cfg) {
    const el = name => form.elements[name];
    const int = (name, stored) => {
        const v = parseInt(el(name).value, 10);
        return Number.isFinite(v) ? v : stored;
    };
    const num = (name, stored) => {
        const v = parseFloat(el(name).value);
        return Number.isFinite(v) ? v : stored;
    };
    const encoding = { ...enc };
    for (const name of ['HardwareAccelerationType', 'EncoderPreset', 'DownMixStereoAlgorithm', 'TonemappingAlgorithm', 'TonemappingRange']) {
        encoding[name] = el(name).value;
    }
    // Automatic: no type saved, so the server keeps using the detected device
    if (encoding.HardwareAccelerationType === 'auto') encoding.HardwareAccelerationType = null;
    for (const name of ['VaapiDevice', 'QsvDevice', 'TranscodingTempPath']) {
        encoding[name] = el(name).value.trim();
    }
    for (const name of [
        'EnableDecodingColorDepth10Hevc', 'EnableDecodingColorDepth10Vp9', 'EnableHardwareEncoding', 'AllowHevcEncoding',
        'AllowAv1Encoding', 'EnableTonemapping', 'EnableThrottling', 'EnableSegmentDeletion',
    ]) {
        encoding[name] = el(name).checked;
    }
    encoding.HardwareDecodingCodecs = Array.from(form.querySelectorAll('input[name="HardwareDecodingCodecs"]:checked'), i => i.value);
    for (const name of ['EncodingThreadCount', 'H264Crf', 'H265Crf', 'MaxMuxingQueueSize', 'ThrottleDelaySeconds', 'SegmentKeepSeconds']) {
        encoding[name] = int(name, enc[name]);
    }
    for (const name of ['DownMixAudioBoost', 'TonemappingDesat', 'TonemappingPeak', 'TonemappingParam']) {
        encoding[name] = num(name, enc[name]);
    }
    const config = { ...cfg };
    for (const name of ['MinResumePct', 'MaxResumePct', 'MinResumeDurationSeconds']) {
        config[name] = int(name, cfg[name]);
    }
    config.RemoteClientBitrateLimit = Math.max(0, Math.round((parseFloat(el('RemoteClientBitrateLimit').value) || 0) * 1e6));
    return { encoding, config };
}

// What is wrong with the values, as a translation key; null when nothing is
function playbackSettingsProblem({ encoding, config }) {
    const crf = v => Number.isInteger(v) && v >= 0 && v <= 51;
    if (!crf(encoding.H264Crf) || !crf(encoding.H265Crf)) return 'playback_admin.error_crf';
    if (!(config.MinResumePct < config.MaxResumePct)) return 'playback_admin.error_resume';
    return null;
}

async function loadPlaybackAdmin() {
    const box = $('admin-playback');
    if (!box) return;
    try {
        const [encoding, config, hw] = await Promise.all([
            API.get('/System/Configuration/encoding'),
            API.getConfig(),
            API.get('/System/MediaEncoder/HardwareAcceleration').catch(() => null),
        ]);
        loaded = { encoding, config };
        hardware = hw;
    } catch (err) {
        box.innerHTML = `<p class="text-xs text-red-400">${escapeHtml(t('playback_admin.load_failed'))}: ${escapeHtml(err.message)}</p>`;
        return;
    }
    box.innerHTML = playbackSettingsHtml(loaded.encoding, loaded.config, hardware);
}

// Runs the hardware detection again (a few seconds), then shows the tab with its result
async function detectHardware(button) {
    if (button) button.disabled = true;
    try {
        await API.post('/System/MediaEncoder/HardwareAcceleration/Detect', {});
    } catch (err) {
        toast(err.message);
    }
    await loadPlaybackAdmin();
}

// The form stays as typed when a save fails: the error goes into the card
async function savePlaybackSettings() {
    const form = $('playback-settings');
    if (!form || !loaded) return;
    const values = readPlaybackSettings(form, loaded.encoding, loaded.config);
    hide('playback-settings-error');
    const problem = playbackSettingsProblem(values);
    const fail = message => {
        $('playback-settings-error').textContent = message;
        show('playback-settings-error');
    };
    if (problem) return fail(t(problem));
    const failure = await postPlaybackSettings(values, API);
    if (failure?.saved === 'encoding') loaded = { ...loaded, encoding: values.encoding };
    if (failure) return fail(failure.message);
    loaded = values;
    toast(t('playback_admin.saved'));
}

// Posts the encoding options, then the server configuration. Null when both are saved; else
// which part was saved before the failure ('encoding' or null) and a message naming the part
// that failed.
async function postPlaybackSettings(values, api) {
    try {
        await api.post('/System/Configuration/encoding', values.encoding);
    } catch (err) {
        return { saved: null, message: t('playback_admin.error_encoding', { error: err.message }) };
    }
    try {
        await api.updateConfig(values.config);
    } catch (err) {
        return { saved: 'encoding', message: t('playback_admin.error_config', { error: err.message }) };
    }
    return null;
}

export { playbackPanelHtml, loadPlaybackAdmin, savePlaybackSettings, detectHardware };
