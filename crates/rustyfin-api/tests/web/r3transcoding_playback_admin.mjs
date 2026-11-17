// R3 transcoding: the dashboard's Playback tab shows the stored encoding options and the server
// configuration's resume and bitrate settings, and reads the form back into the whole objects it
// loaded, keeping the fields it doesn't show.
// Runs playback-admin.js in a vm sandbox with its imports stubbed.
// Usage: node r3transcoding_playback_admin.mjs (run by api_tests.rs r3transcoding_web_playback_admin).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const src = new URL('../../../rustyfin-web/src/', import.meta.url);
const module = fs.readFileSync(new URL('modules/playback-admin.js', src), 'utf8')
    .replace(/^import .*$/gm, '')
    .replace(/^export \{[\s\S]*?\};$/m, '');
const en = JSON.parse(fs.readFileSync(new URL('translations/en.json', src), 'utf8'));

function sandbox() {
    const context = vm.createContext({
        t: (key, vars = {}) => {
            const [section, name] = key.split('.');
            const text = en[section]?.[name];
            assert.ok(text !== undefined, `en.json has ${key}`);
            return text.replace(/\{\{(\w+)\}\}/g, (_, k) => vars[k] ?? '');
        },
        escapeHtml: s => String(s).replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;'),
        $: () => null,
        show: () => {},
        hide: () => {},
        toast: () => {},
    });
    vm.runInContext(`${module}\nthis.api = { playbackSettingsHtml, readPlaybackSettings, playbackSettingsProblem, postPlaybackSettings };`, context);
    return context.api;
}

const unescape = s => s.replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');

// A form object over the rendered markup: `elements[name]` with value/checked, and the checked
// codec boxes for querySelectorAll.
function formFrom(html) {
    const elements = {};
    const codecs = [];
    for (const m of html.matchAll(/<input ([^>]*)>/g)) {
        const attrs = m[1];
        const name = attrs.match(/name="([^"]*)"/)?.[1];
        if (!name) continue;
        const value = unescape(attrs.match(/value="([^"]*)"/)?.[1] ?? '');
        const checked = / checked\b/.test(` ${attrs}`);
        if (name === 'HardwareDecodingCodecs') codecs.push({ value, checked });
        else elements[name] = { value, checked };
    }
    for (const m of html.matchAll(/<select [^>]*name="([^"]*)"[^>]*>([\s\S]*?)<\/select>/g)) {
        const selected = m[2].match(/<option value="([^"]*)" selected>/)?.[1];
        const first = m[2].match(/<option value="([^"]*)"/)?.[1];
        elements[m[1]] = { value: unescape(selected ?? first) };
    }
    return {
        elements,
        codecs,
        querySelectorAll: sel => {
            assert.equal(sel, 'input[name="HardwareDecodingCodecs"]:checked');
            return codecs.filter(c => c.checked);
        },
    };
}

const encoding = {
    EncodingThreadCount: -1, TranscodingTempPath: '/data/transcodes', DownMixAudioBoost: 2,
    DownMixStereoAlgorithm: 'None', MaxMuxingQueueSize: 2048, EnableThrottling: false,
    ThrottleDelaySeconds: 180, EnableSegmentDeletion: false, SegmentKeepSeconds: 720,
    HardwareAccelerationType: 'vaapi', VaapiDevice: '/dev/dri/renderD128', QsvDevice: '',
    EnableTonemapping: false, TonemappingAlgorithm: 'bt2390', TonemappingRange: 'auto',
    TonemappingDesat: 0, TonemappingPeak: 100, TonemappingParam: 0, H264Crf: 23, H265Crf: 28,
    EncoderPreset: 'auto', EnableDecodingColorDepth10Hevc: true, EnableDecodingColorDepth10Vp9: true,
    EnableHardwareEncoding: true, AllowHevcEncoding: false, AllowAv1Encoding: false,
    HardwareDecodingCodecs: ['h264', 'vc1'], EncoderAppPath: 'ffmpeg', DeinterlaceMethod: 'yadif',
    R3FutureOption: { Nested: [1, 'two'] },
};
const config = {
    ServerName: 'R3 <Server>', MinResumePct: 5, MaxResumePct: 90, MinResumeDurationSeconds: 300,
    RemoteClientBitrateLimit: 8000000, PluginRepositories: [],
};

