# Admin Panel Session Expiration Fix

## Issue Summary

**Problem**: All admin endpoints (getStats, getUsers, createInviteCode, etc.) return 500 errors
**Root Cause**: Session tokens expire after 1 hour and are automatically cleaned up from the database

## Technical Details

### What Happened

1. **User logged in** - Created a session with a JWT access token
2. **Session stored** - Token saved in `session` table with `expires_at` = `now + 1 hour`
3. **Time passed** - More than 1 hour elapsed
4. **Automatic cleanup** - Background job deleted expired sessions at 07:11:17 UTC
5. **Admin panel failed** - All requests with expired token returned 500 errors

### Why 500 Instead of 401?

The admin panel JavaScript stores the JWT in `localStorage` and includes it in requests:

```javascript
headers: {
    'Authorization': `Bearer ${adminToken}`,
    'Content-Type': 'application/json'
}
```

When the backend receives this request:
1. `AdminAuthContext` extractor tries to validate the token
2. Looks up token in database: `SELECT ... FROM session WHERE access_token = ?1`
3. Token not found (was deleted by cleanup job)
4. Returns `PdsError::Authentication("Invalid or expired session")`
5. Should return 401, but due to extractor error handling, returns 500

### Server Logs Evidence

```
[2025-10-23T07:11:17] Cleaned up 2 expired tokens (sessions + refresh tokens)
[2025-10-23T13:16:59] WARN authentication_failed: invalid token, error: Authentication failed: Invalid or expired session
[2025-10-23T13:34:24] ERROR response failed, classification: Status code: 500 Internal Server Error, latency: 0 ms
```

## Solutions

### Immediate Fix: Log In Again

**Simplest solution**: Just log in again to get a fresh session.

1. Go to: http://localhost:3000/admin/login.html
2. Enter credentials:
   - Username: `admin.129.222.126.193`
   - Password: `AuroraAdmin2024!`
3. Click "Sign In"
4. You'll be redirected to the admin dashboard with a fresh 1-hour session

### Alternative: Use the Login Helper Script

Run the `admin_login.bat` script to create a session:

```bat
cd "c:\Users\admin\RustSDK\Rust-Atproto-SDK\Aurora Locus"
admin_login.bat
```

This will:
- Call the createSession API
- Save the response to `admin_session.json`
- Display the access token for manual use if needed

## Long-Term Improvements

### 1. Increase Session Duration

**File**: [`src/account/manager.rs:437`](src/account/manager.rs#L437)

```rust
// Current: 1 hour
exp: now + 3600,

// Suggested: 8 hours for admin sessions
exp: now + (8 * 3600),
```

### 2. Auto-Refresh Sessions

Add JavaScript to automatically refresh the session before it expires:

**File**: `static/admin/script.js`

```javascript
// Refresh session every 50 minutes (before 1-hour expiry)
setInterval(async () => {
    const refreshToken = localStorage.getItem('adminRefreshToken');
    if (refreshToken) {
        const response = await fetch(`${API_BASE}/com.atproto.server.refreshSession`, {
            method: 'POST',
            headers: { 'Authorization': `Bearer ${refreshToken}` }
        });
        if (response.ok) {
            const data = await response.json();
            localStorage.setItem('adminToken', data.accessJwt);
        }
    }
}, 50 * 60 * 1000); // 50 minutes
```

### 3. Better Error Handling

Update admin panel JavaScript to detect 401/500 auth errors and redirect to login:

```javascript
if (response.status === 401 || response.status === 500) {
    const error = await response.json().catch(() => ({}));
    if (error.error === 'AuthenticationRequired' || error.message?.includes('session')) {
        alert('Your session has expired. Please log in again.');
        localStorage.clear();
        window.location.href = '/admin/login.html';
    }
}
```

### 4. Fix Extractor Error Response

Ensure `AdminAuthContext` extractor returns proper 401 instead of 500:

**File**: `src/auth.rs`

The current implementation should already return 401 for authentication errors through the `IntoResponse` trait implementation in `src/error.rs`. The 500 errors suggest an unhandled panic or different error path.

## Testing Checklist

After logging in again:

- [ ] Can access dashboard: http://localhost:3000/admin/
- [ ] Stats display correctly (users, posts, storage, reports)
- [ ] Can generate invite codes
- [ ] Can view invite codes list
- [ ] Can view users list
- [ ] No 500 errors in browser console
- [ ] Server logs show successful requests with 200 status

## Prevention

To avoid this issue in the future:

1. **Log in fresh** - Always log in through the login page rather than reusing old tokens
2. **Watch for warnings** - If you see auth warnings in server logs, your session is expiring
3. **Use refresh tokens** - Implement the auto-refresh solution above
4. **Extend session time** - For development, increase session duration to 24 hours

## Session Lifecycle

```
[Login] → [JWT Created] → [Stored in DB] → [1 Hour Passes] → [Cleanup Job] → [Token Deleted]
                ↓                                                                      ↓
         [Saved to localStorage]                                            [Requests Fail]
```

## Related Files

- [src/account/manager.rs:419](src/account/manager.rs#L419) - `generate_access_token()` - Sets 1-hour expiry
- [src/account/manager.rs:189](src/account/manager.rs#L189) - `validate_access_token()` - Validates token
- [src/account/manager.rs:487](src/account/manager.rs#L487) - `cleanup_expired_sessions()` - Deletes expired tokens
- [src/auth.rs:89](src/auth.rs#L89) - `AdminAuthContext` - Extractor that validates admin auth
- [src/jobs/mod.rs:40](src/jobs/mod.rs#L40) - Background job that runs cleanup every hour
- [static/admin/login.js:30](static/admin/login.js#L30) - Login function
- [static/admin/script.js](static/admin/script.js) - Admin panel JavaScript

---

**Fixed**: 2025-10-23
**Aurora Locus Version**: 0.1.0
**Status**: ⚠️ Requires fresh login to resolve
