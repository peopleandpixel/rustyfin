// Sign-in (password, single sign-on, Quick Connect), sign-up of the owner and sign-out
import { $, html, show, hide, toast, escapeHtml } from './util.js';
import { currentPage, navigate, closeUserMenu } from './router.js';
import { renderSettings } from './settings.js';
import { loadUserSettings } from './user-settings.js';
import { jiChoice, jiConfirm, jiShownCodes } from './jellyfin-import.js';
import { loadSidebar, initScanProgressWebSocket, stopScanProgressWebSocket } from './library-admin.js';

// --- OpenID Connect: helpers for the sign-in page and the profile ---
// The redirect codes of the OIDC flows, each mapped to its translation key. A code not listed here gets the
// generic message; the code itself never reaches t().
const OIDC_ERROR_KEYS = new Map([
    ['cancelled', 'oidc.error.cancelled'],
    ['state_mismatch', 'oidc.error.state_mismatch'],
    ['expired', 'oidc.error.expired'],
    ['disabled', 'oidc.error.disabled'],
    ['provider_error', 'oidc.error.provider_error'],
    ['invalid_token', 'oidc.error.invalid_token'],
    ['no_username', 'oidc.error.no_username'],
    ['not_allowed', 'oidc.error.not_allowed'],
    ['name_taken', 'oidc.error.name_taken'],
    ['name_taken_ask_admin', 'oidc.error.name_taken_ask_admin'],
    ['already_linked', 'oidc.error.already_linked'],
    ['account_disabled', 'oidc.error.account_disabled'],
    ['server_error', 'oidc.error.server_error'],
]);

// The code of an 'oidc-error/<code>' route, or null for any other route.
function oidcErrorCode(data) {
    const match = /^oidc-error\/(.*)$/.exec(data || '');
    return match ? match[1] : null;
}

function oidcErrorText(code) {
    return t(OIDC_ERROR_KEYS.get(code) || 'oidc.error.unknown');
}