test('the form shows the stored options', () => {
    const { playbackSettingsHtml } = sandbox();
    const html = playbackSettingsHtml(encoding, config);
    const form = formFrom(html);
    assert.equal(form.elements.HardwareAccelerationType.value, 'vaapi');
    assert.equal(form.elements.VaapiDevice.value, '/dev/dri/renderD128');
    assert.deepEqual(form.codecs.filter(c => c.checked).map(c => c.value), ['h264', 'vc1']);
    assert.equal(form.codecs.length, 8);
    assert.equal(form.elements.EnableHardwareEncoding.checked, true);
    assert.equal(form.elements.AllowHevcEncoding.checked, false);
    assert.equal(form.elements.H264Crf.value, '23');
    assert.equal(form.elements.EncoderPreset.value, 'auto');
    assert.equal(form.elements.TonemappingAlgorithm.value, 'bt2390');
    assert.equal(form.elements.MinResumePct.value, '5');
    assert.equal(form.elements.RemoteClientBitrateLimit.value, '8', 'shown in Mbps');
    // The types Rustyfin stores only say so; the implemented ones don't
    assert.match(html, /<option value="videotoolbox">Apple VideoToolbox: saved only, transcodes in software<\/option>/);
    assert.match(html, /<option value="nvenc">Nvidia NVENC<\/option>/);
    assert.match(html, /data-action="playback-save"/);
});

test('an unchanged form reads back the objects it was given', () => {
    const { playbackSettingsHtml, readPlaybackSettings } = sandbox();
    const form = formFrom(playbackSettingsHtml(encoding, config));
    const read = readPlaybackSettings(form, encoding, config);
    assert.deepEqual(JSON.parse(JSON.stringify(read.encoding)), encoding);
    assert.deepEqual(JSON.parse(JSON.stringify(read.config)), config);
});

test('edits land in the encoding options and the server configuration', () => {
    const { playbackSettingsHtml, readPlaybackSettings, playbackSettingsProblem } = sandbox();
    const form = formFrom(playbackSettingsHtml(encoding, config));
    form.elements.HardwareAccelerationType.value = 'qsv';
    form.elements.QsvDevice.value = ' /dev/dri/renderD129 ';
    form.codecs.find(c => c.value === 'hevc').checked = true;
    form.elements.EnableHardwareEncoding.checked = false;
    form.elements.AllowHevcEncoding.checked = true;
    form.elements.EncodingThreadCount.value = '4';
    form.elements.H264Crf.value = '19';
    form.elements.EncoderPreset.value = 'slow';
    form.elements.DownMixAudioBoost.value = '1.5';
    form.elements.DownMixStereoAlgorithm.value = 'Dave750';
    form.elements.EnableTonemapping.checked = true;
    form.elements.TonemappingAlgorithm.value = 'hable';
    form.elements.MaxMuxingQueueSize.value = '';
    form.elements.MinResumePct.value = '10';
    form.elements.RemoteClientBitrateLimit.value = '2.5';
    const read = readPlaybackSettings(form, encoding, config);
    const e = read.encoding;
    assert.equal(e.HardwareAccelerationType, 'qsv');
    assert.equal(e.QsvDevice, '/dev/dri/renderD129');
    assert.deepEqual([...e.HardwareDecodingCodecs], ['h264', 'hevc', 'vc1']);
    assert.equal(e.EnableHardwareEncoding, false);
    assert.equal(e.AllowHevcEncoding, true);
    assert.equal(e.EncodingThreadCount, 4);
    assert.equal(e.H264Crf, 19);
    assert.equal(e.EncoderPreset, 'slow');
    assert.equal(e.DownMixAudioBoost, 1.5);
    assert.equal(e.DownMixStereoAlgorithm, 'Dave750');
    assert.equal(e.EnableTonemapping, true);
    assert.equal(e.TonemappingAlgorithm, 'hable');
    assert.equal(e.MaxMuxingQueueSize, 2048, 'an empty number keeps the stored value');
    assert.deepEqual(JSON.parse(JSON.stringify(e.R3FutureOption)), { Nested: [1, 'two'] });
    assert.equal(read.config.MinResumePct, 10);
    assert.equal(read.config.RemoteClientBitrateLimit, 2500000);
    assert.equal(read.config.ServerName, 'R3 <Server>');
    assert.equal(playbackSettingsProblem(read), null);
    // The original objects are not changed
    assert.equal(encoding.H264Crf, 23);
    assert.equal(config.MinResumePct, 5);
});

