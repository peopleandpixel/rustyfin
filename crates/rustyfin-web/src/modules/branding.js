// Jellyfin's branding: the administrator's custom CSS, applied on every page (the login page
// too, since /Branding/Css needs no sign-in), and the login disclaimer under the sign-in form.

const STYLE_ID = 'rf-branding-css';

// Fetches the custom CSS and puts it in a <style> at the end of <head>; a failure keeps the CSS
// the page has
async function applyBrandingCss() {
    let css;
    try {
        const res = await fetch(`${API.base}/Branding/Css`, { cache: 'no-store' });
        if (!res.ok) return;
        css = await res.text();
    } catch {
        return;
    }
    let style = document.getElementById(STYLE_ID);
    if (!style) {
        style = document.createElement('style');
        style.id = STYLE_ID;
        document.head.appendChild(style);
    }
    style.textContent = css;
}

// Shows the login disclaimer, as plain text, in the element `el`; hidden when there is none
async function showLoginDisclaimer(el) {
    if (!el) return;
    let text = '';
    try {
        const res = await fetch(`${API.base}/Branding/Configuration`, { cache: 'no-store' });
        if (res.ok) text = (await res.json()).LoginDisclaimer || '';
    } catch {
        // No disclaimer when the server can't be read
    }
    el.textContent = text;
    el.classList.toggle('hidden', !text.trim());
}

applyBrandingCss();

export { applyBrandingCss, showLoginDisclaimer };
