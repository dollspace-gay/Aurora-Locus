# Aurora Locus Admin Panel

Web-based administration interface for managing your Aurora Locus Personal Data Server.

## Overview

The admin panel provides a comprehensive web interface for:
- **Dashboard**: Overview of server statistics and metrics
- **User Management**: View and manage user accounts
- **Moderation Queue**: Review and action pending moderation items
- **Reports**: Handle user-submitted reports
- **Invite Codes**: Generate and manage invite codes
- **Settings**: Configure server parameters

## Accessing the Admin Panel

### URL
```
http://localhost:3000/admin/
```

Or on your configured hostname:
```
https://your-pds-domain.com/admin/
```

### Authentication

1. Navigate to the admin login page: `/admin/login.html`
2. Enter your admin credentials:
   - **Identifier**: Admin handle (e.g., `admin.localhost`) or email
   - **Password**: Your admin account password
3. Click "Sign In"

**Note**: Only accounts with admin roles can access the admin panel.

## Creating an Admin Account

### Via CLI (Recommended)

```bash
# Create a new admin account
aurora-locus admin create \
  --handle admin.localhost \
  --email admin@localhost \
  --password <secure-password>

# Grant admin role to existing account
aurora-locus admin grant-role \
  --did did:plc:xyz123... \
  --role admin
```

### Via Database (Development Only)

```sql
-- Insert admin role for an existing DID
INSERT INTO admin_roles (did, role, granted_by, granted_at)
VALUES ('did:plc:xyz123...', 'admin', 'system', datetime('now'));
```

## Features

### Dashboard

The dashboard provides an at-a-glance view of your PDS:

- **Total Users**: Count of registered accounts
- **Total Posts**: Number of posts created
- **Pending Reports**: Open moderation reports
- **Storage Used**: Total blob storage consumption
- **User Growth Chart**: Visual representation of new user signups
- **Activity Chart**: Breakdown of platform activity
- **Recent Activity**: Real-time feed of server events

**Auto-refresh**: Dashboard updates every 30 seconds

### User Management

Browse and manage user accounts:

- View all registered users
- See account details (DID, handle, email, creation date)
- Check account status (active, suspended)
- View user statistics (posts, followers, following)
- Suspend/unsuspend accounts
- Export user list to CSV

**Actions**:
- **View**: Display detailed account information
- **Suspend**: Temporarily disable an account (reversible)

**Search**: Filter users by handle, email, or DID

### Moderation Queue

Review and action flagged content:

- See all pending moderation items
- View reason for flagging
- Preview content
- See who reported the item
- Take action on flagged content

**Actions**:
- **Dismiss**: Mark the report as resolved without action
- **Takedown**: Remove the content and apply moderation action

**Filters**:
- All items
- By reason type (spam, harassment, etc.)
- By status (pending, reviewed)

### Reports

Manage user-submitted reports:

- List all moderation reports
- Filter by status (open, dismissed, resolved)
- View detailed report information
- Resolve reports with appropriate actions

**Report Details**:
- Reporter identity
- Reported subject (user or content)
- Reason type and description
- Timestamp
- Current status

### Invite Codes

Generate and manage registration invite codes:

- View all invite codes
- See usage statistics
- Track which codes have been used
- Disable codes

**Actions**:
- **Generate Codes**: Create new invite codes in bulk
- **Disable**: Deactivate an invite code

**Statistics**:
- Total codes generated
- Available codes
- Used codes

**Configuration**:
```javascript
// Generate 10 single-use invite codes
count: 10
useCount: 1
```

### Settings

Configure PDS parameters:

**General Settings**:
- Instance name
- Service URL
- Contact email

**Registration Settings**:
- Require invite codes
- Open registration
- Email verification

**Moderation Settings**:
- Auto-moderation rules
- Report thresholds
- Content policies

## API Endpoints

The admin panel uses the following XRPC endpoints:

### Statistics
```
GET /xrpc/com.atproto.admin.getStats
```
Returns server statistics (users, posts, reports, storage)

### User Management
```
GET /xrpc/com.atproto.admin.listAccounts?limit=100
GET /xrpc/com.atproto.admin.getAccount?did=did:plc:...
POST /xrpc/com.atproto.admin.updateSubjectStatus
```

### Moderation
```
GET /xrpc/com.atproto.admin.getModerationQueue?limit=50
GET /xrpc/com.atproto.admin.listReports?limit=50
GET /xrpc/com.atproto.admin.getReport?id=...
POST /xrpc/com.atproto.admin.resolveReport
```

### Invite Codes
```
GET /xrpc/com.atproto.admin.listInviteCodes?limit=100
POST /xrpc/com.atproto.admin.createInviteCodes
POST /xrpc/com.atproto.admin.disableInviteCode
```

## Security

### Authentication

- Admin panel requires valid JWT access token
- Tokens are stored in browser localStorage
- Admin role is verified on every API request
- Sessions expire after inactivity

### Authorization

Access control is enforced at multiple levels:

1. **Frontend**: Redirects non-authenticated users to login
2. **Backend**: Verifies JWT signature and admin role
3. **Database**: Checks admin_roles table for permissions