test('a failed save says which part failed', async () => {
    const { postPlaybackSettings } = sandbox();
    const values = { encoding: { H264Crf: 20 }, config: { MinResumePct: 5 } };
    const api = (encodingError, configError) => {
        const posted = [];
        return {
            posted,
            post: async (path, body) => { if (encodingError) throw new Error(encodingError); posted.push([path, body]); },
            updateConfig: async body => { if (configError) throw new Error(configError); posted.push(['config', body]); },
        };
    };
    const ok = api();
    assert.equal(await postPlaybackSettings(values, ok), null);
    assert.deepEqual(ok.posted.map(([p]) => p), ['/System/Configuration/encoding', 'config']);
    // The encoding options refused: nothing saved, and the configuration isn't posted
    const refused = api('encoding: H264Crf: 60 is outside 0 to 51');
    const first = await postPlaybackSettings(values, refused);
    assert.equal(first.saved, null);
    assert.match(first.message, /^Nothing was saved: the transcoding settings were refused \(encoding: H264Crf/);
    assert.equal(refused.posted.length, 0);
    // The configuration refused after the options were saved: the message says both
    const partial = await postPlaybackSettings(values, api(null, 'HTTP 500'));
    assert.equal(partial.saved, 'encoding');
    assert.equal(partial.message, 'The transcoding settings were saved, but the resume and streaming settings were not (HTTP 500).');
});

test('out-of-range values are refused before a save', () => {
    const { playbackSettingsProblem } = sandbox();
    assert.equal(playbackSettingsProblem({ encoding: { ...encoding, H265Crf: 60 }, config }), 'playback_admin.error_crf');
    assert.equal(
        playbackSettingsProblem({ encoding, config: { ...config, MinResumePct: 95 } }),
        'playback_admin.error_resume',
    );
});

// Hardware detection: "Automatic" leads the type list, the detected device is described, and
// saving with "Automatic" sends no type, so the server keeps using the detected device.
const hw = {
    Automatic: true,
    Detecting: false,
    Detection: {
        Devices: [
            { Type: 'vaapi', Device: '/dev/dri/renderD128', Usable: true, Error: null, HardwareDecodingCodecs: ['h264', 'hevc', 'vp9', 'av1'], EnableDecodingColorDepth10Hevc: true, EnableDecodingColorDepth10Vp9: false },
            { Type: 'vaapi', Device: '/dev/dri/renderD129', Usable: false, Error: 'permission denied', HardwareDecodingCodecs: [], EnableDecodingColorDepth10Hevc: false, EnableDecodingColorDepth10Vp9: false },
        ],
        DetectedAt: '2026-11-16T00:00:00Z',
    },
};
hw.Detection.Selected = hw.Detection.Devices[0];

test('r3hwaccel: the detected device and the Automatic choice', () => {
    const { playbackSettingsHtml, readPlaybackSettings } = sandbox();
    const html = playbackSettingsHtml(encoding, config, hw);
    assert.ok(html.includes('Detected: VA-API, /dev/dri/renderD128, decodes H.264, HEVC, HEVC 10-bit, VP9, AV1'), html);
    assert.ok(html.includes('VA-API /dev/dri/renderD129 is not usable: permission denied'), html);
    assert.ok(html.includes('data-action="playback-detect-hw"'));
    assert.ok(html.includes('>Automatic (VA-API)</option>'));
    const form = formFrom(html);
    assert.equal(form.elements.HardwareAccelerationType.value, 'auto');
    const { encoding: saved } = readPlaybackSettings(form, encoding, config);
    assert.equal(saved.HardwareAccelerationType, null);

    // A chosen type is shown and saved as before
    const chosen = formFrom(playbackSettingsHtml(encoding, config, { ...hw, Automatic: false }));
    assert.equal(chosen.elements.HardwareAccelerationType.value, 'vaapi');
    assert.equal(readPlaybackSettings(chosen, encoding, config).encoding.HardwareAccelerationType, 'vaapi');

    // Nothing found
    const none = playbackSettingsHtml(encoding, config, { Automatic: true, Detecting: false, Detection: { Devices: [], Selected: null } });
    assert.ok(none.includes('No usable video hardware was found'), none);
    assert.ok(none.includes('>Automatic (no hardware found)</option>'));
    // Without the detection (an older server), the list is Jellyfin's
    assert.ok(!playbackSettingsHtml(encoding, config).includes('Automatic ('));
});
