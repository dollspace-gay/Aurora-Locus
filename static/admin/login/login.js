// Admin Login Page JavaScript - OAuth Flow

// One-time token-key migration (§8.1.1): move a legacy 'adminToken' to
// 'aurora-admin-token'. The main app (session.js) runs the same
// migration, but login.html doesn't load session.js, so it's inlined
// here too — a pre-rename operator landing on the login page still
// migrates and gets redirected into the app.
(function () {
    try {
        const legacy = localStorage.getItem('adminToken');
        if (legacy && !localStorage.getItem('aurora-admin-token')) {
            localStorage.setItem('aurora-admin-token', legacy);
        }
        if (legacy != null) localStorage.removeItem('adminToken');
    } catch (e) { /* localStorage unavailable */ }
})();

// Check if already logged in
if (localStorage.getItem('aurora-admin-token')) {
    window.location.href = '/admin/index.html';
}

// Theme + branding bootstrap (v0.9). The static <link>s already load the
// deployment-default theme's tokens, so the page is themed immediately; this
// adds <html data-theme> (so theme-scoped rules — Pride's rainbow borders, the
// button treatment — apply on this pre-auth page) and the operator's branding
// (custom logo / banner) from the unauthenticated /theme/login-branding
// endpoint. Best-effort: on failure the page keeps its default theme + the
// built-in stack-icon logo.
(function () {
    fetch('/theme/login-branding')
        .then(function (r) { return r.ok ? r.json() : null; })
        .then(function (b) {
            if (!b) return;
            if (b.theme) {
                document.documentElement.setAttribute('data-theme', b.theme);
                // Cache-bust the theme CSS keyed on the resolved default, so a
                // changed deployment-default repaints (no ?id — the server
                // still resolves the default). Mirrors the admin UI (#306).
                var v = encodeURIComponent(b.theme);
                var tokens = document.getElementById('theme-tokens');
                var effects = document.getElementById('theme-effects');
                if (tokens) tokens.setAttribute('href', '/theme/active.css?v=' + v);
                if (effects) effects.setAttribute('href', '/theme/active-effects.css?v=' + v);
            }
            if (b.logoUrl) {
                var logo = document.getElementById('login-logo');
                if (logo) {
                    var img = document.createElement('img');
                    img.src = b.logoUrl;
                    img.alt = 'Logo';
                    logo.replaceChildren(img);
                }
            }
            if (b.bannerUrl) {
                var header = document.querySelector('.login-header');
                if (header) {
                    // Banner under a theme-token scrim so the wordmark stays legible.
                    header.style.backgroundImage =
                        'linear-gradient(var(--color-surface-overlay), var(--color-surface-overlay)), ' +
                        'url("' + b.bannerUrl.replace(/"/g, '%22') + '")';
                }
            }
            // Operator title/subtitle text + color overrides. Empty = keep the
            // built-in wordmark / theme-token color (the CSS var fallback).
            var titleEl = document.getElementById('login-title');
            var subtitleEl = document.getElementById('login-subtitle');
            if (titleEl) {
                if (b.titleText) titleEl.textContent = b.titleText;
                if (b.titleColor) titleEl.style.setProperty('--login-title-color', b.titleColor);
            }
            if (subtitleEl) {
                if (b.subtitleText) subtitleEl.textContent = b.subtitleText;
                if (b.subtitleColor) subtitleEl.style.setProperty('--login-subtitle-color', b.subtitleColor);
            }
        })
        .catch(function () { /* keep the default theme + built-in logo */ });
})();

// Check for OAuth callback parameters
const urlParams = new URLSearchParams(window.location.search);
const authCode = urlParams.get('code');
const authState = urlParams.get('state');
const authError = urlParams.get('error');

if (authError) {
    showError(`Authentication failed: ${authError}`);
    // Clear URL parameters
    window.history.replaceState({}, document.title, window.location.pathname);
} else if (authCode && authState) {
    // OAuth callback - exchange code for tokens
    handleOAuthCallback(authCode, authState);
}

async function handleOAuthCallback(code, state) {
    try {
        setLoading(true, 'oauth-login-btn');

        // The backend will exchange the code for tokens
        // The callback endpoint is already handling this
        const callbackUrl = `/admin-oauth/callback?code=${encodeURIComponent(code)}&state=${encodeURIComponent(state)}&iss=${encodeURIComponent(urlParams.get('iss') || '')}`;

        const response = await fetch(callbackUrl);

        if (!response.ok) {
            throw new Error(`Authentication failed: ${response.statusText}`);
        }

        const data = await response.json();

        // Store tokens in localStorage.
        //
        // The refresh token is now stored (§8.1.2 / #268): api/client.js's
        // silent refresh-on-401 is the consumer that previously did not
        // exist. localStorage is the chosen store (same XSS-exfil surface
        // as the access token, documented as the accepted threat model).
        // The server omits refresh_token only if it minted none, so guard.
        localStorage.setItem('aurora-admin-token', data.access_token);
        if (data.refresh_token) {
            localStorage.setItem('aurora-admin-refresh-token', data.refresh_token);
        }
        localStorage.setItem('adminDid', data.did);
        localStorage.setItem('adminRole', data.role || 'admin');

        // Clear URL parameters
        window.history.replaceState({}, document.title, '/admin/login.html');

        // Redirect to the Aurora-Locus admin
        window.location.href = '/admin/index.html';
    } catch (error) {
        console.error('OAuth callback error:', error);
        showError(error.message || 'Authentication failed');
        setLoading(false, 'oauth-login-btn');

        // Clear URL parameters on error
        window.history.replaceState({}, document.title, '/admin/login.html');
    }
}