### Role Hierarchy

Aurora Locus supports multiple admin roles:

- **SuperAdmin**: Full system access, can grant roles
- **Admin**: Standard admin operations
- **Moderator**: Moderation actions only
- **Viewer**: Read-only access

### Best Practices

1. **Use Strong Passwords**: Minimum 12 characters, mixed case, numbers, symbols
2. **Limit Admin Accounts**: Only create admin accounts for trusted users
3. **Regular Audits**: Review admin actions in logs
4. **Secure Transport**: Always use HTTPS in production
5. **Session Management**: Log out when finished
6. **Principle of Least Privilege**: Grant minimum necessary permissions

## Customization

### Branding

Edit `static/admin/style.css` to customize colors and styling:

```css
:root {
    --primary-color: #3b82f6;  /* Primary accent color */
    --sidebar-bg: #1e293b;      /* Sidebar background */
    --danger-color: #ef4444;    /* Warning/danger actions */
}
```

### Logo

Replace the SVG logo in `static/admin/index.html`:

```html
<div class="logo">
    <!-- Your custom logo SVG or image -->
</div>
```

### Metrics

Add custom metrics to the dashboard by modifying `static/admin/script.js`:

```javascript
function loadDashboardData() {
    // Add your custom metrics fetch
    fetch(`${API_BASE}/your-custom-metric`)
        .then(res => res.json())
        .then(data => {
            // Update dashboard
        });
}
```

## Monitoring

### Activity Logging

All admin actions are logged with:
- Timestamp
- Admin DID/handle
- Action performed
- Target subject
- Result status

View logs:
```bash
# View admin action logs
grep "admin_action" /var/log/aurora-locus/server.log

# View login attempts
grep "admin_login" /var/log/aurora-locus/server.log
```

### Audit Trail

Admin actions are recorded in the database:

```sql
-- View recent admin actions
SELECT * FROM admin_actions
ORDER BY timestamp DESC
LIMIT 100;

-- Find actions by specific admin
SELECT * FROM admin_actions
WHERE admin_did = 'did:plc:...'
ORDER BY timestamp DESC;
```

## Troubleshooting

### Cannot Access Admin Panel

**Problem**: 404 error when visiting `/admin/`

**Solution**:
- Verify `static/admin/` directory exists
- Check server logs for static file serving errors
- Ensure tower-http `fs` feature is enabled

### Login Fails

**Problem**: "Access denied: Admin privileges required"

**Solution**:
- Verify account has admin role in `admin_roles` table
- Check JWT token is valid and not expired
- Review server logs for authentication errors

### Stats Not Loading

**Problem**: Dashboard shows 0 for all stats

**Solution**:
- Check database connectivity
- Verify tables exist (accounts, records, moderation_reports)
- Check browser console for API errors
- Ensure CORS headers allow admin requests

### Permission Denied

**Problem**: "Authorization error" when performing actions

**Solution**:
- Verify admin role has necessary permissions
- Check role hierarchy (some actions require SuperAdmin)
- Review backend authorization logs

## Development

### Running Locally

```bash
# Start Aurora Locus with admin panel
cargo run --release

# Access admin panel
open http://localhost:3000/admin/
```

### Testing

```bash
# Run admin panel tests
cargo test admin_panel

# Test admin API endpoints
curl -H "Authorization: Bearer $TOKEN" \
  http://localhost:3000/xrpc/com.atproto.admin.getStats
```

### Hot Reload (Development)

For frontend development with hot reload:

```bash
# Install live server
npm install -g live-server

# Serve admin panel with auto-reload
cd static/admin
live-server --port=8080 --proxy=/xrpc:http://localhost:3000/xrpc
```

## Production Deployment

### Nginx Configuration

```nginx
server {
    listen 443 ssl http2;
    server_name pds.example.com;

    # SSL configuration...

    # Admin panel
    location /admin/ {
        alias /var/www/aurora-locus/static/admin/;
        try_files $uri $uri/ /admin/index.html;
    }

    # API proxy
    location /xrpc/ {
        proxy_pass http://localhost:3000/xrpc/;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}
```

### Environment Variables

```bash
# Production settings
export LOG_LEVEL=info
export LOG_FORMAT=json
export ADMIN_SESSION_TIMEOUT=3600
export REQUIRE_ADMIN_2FA=true
```

### Security Hardening

1. **Enable 2FA**: Require two-factor authentication for admin accounts
2. **IP Allowlist**: Restrict admin panel to specific IP addresses
3. **Rate Limiting**: Apply stricter rate limits to admin endpoints
4. **Audit Logging**: Enable comprehensive audit logs
5. **Session Timeout**: Set appropriate timeout for admin sessions

## Support

For issues or questions:
- GitHub Issues: https://github.com/your-org/aurora-locus/issues
- Documentation: https://docs.aurora-locus.dev
- Community: https://discord.gg/aurora-locus

## License

Aurora Locus Admin Panel is part of the Aurora Locus PDS project.
See LICENSE file for details.
