// Rustyfin WebUI translations, using i18next
// One JSON file per language in translations/ (English is the source and the fallback).
// Keys are snake_case and grouped by area: t('wizard.next'). Messages take {{name}}
// placeholders; plurals are key_one / key_other, picked by a numeric `count` option.
// Static markup in index.html is translated through data-i18n attributes.
// app.js and api.js are classic scripts, so the helpers are exposed on window.
import i18next from 'i18next';
import LanguageDetector from 'i18next-browser-languagedetector';

const TRANSLATED_ATTRIBUTES = ['placeholder', 'title', 'alt', 'aria-label'];

const files = import.meta.glob('./translations/*.json', { eager: true, import: 'default' });
const resources = Object.fromEntries(Object.entries(files).map(([path, translation]) => [
    path.slice(path.lastIndexOf('/') + 1, -'.json'.length).replace(/_/g, '-'),
    { translation },
]));

i18next.use(LanguageDetector).init({
    resources,
    fallbackLng: 'en',
    supportedLngs: Object.keys(resources),
    // Keep the browser's region (en-GB) as the language, so dates and numbers follow it
    nonExplicitSupportedLngs: true,
    // Resources are bundled, so t() works as soon as this module has run
    initAsync: false,
    // Untranslated strings exported as "" fall back to English
    returnEmptyString: false,
    // Callers escape values that end up in HTML (escapeHtml), as before
    interpolation: { escapeValue: false },
    detection: {
        order: ['localStorage', 'navigator'],
        lookupLocalStorage: 'rustyfin_ui_lang',
        caches: ['localStorage'],
    },
    saveMissing: true,
    missingKeyHandler: (lngs, ns, key) => console.warn(`Missing translation key: ${key}`),
});

const t = (key, options) => i18next.t(key, options);

// <span data-i18n="nav.home">Home</span> gets its text translated,
// data-i18n-placeholder="key" (and -title, -alt, -aria-label) the attribute
function applyTranslations(root = document) {
    root.querySelectorAll('[data-i18n]').forEach(el => {
        el.textContent = t(el.dataset.i18n);
    });
    for (const attr of TRANSLATED_ATTRIBUTES) {
        root.querySelectorAll(`[data-i18n-${attr}]`).forEach(el => {
            el.setAttribute(attr, t(el.getAttribute(`data-i18n-${attr}`)));
        });
    }
    document.documentElement.lang = i18next.resolvedLanguage;
}

// The translation in use, one of the translations/ file names: 'de'
function getUiLanguage() {
    return i18next.resolvedLanguage;
}

// Switches and remembers the language; markup with data-i18n follows at once,
// text built by app.js only when it is rendered again
function setUiLanguage(lng) {
    return i18next.changeLanguage(lng).then(() => applyTranslations());
}

// BCP 47 tag for Intl formatting: the regional variant of the UI language
// from the browser (en-GB), else the language itself
function uiLocale() {
    const lng = i18next.resolvedLanguage;
    const prefix = lng.toLowerCase() + '-';
    return [i18next.language, ...navigator.languages].find(tag => tag.toLowerCase().startsWith(prefix)) || lng;
}

// Languages for a picker, each named in itself: [{ value: 'de', label: 'Deutsch' }, ...]
function uiLanguageOptions() {
    return Object.keys(resources).map(lng => {
        let label = lng;
        try {
            label = new Intl.DisplayNames([lng], { type: 'language' }).of(lng) || lng;
        } catch {}
        return { value: lng, label: label.charAt(0).toLocaleUpperCase(lng) + label.slice(1) };
    }).sort((a, b) => a.label.localeCompare(b.label));
}

Object.assign(window, { t, applyTranslations, getUiLanguage, setUiLanguage, uiLocale, uiLanguageOptions });

applyTranslations();
