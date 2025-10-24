// Admin Login Page JavaScript - OAuth Flow

// Check if already logged in
if (localStorage.getItem('adminToken')) {
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
