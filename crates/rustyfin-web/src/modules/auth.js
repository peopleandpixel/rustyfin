// Sign-in (password, single sign-on, Quick Connect), sign-up of the owner and sign-out
import { $, html, show, hide, toast, escapeHtml } from './util.js';
import { currentPage, navigate } from './router.js';
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
function renderLogin(data) {
    pendingSetup = null;
    const oidcError = oidcErrorCode(data);
    if (oidcError !== null && API.isLoggedIn()) {
        leaveOidcReturnSignedIn(oidcErrorText(oidcError));
        return;
    }
    html('page-login', `
    <div class="flex justify-center items-center min-h-[70vh] jf-page px-4">
        <div class="w-full max-w-sm p-8 rounded-2xl bg-[#131822] border border-white/10 shadow-2xl">
            <div class="text-center mb-8">
                <img src="/web/assets/logo.png" alt="Rustyfin" class="w-20 h-20 mx-auto rounded-2xl shadow-xl mb-3 p-1 bg-white/5 border border-white/10">
                <h2 class="text-2xl font-black text-white tracking-tight">${t('nav.sign_in')}</h2>
                <p class="text-white/40 text-xs mt-1">${t('login.subtitle')}</p>
            </div>
            <form id="login-form" onsubmit="handleLogin(event)" class="space-y-4">
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">${t('common.username')}</label>
                    <input type="text" id="login-user" class="jf-input w-full" placeholder="${escapeHtml(t('common.username'))}" required autofocus>
                </div>
                <div>
                    <label class="block text-xs font-semibold text-white/70 mb-1.5">${t('common.password')}</label>
                    <input type="password" id="login-pass" class="jf-input w-full" placeholder="${escapeHtml(t('common.password'))}">
                </div>
                <button type="submit" class="jf-btn-primary w-full !py-3 font-bold text-sm shadow-lg mt-2">
                    ${t('login.submit')}
                </button>
                <div id="login-error" class="text-red-400 text-xs text-center hidden p-2 rounded bg-red-500/10 border border-red-500/20"></div>
            </form>
            <div id="oidc-sign-in"></div>
        </div>
    </div>`);
    loadOidcSignIn();
    if (data === 'oidc') completeOidcSignIn();
    else if (oidcError !== null) showLoginError(oidcErrorText(oidcError));
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
    handleSetupPassword, doLogout, showQuickConnectModal, authorizeQuickConnect, updateAuthUI,
};

// Shared state that inline handlers or other modules assign: reached through window
Object.defineProperties(window, {
    pendingSetup: { get: () => pendingSetup, set: value => { pendingSetup = value; } },
});
