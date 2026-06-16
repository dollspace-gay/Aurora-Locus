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
        setLoading(true);

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
        // adminRefreshToken is intentionally NOT stored: a real
        // refresh-token flow is planned for v0.3's token-lifecycle
        // design pass (see admin UI audit findings #2, #4, #12). The
        // server may still emit `refresh_token` in the response —
        // we just discard it client-side until the consumer exists.
        // Storing without consuming would expand the localStorage
        // attack surface for zero current value.
        localStorage.setItem('aurora-admin-token', data.access_token);
        localStorage.setItem('adminDid', data.did);
        localStorage.setItem('adminRole', data.role || 'admin');

        // Clear URL parameters
        window.history.replaceState({}, document.title, '/admin/login.html');

        // Redirect to the Aurora-Locus admin
        window.location.href = '/admin/index.html';
    } catch (error) {
        console.error('OAuth callback error:', error);
        showError(error.message || 'Authentication failed');
        setLoading(false);

        // Clear URL parameters on error
        window.history.replaceState({}, document.title, '/admin/login.html');
    }
}

async function handleLogin(event) {
    event.preventDefault();

    const handle = document.getElementById('handle').value.trim();

    // Clear previous errors
    hideError();

    // Show loading state
    setLoading(true);

    try {
        // Build OAuth initiation URL
        const oauthUrl = `/admin-oauth/login${handle ? `?handle=${encodeURIComponent(handle)}` : ''}`;

        // Redirect to OAuth flow
        window.location.href = oauthUrl;
    } catch (error) {
        console.error('OAuth initiation error:', error);
        showError(error.message || 'Failed to start OAuth flow');
        setLoading(false);
    }
}

function setLoading(isLoading) {
    const btn = document.getElementById('login-btn');
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

// Handle Enter key in form
document.getElementById('login-form').addEventListener('submit', handleLogin);

// Focus on handle field on page load
document.addEventListener('DOMContentLoaded', () => {
    document.getElementById('handle').focus();
});