// Password-based admin login (chainlink #435). Calls the working
// /admin-oauth/password-login endpoint — a Bearer-token alternative to the OAuth
// path, which is blocked upstream by the proto-blue-oauth DPoP `exp` issue
// (#434) — and stows the returned tokens under the SAME localStorage keys the
// OAuth callback writes, so the rest of the admin UI (api/client.js, session.js)
// reads them identically.
async function handlePasswordLogin(event) {
    event.preventDefault();

    const identifier = document.getElementById('admin-login-identifier').value.trim();
    const password = document.getElementById('admin-login-password').value;

    hideError();

    // Client-side required-field guard (belt-and-suspenders with the inputs'
    // `required` attribute).
    if (!identifier || !password) {
        showError('Enter your handle/DID and password.');
        return;
    }

    setLoading(true, 'password-login-btn');

    try {
        const response = await fetch('/admin-oauth/password-login', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ identifier, password }),
        });

        if (!response.ok) {
            // Generic message — no enumeration between wrong identifier / wrong
            // password (401) and no-admin-role (403). The specific code is logged
            // for debugging only, never surfaced.
            console.debug('admin password login failed:', response.status);
            showError('Login failed. Check your credentials or admin role.');
            setLoading(false, 'password-login-btn');
            return;
        }

        const data = await response.json();

        // Same keys as handleOAuthCallback above, read by api/client.js +
        // session.js.
        localStorage.setItem('aurora-admin-token', data.access_token);
        if (data.refresh_token) {
            localStorage.setItem('aurora-admin-refresh-token', data.refresh_token);
        }
        localStorage.setItem('adminDid', data.did);
        localStorage.setItem('adminRole', data.role || 'admin');

        window.location.href = '/admin/index.html';
    } catch (error) {
        console.error('password login error:', error);
        showError('Login failed. Check your credentials or admin role.');
        setLoading(false, 'password-login-btn');
    }
}

// OAuth admin sign-in. Reads the SAME identifier field as the password form
// (admin logins are local-only per Fix 1, so one field serves both). This is a
// type="button", so it never submits the form — preventDefault is defensive.
async function handleOAuthLogin(event) {
    event.preventDefault();

    const identifier = document.getElementById('admin-login-identifier').value.trim();

    hideError();

    if (!identifier) {
        showError('Enter your handle or DID to continue.');
        return;
    }

    setLoading(true, 'oauth-login-btn');

    try {
        window.location.href = `/admin-oauth/login?handle=${encodeURIComponent(identifier)}`;
    } catch (error) {
        console.error('OAuth initiation error:', error);
        showError('Failed to start OAuth flow');
        setLoading(false, 'oauth-login-btn');
    }
}

function setLoading(isLoading, btnId) {
    const btn = document.getElementById(btnId || 'password-login-btn');
    if (!btn) return; // null-safe: never throw if the target button is absent
    const btnText = btn.querySelector('.btn-text');
    const btnSpinner = btn.querySelector('.btn-spinner');

    if (isLoading) {
        btn.disabled = true;
        btnText.style.display = 'none';
        btnSpinner.style.display = 'inline-block';
    } else {
        btn.disabled = false;
        btnText.style.display = 'inline';
        btnSpinner.style.display = 'none';
    }
}

function showError(message) {
    const errorElement = document.getElementById('error-message');
    errorElement.textContent = message;
    errorElement.style.display = 'block';
}

function hideError() {
    const errorElement = document.getElementById('error-message');
    errorElement.style.display = 'none';
}

// Wire the form + OAuth button. The password "Sign in" is the form's submit
// (handlePasswordLogin, which preventDefault()s and does the real JSON POST);
// "Sign in with OAuth" is a type=button sharing the identifier field. Wired via
// addEventListener rather than inline onsubmit so the binding is robust, and
// guarded on readyState so it runs whether or not DOMContentLoaded has fired.
function wireAdminLogin() {
    const form = document.getElementById('admin-login-form');
    if (form) form.addEventListener('submit', handlePasswordLogin);

    const oauthBtn = document.getElementById('oauth-login-btn');
    if (oauthBtn) oauthBtn.addEventListener('click', handleOAuthLogin);

    const idField = document.getElementById('admin-login-identifier');
    if (idField) idField.focus();
}

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', wireAdminLogin);
} else {
    wireAdminLogin();
}