// Sends the browser to the provider. Only http(s) links are followed; anything else is an error.
function goToOidcProvider(url) {
    if (!/^https?:\/\//i.test(url || '')) throw new Error(t('oidc.error.provider_error'));
    window.location.href = url;
}

function showLoginError(message) {
    $('login-error').textContent = message;
    show('login-error');
}

// The address sign-ins must start from, from the last Info call (B156).
let oidcPublicUrl = '';

// Adds the provider button below the sign-in form when OIDC is on. A failed Info call counts as off.
async function loadOidcSignIn() {
    const info = await API.oidcInfo().catch(() => null);
    oidcPublicUrl = info?.PublicUrl || '';
    const box = $('oidc-sign-in');
    if (!info?.Enabled || !box) return;
    const label = (info.ButtonLabel || '').trim() || t('oidc.sign_in_default');
    box.innerHTML = `<button type="button" class="jf-btn-ghost w-full !py-3 !font-semibold text-sm mt-4" data-action="oidc-sign-in">${escapeHtml(label)}</button>`;
}

async function startOidcSignIn() {
    try {
        const { AuthorizationUrl } = await API.oidcStart();
        goToOidcProvider(AuthorizationUrl);
    } catch (err) {
        // 409: this page is open at another address than the one the provider sends users back to
        showLoginError(err.status === 409 && oidcPublicUrl ? t('oidc.error.wrong_address', { url: oidcPublicUrl }) : err.message);
    }
}

// Leaves a return from the provider for the home page when this page is signed in already (a session
// from before), so it never ends on the sign-in form inside the signed-in page. The entry is replaced,
// so Back does not return to the provider's answer.
function leaveOidcReturnSignedIn(errorMessage) {
    if (errorMessage) toast(errorMessage, 'error');
    history.replaceState(null, '', '#home');
    navigate('home');
}

// The flow is single-use, so the browser's return to #login/oidc is completed once per page load.
let oidcCompleteStarted = false;

async function completeOidcSignIn() {
    if (oidcCompleteStarted) return;
    oidcCompleteStarted = true;
    const completed = await API.oidcComplete().then(() => true, () => false);
    if (!completed) {
        if (API.isLoggedIn()) {
            leaveOidcReturnSignedIn(t('oidc.complete_failed'));
            return;
        }
        showLoginError(t('oidc.complete_failed'));
        history.replaceState(null, '', '#login');
        return;
    }
    history.replaceState(null, '', '#home');
    await finishSignIn();
}

// Starts linking the signed-in account. The provider's callback returns to #settings.
async function startOidcLink() {
    try {
        const { AuthorizationUrl } = await API.oidcLink();
        goToOidcProvider(AuthorizationUrl);
    } catch (err) {
        toast(err.message, 'error');
    }
}

async function unlinkOidc(userId) {
    if (!confirm(t('oidc.unlink_confirm'))) return;
    try {
        await API.oidcUnlink(userId);
        toast(t('oidc.unlinked'));
        renderSettings();
    } catch (err) {
        toast(err.message, 'error');
    }
}

// The profile gets the result of a link as #settings/oidc-linked or #settings/oidc-error/<code>. It is toasted
// once, then the hash is cleared so that a reload does not repeat it.
function showOidcLinkResult(data) {
    const code = oidcErrorCode(data);
    if (code !== null) toast(oidcErrorText(code), 'error');
    else if (data === 'oidc-linked') toast(t('oidc.linked'));
    else return;
    history.replaceState(null, '', '#settings');
}

// The profile's "Single sign-on" card. Not linked: a link button, when OIDC is on. Linked: the provider login
// and Unlink, which is disabled with the reason when the server says CanUnlink is false.
function oidcCardHtml(enabled, account) {
    let body;
    if (account?.Linked) {
        const linkedAt = account.LinkedAt ? new Date(account.LinkedAt).toLocaleString(uiLocale()) : '—';
        const canUnlink = Boolean(account.CanUnlink);
        const reason = account.HasPassword ? t('oidc.unlink_password_off') : t('oidc.unlink_needs_password');
        const linkedAs = t('oidc.linked_as', { name: account.ProviderUsername || '', date: linkedAt });
        body = `
            <p class="text-sm text-white/80">${escapeHtml(linkedAs)}</p>
            <div class="flex flex-wrap items-center gap-3 mt-4">
                <button type="button" class="jf-btn-ghost !py-2 !px-4 text-xs${canUnlink ? '' : ' !cursor-not-allowed !opacity-50 disabled:pointer-events-none'}" data-action="oidc-unlink" data-id="${escapeHtml(account.UserId)}"${canUnlink ? '' : ' disabled'}>${t('oidc.unlink')}</button>
                ${canUnlink ? '' : `<span class="text-xs text-white/40">${escapeHtml(reason)}</span>`}
            </div>`;
    } else {
        body = `
            <p class="text-sm text-white/60">${t('oidc.not_linked')}</p>
            ${enabled ? `<button type="button" class="jf-btn-primary !py-2 !px-4 text-xs mt-4" data-action="oidc-link">${t('oidc.link')}</button>` : ''}`;
    }
    return `
        <div class="jf-settings-card">
            <h3 class="text-base font-bold text-white mb-4 flex items-center gap-2">
                <span class="w-2 h-2 rounded-full bg-[#38bdf8]"></span>
                ${t('oidc.account_title')}
            </h3>
            ${body}
        </div>`;
}

// --- Login Page Renderer ---
let pendingSetup = null; // { username, code } of an imported user's one-time code; memory only
// The sign-in page's user tiles from /Users/Public, as Jellyfin Web's: the users not hidden from it
let loginUsers = [];
// Counts sign-in page renders: a user list for an earlier render is dropped
let loginSeq = 0;

// The card every sign-in step is drawn in: the logo, a title and a line under it, then `body`
function loginCard(title, subtitle, body, wide = false) {
    return `
    <div class="flex justify-center items-center min-h-[70vh] jf-page px-4">
        <div class="w-full ${wide ? 'max-w-3xl' : 'max-w-sm'} p-8 rounded-2xl bg-[#131822] border border-white/10 shadow-2xl">
            <div class="text-center mb-8">
                <img src="/web/assets/logo.png" alt="Rustyfin" class="w-20 h-20 mx-auto rounded-2xl shadow-xl mb-3 p-1 bg-white/5 border border-white/10">
                <h2 class="text-2xl font-black text-white tracking-tight">${escapeHtml(title)}</h2>
                <p class="text-white/40 text-xs mt-1">${escapeHtml(subtitle)}</p>
            </div>
            ${body}
        </div>
    </div>`;
}

const LINK_BUTTON = 'w-full text-xs text-white/50 hover:text-white transition py-2';

// One user tile: the profile picture or the initial, and the name. The router-free buttons are
// handled by the page's own click listener (loginClick), so no name reaches inline JavaScript.
function loginTileHtml(user) {
    const name = user.Name || '';
    const picture = user.PrimaryImageTag
        ? `<img alt="" class="w-full h-full object-cover" src="${escapeHtml(`${API.base}/UserImage?userId=${encodeURIComponent(user.Id)}&tag=${encodeURIComponent(user.PrimaryImageTag)}`)}">`
        : `<span class="text-3xl font-black text-white">${escapeHtml((name[0] || '?').toUpperCase())}</span>`;
    return `<button type="button" class="jf-login-tile" data-login="user" data-user-id="${escapeHtml(user.Id)}">
            <span class="jf-login-tile-picture">${picture}</span>
            <span class="block text-sm font-semibold text-white truncate mt-2">${escapeHtml(name)}</span>
        </button>`;
}

// The manual form: the name (filled in from a tile), the password, and the links under it
function loginFormHtml(name = '') {
    return `
            <form id="login-form" onsubmit="handleLogin(event)" class="space-y-4">
                <div>
                    <label for="login-user" class="block text-xs font-semibold text-white/70 mb-1.5">${t('common.username')}</label>
                    <input type="text" id="login-user" class="jf-input w-full" autocomplete="username" placeholder="${escapeHtml(t('common.username'))}" value="${escapeHtml(name)}" required>
                </div>
                <div>
                    <label for="login-pass" class="block text-xs font-semibold text-white/70 mb-1.5">${t('common.password')}</label>
                    <input type="password" id="login-pass" class="jf-input w-full" autocomplete="current-password" placeholder="${escapeHtml(t('common.password'))}">
                </div>
                <button type="submit" class="jf-btn-primary w-full !py-3 font-bold text-sm shadow-lg mt-2">
                    ${t('login.submit')}
                </button>
                <div id="login-error" class="text-red-400 text-xs text-center hidden p-2 rounded bg-red-500/10 border border-red-500/20"></div>
            </form>
            <div id="oidc-sign-in"></div>
            <div class="mt-4 flex flex-col">
                ${loginUsers.length ? `<button type="button" class="${LINK_BUTTON}" data-login="users">${escapeHtml(t('login.choose_user'))}</button>` : ''}
                <button type="button" class="${LINK_BUTTON}" data-login="forgot">${escapeHtml(t('login.forgot_password'))}</button>
            </div>`;
}

// The user tiles, with Manual login and Forgot password under them
function loginUsersHtml() {
    return `
            <div class="jf-login-tiles" role="list">${loginUsers.map(u => `<div role="listitem">${loginTileHtml(u)}</div>`).join('')}</div>
            <div id="login-error" class="text-red-400 text-xs text-center hidden p-2 rounded bg-red-500/10 border border-red-500/20 mt-4"></div>
            <div id="oidc-sign-in" class="max-w-sm mx-auto"></div>
            <div class="mt-6 flex flex-col max-w-sm mx-auto">
                <button type="button" class="jf-btn-ghost w-full !py-2.5 text-sm" data-login="manual">${escapeHtml(t('login.manual_login'))}</button>
                <button type="button" class="${LINK_BUTTON} mt-2" data-login="forgot">${escapeHtml(t('login.forgot_password'))}</button>
            </div>`;
}

// Draws the tiles when there are any, else the form; `name` fills the form's name and asks for the password
function showLoginStep(step, name = '') {
    const tiles = step === 'users' && loginUsers.length > 0;
    html('page-login', loginCard(
        tiles ? t('login.choose_user_title') : t('nav.sign_in'),
        t('login.subtitle'),
        tiles ? loginUsersHtml() : loginFormHtml(name),
        tiles,
    ));
    loadOidcSignIn();
    if (tiles) document.querySelector('#page-login [data-login="user"]')?.focus();
    else (name ? $('login-pass') : $('login-user'))?.focus();
}

function renderLogin(data) {
    pendingSetup = null;
    const seq = ++loginSeq;
    const oidcError = oidcErrorCode(data);
    if (oidcError !== null && API.isLoggedIn()) {
        leaveOidcReturnSignedIn(oidcErrorText(oidcError));
        return;
    }
    if (data === 'forgot') { renderForgotPassword(); return; }
    if (data === 'forgot-pin') { renderForgotPin(); return; }
    // The form first, so the page works at once; the tiles replace it when the list comes
    showLoginStep('manual');
    if (data === 'oidc') { completeOidcSignIn(); return; }
    if (oidcError !== null) { showLoginError(oidcErrorText(oidcError)); return; }
    API.publicUsers().then(users => {
        if (seq !== loginSeq || API.isLoggedIn()) return;
        loginUsers = Array.isArray(users) ? users : [];
        // Someone already typing keeps the form
        const typing = $('login-user')?.value || $('login-pass')?.value;
        if (loginUsers.length && !typing) showLoginStep('users');
    }).catch(() => { loginUsers = []; });
}

// A tile: a user without a password signs in at once, as in Jellyfin Web; the others get the form with
// their name filled in and the password field focused
async function signInFromTile(userId) {
    const user = loginUsers.find(u => u.Id === userId);
    if (!user) return;
    if (user.HasPassword === false) {
        try {
            await API.login(user.Name, '');
        } catch (err) {
            showLoginStep('manual', user.Name);
            showLoginError(err.status === 403 ? t('oidc.password_sign_in_off') : err.message);
            return;
        }
        await finishSignIn();
        return;
    }
    showLoginStep('manual', user.Name);
}

// The page's own buttons (data-login): the tiles, Manual login, back to the tiles and Forgot password
function loginClick(e) {
    const el = e.target.closest?.('#page-login [data-login]');
    if (!el) return;
    const step = el.dataset.login;
    if (step === 'user') signInFromTile(el.dataset.userId);
    else if (step === 'manual' || step === 'users') showLoginStep(step);
    else if (step === 'forgot') navigate('login', 'forgot');
    else if (step === 'back') navigate('login');
    else if (step === 'pin') navigate('login', 'forgot-pin');
}
document.addEventListener('click', loginClick);

// --- Forgot password: Jellyfin's two steps. The server writes a PIN file on its own disk, only for
// a request from the local network; the PIN, typed in the second step, becomes the new password ---
function forgotMessage(text, kind = 'info') {
    const box = $('forgot-message');
    if (!box) return;
    box.textContent = text;
    box.className = `text-xs text-center p-3 rounded border ${kind === 'error' ? 'text-red-400 bg-red-500/10 border-red-500/20' : 'text-white/80 bg-white/5 border-white/10'}`;
    box.hidden = false;
}

function renderForgotPassword() {
    html('page-login', loginCard(t('login.forgot_title'), t('login.forgot_text'), `
            <form id="forgot-form" class="space-y-4">
                <div>
                    <label for="forgot-user" class="block text-xs font-semibold text-white/70 mb-1.5">${t('common.username')}</label>
                    <input type="text" id="forgot-user" class="jf-input w-full" autocomplete="username" required>
                </div>
                <button type="submit" class="jf-btn-primary w-full !py-3 font-bold text-sm shadow-lg">${escapeHtml(t('login.forgot_submit'))}</button>
                <p id="forgot-message" role="status" hidden></p>
            </form>
            <div class="mt-4 flex flex-col">
                <button type="button" class="${LINK_BUTTON}" data-login="pin">${escapeHtml(t('login.have_pin'))}</button>
                <button type="button" class="${LINK_BUTTON}" data-login="back">${escapeHtml(t('login.back_to_sign_in'))}</button>
            </div>`));
    $('forgot-form').addEventListener('submit', submitForgotPassword);
    $('forgot-user').focus();
}

async function submitForgotPassword(e) {
    e.preventDefault();
    const button = e.target.querySelector('[type="submit"]');
    const name = $('forgot-user').value.trim();
    if (!name || button.disabled) return;
    button.disabled = true;
    try {
        const result = await API.forgotPassword(name);
        if (result.Action === 'PinCode') {
            const expires = result.PinExpirationDate ? new Date(result.PinExpirationDate).toLocaleString() : '';
            renderForgotPin(t('login.forgot_pin_written', { file: result.PinFile || '', expires }));
            return;
        }
        forgotMessage(t(result.Action === 'InNetworkRequired' ? 'login.forgot_in_network' : 'login.forgot_contact_admin'));
    } catch (err) {
        forgotMessage(err.status === 429 ? t('login.too_many_attempts') : err.message, 'error');
    } finally {
        button.disabled = false;
    }
}

// The PIN step; `note` says where the server wrote the PIN file
function renderForgotPin(note = '') {
    html('page-login', loginCard(t('login.pin_title'), t('login.pin_text'), `
            ${note ? `<p class="text-xs text-white/80 p-3 rounded bg-white/5 border border-white/10 mb-4 break-words">${escapeHtml(note)}</p>` : ''}
            <form id="pin-form" class="space-y-4">
                <div>
                    <label for="forgot-pin" class="block text-xs font-semibold text-white/70 mb-1.5">${escapeHtml(t('login.pin'))}</label>
                    <input type="text" id="forgot-pin" class="jf-input w-full font-mono" autocomplete="one-time-code" autocapitalize="characters" spellcheck="false" required>
                </div>
                <button type="submit" class="jf-btn-primary w-full !py-3 font-bold text-sm shadow-lg">${escapeHtml(t('login.pin_submit'))}</button>
                <p id="forgot-message" role="status" hidden></p>
            </form>
            <div class="mt-4 flex flex-col">
                <button type="button" class="${LINK_BUTTON}" data-login="back">${escapeHtml(t('login.back_to_sign_in'))}</button>
            </div>`));
    $('pin-form').addEventListener('submit', submitForgotPin);
    $('forgot-pin').focus();
}

async function submitForgotPin(e) {
    e.preventDefault();
    const button = e.target.querySelector('[type="submit"]');
    const input = $('forgot-pin');
    const pin = input.value.trim();
    if (!pin || button.disabled) return;
    button.disabled = true;
    try {
        const result = await API.redeemResetPin(pin);
        input.value = '';
        // Outside the local network the server tries no PIN, and answers Jellyfin's failed result
        if (result.Success === false) {
            forgotMessage(t('login.forgot_in_network'), 'error');
            return;
        }
        // The PIN is the new password now
        const names = (result.UsersReset || []).join(', ');
        html('page-login', loginCard(t('login.pin_done_title'), t('login.pin_done_text', { names }), `
            <button type="button" class="jf-btn-primary w-full !py-3 font-bold text-sm" data-login="back">${escapeHtml(t('login.back_to_sign_in'))}</button>`));
        document.querySelector('#page-login [data-login="back"]')?.focus();
    } catch (err) {
        const key = err.status === 429 ? 'login.too_many_attempts' : err.status === 403 ? 'oidc.password_sign_in_off' : 'login.pin_invalid';
        forgotMessage(t(key), 'error');
    } finally {
        button.disabled = false;
    }
}

// The steps after any sign-in: password, setup code or OpenID Connect.
async function finishSignIn() {
    updateAuthUI();
    // Replace a socket or retry left from an earlier session, so the new token is used now
    stopScanProgressWebSocket();
    initScanProgressWebSocket();
    await loadSidebar();
    navigate('home');
    toast(t('login.welcome_back', { name: API.userName }));
}

async function handleLogin(e) {
    e.preventDefault();
    const username = $('login-user').value;
    const password = $('login-pass').value;
    try {
        await API.login(username, password);
    } catch (err) {
        if (err.setupRequired) {
            renderSetupPassword(username, password);
            return;
        }
        showLoginError(err.status === 403 ? t('oidc.password_sign_in_off') : err.message);
        return;
    }
    await finishSignIn();
}

// Shown when a sign-in answers "setup required" for an imported user. The code is kept in memory only.
function renderSetupPassword(username, code) {
    pendingSetup = { username, code };
    html('page-login', `
    <div class="flex justify-center items-center min-h-[70vh] jf-page px-4">
        <div class="w-full max-w-sm p-8 rounded-2xl bg-[#131822] border border-white/10 shadow-2xl">
            <div class="text-center mb-8">
                <img src="/web/assets/logo.png" alt="Rustyfin" class="w-20 h-20 mx-auto rounded-2xl shadow-xl mb-3 p-1 bg-white/5 border border-white/10">
                <h2 class="text-2xl font-black text-white tracking-tight">${t('jellyfin_import.setup_title')}</h2>
                <p class="text-white/40 text-xs mt-1">${t('jellyfin_import.setup_text', { name: escapeHtml(username) })}</p>
            </div>
            <form id="setup-form" onsubmit="handleSetupPassword(event)" class="space-y-4">
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">${t('jellyfin_import.new_password')}</label>
                    <input type="password" id="setup-pass" class="jf-input w-full" autocomplete="new-password" autofocus>
                </div>
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">${t('jellyfin_import.confirm_password')}</label>
                    <input type="password" id="setup-pass2" class="jf-input w-full" autocomplete="new-password">
                </div>
                <button type="submit" class="jf-btn-primary w-full !py-3 font-bold text-sm shadow-lg mt-2">
                    ${t('jellyfin_import.setup_submit')}
                </button>
                <div id="setup-error" class="text-red-400 text-xs text-center hidden p-2 rounded bg-red-500/10 border border-red-500/20"></div>
                <button type="button" id="setup-back" class="w-full text-xs text-white/50 hover:text-white transition py-2" onclick="pendingSetup = null; renderLogin()">${t('jellyfin_import.back')}</button>
            </form>
        </div>
    </div>`);
}

async function handleSetupPassword(e) {
    e.preventDefault();
    const pw = $('setup-pass').value;
    const pw2 = $('setup-pass2').value;
    const fail = message => { $('setup-error').textContent = message; show('setup-error'); };
    if (!pw) return fail(t('jellyfin_import.password_required'));
    if (pw !== pw2) return fail(t('jellyfin_import.password_mismatch'));
    if (!pendingSetup) { renderLogin(); return; }
    try {
        await API.loginWithSetupCode(pendingSetup.username, pendingSetup.code, pw);
        pendingSetup = null;
        await finishSignIn();
    } catch (err) {
        fail(err.status === 401 ? t('jellyfin_import.code_invalid') : err.message);
    }
}

function doLogout() {
    API.logout();
    navigate('home');
    toast(t('login.signed_out'));
}

// The user menu's Switch user: signs out and opens the sign-in page with its user tiles
function switchUser() {
    closeUserMenu();
    API.logout();
    navigate('login');
}

// --- Quick Connect Modal Trigger ---
function showQuickConnectModal() {
    const dlg = $('qc-modal');
    if (!dlg) return;
    if (typeof dlg.showModal === 'function') dlg.showModal();
    else dlg.setAttribute('open', '');
}

async function authorizeQuickConnect() {
    const code = $('qc-code-input')?.value.trim();
    if (!code) {
        toast(t('quick_connect.enter_valid_code'), 'error');
        return;
    }
    try {
        await API.post('/QuickConnect/Authorize?code=' + encodeURIComponent(code));
        toast(t('quick_connect.authorized'));
        $('qc-modal')?.close();
    } catch (e) {
        toast(t('quick_connect.authorize_failed', { message: e.message }), 'error');
    }
}

// --- Auth UI Sync ---
function updateAuthUI() {
    if (API.isLoggedIn()) {
        hide('nav-login-btn');
        show('nav-user-menu');
        if ($('nav-avatar-letter')) $('nav-avatar-letter').textContent = (API.userName || 'U')[0].toUpperCase();
        if ($('nav-user-name')) $('nav-user-name').textContent = API.userName || '';
        if ($('dropdown-username')) $('dropdown-username').textContent = API.userName || '';
        // The user's settings (once per user): the profile picture, backdrops and date format
        loadUserSettings();
        show('nav-add-lib');
        if (API.isAdmin) {
            show('nav-admin-link');
            show('sidebar-add-lib');
            show('sidebar-admin-section');
        } else {
            hide('nav-admin-link');
            show('sidebar-add-lib');
            hide('sidebar-admin-section');
        }
    } else {
        show('nav-login-btn');
        hide('nav-user-menu');
        hide('nav-add-lib');
        hide('sidebar');
    }
}

// Reacts to lost administrator rights: hides the admin UI, says so, and leaves the Dashboard.
function followAdminLoss() {
    updateAuthUI();
    toast(t('admin.rights_removed'), 'error');
    if (currentPage === 'admin') navigate('home');
}
API.onAdminLost = followAdminLoss;

// Runs after every sign-out, the user's or a 401's: drops what the signed-out user left in this tab, such as the
// import page's setup codes and the server log, so the next user can't read them.
function afterSignOut() {
    stopScanProgressWebSocket();
    LiveLog.disconnect();
    LiveLog.messages = [];
    LiveLog.clearedSeq = 0;
    jiShownCodes.clear();
    jiPendingNames = new Map();
    jiChoice.clear();
    jiConfirm.clear();
    jiMine = '';
    $('page-admin').innerHTML = ''; // the import preview (#ji-users), its codes box and the live log
    libraries = [];
    updateAuthUI();
}
API.onSignedOut = afterSignOut;

// Another tab signed in or out: reload, so this tab drops the old session's token, cache and pages.
window.addEventListener('storage', e => {
    if (e.key === 'rustyfin_token') location.reload();
});

export {
    startOidcSignIn, startOidcLink, unlinkOidc, showOidcLinkResult, oidcCardHtml, renderLogin, handleLogin,
    handleSetupPassword, doLogout, switchUser, showQuickConnectModal, authorizeQuickConnect, updateAuthUI,
    signInFromTile, loginClick, submitForgotPassword, submitForgotPin,
};

// Shared state that inline handlers or other modules assign: reached through window
Object.defineProperties(window, {
    pendingSetup: { get: () => pendingSetup, set: value => { pendingSetup = value; } },
});
