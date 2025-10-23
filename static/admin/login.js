// Admin Login Page JavaScript

const API_BASE = '/xrpc';

// Check if already logged in
if (localStorage.getItem('adminToken')) {
    window.location.href = '/admin/index.html';
}

async function handleLogin(event) {
    event.preventDefault();

    const identifier = document.getElementById('identifier').value.trim();
    const password = document.getElementById('password').value;
    const remember = document.getElementById('remember').checked;

    // Clear previous errors
    hideError();

    // Validate inputs
    if (!identifier || !password) {
        showError('Please enter both identifier and password');
        return;
    }

    // Show loading state
    setLoading(true);

    try {
        // Call ATProto createSession endpoint
        const response = await fetch(`${API_BASE}/com.atproto.server.createSession`, {
            method: 'POST',
            headers: {
                'Content-Type': 'application/json',
            },
            body: JSON.stringify({
                identifier,
                password,
            }),
        });

        if (!response.ok) {
            const errorData = await response.json().catch(() => ({}));
            throw new Error(errorData.message || 'Login failed');
        }

        const data = await response.json();

        // Verify this is an admin account
        if (!isAdminAccount(data.did, data.handle)) {
            throw new Error('Access denied: Admin privileges required');
        }

        // Store the access token
        const storage = remember ? localStorage : sessionStorage;
        storage.setItem('adminToken', data.accessJwt);
        localStorage.setItem('adminToken', data.accessJwt); // Always use localStorage for now

        // Store session info
        localStorage.setItem('adminDid', data.did);
        localStorage.setItem('adminHandle', data.handle);

        // Redirect to admin panel
        window.location.href = '/admin/index.html';
    } catch (error) {
        console.error('Login error:', error);
        showError(error.message || 'Login failed. Please check your credentials.');
        setLoading(false);
    }
}

function isAdminAccount(did, handle) {
    // Check if this is an admin account
    // In a real system, this would verify admin privileges on the backend
    // For now, check if handle contains 'admin' or specific DIDs
    const adminHandles = ['admin', 'administrator', 'root'];
    const handleLower = handle.toLowerCase();

    return adminHandles.some(admin => handleLower.includes(admin));
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

// Focus on identifier field on page load
document.addEventListener('DOMContentLoaded', () => {
    document.getElementById('identifier').focus();
});

// Handle password visibility toggle (if needed in future)
function togglePasswordVisibility() {
    const passwordInput = document.getElementById('password');
    const type = passwordInput.getAttribute('type') === 'password' ? 'text' : 'password';
    passwordInput.setAttribute('type', type);
}
