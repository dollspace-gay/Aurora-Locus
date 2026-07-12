// Admin password sign-in page (fallback path, not surfaced from the main login
// page). Posts to the /admin-oauth/password-login endpoint (chainlink #435) and
// stows the returned tokens under the SAME localStorage keys the OAuth callback
// writes, so the rest of the admin UI (api/client.js, session.js) reads them
// identically.

// One-time token-key migration (§8.1.1): move a legacy 'adminToken' to
// 'aurora-admin-token', mirroring login.js/session.js.
(function () {
    try {
        const legacy = localStorage.getItem('adminToken');
        if (legacy && !localStorage.getItem('aurora-admin-token')) {
            localStorage.setItem('aurora-admin-token', legacy);
        }
        if (legacy != null) localStorage.removeItem('adminToken');
    } catch (e) { /* localStorage unavailable */ }
})();

// Already logged in → go straight to the app.
if (localStorage.getItem('aurora-admin-token')) {
    window.location.href = '/admin/index.html';
}

async function handlePasswordLogin(event) {
    event.preventDefault();

    const identifier = document.getElementById('admin-login-identifier').value.trim();
    const password = document.getElementById('admin-login-password').value;

    hideError();

    if (!identifier || !password) {
        showError('Enter your handle/DID and password.');
        return;
    }

    setLoading(true);

    try {
        const response = await fetch('/admin-oauth/password-login', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ identifier, password }),
        });

        if (!response.ok) {
            // Generic message — no enumeration between wrong identifier / wrong
            // password (401) and no-admin-role (403).
            console.debug('admin password login failed:', response.status);
            showError('Login failed. Check your credentials or admin role.');
            setLoading(false);
            return;
        }

        const data = await response.json();

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
        setLoading(false);
    }
}

function setLoading(isLoading) {
    const btn = document.getElementById('password-login-btn');
    if (!btn) return;
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
    const el = document.getElementById('error-message');
    el.textContent = message;
    el.style.display = 'block';
}

function hideError() {
    document.getElementById('error-message').style.display = 'none';
}

function wirePasswordLogin() {
    const form = document.getElementById('password-login-form');
    if (form) form.addEventListener('submit', handlePasswordLogin);
    const idField = document.getElementById('admin-login-identifier');
    if (idField) idField.focus();
}

if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', wirePasswordLogin);
} else {
    wirePasswordLogin();
}
