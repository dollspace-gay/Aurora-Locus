# Aurora Locus Admin Panel Credentials

## Server Access Information

**Server Status**: RUNNING
**IP Address**: 129.222.126.193
**Port**: 3000

## Admin Panel URLs

**Local Access**: http://localhost:3000/admin/
**Remote Access**: http://129.222.126.193:3000/admin/

## Admin Login Credentials

**Username**: `admin.129.222.126.193`
**Password**: `AuroraAdmin2024!`

## Alternative Login

You can also use the email address:
**Email**: `admin@localhost`
**Password**: `AuroraAdmin2024!`

## How to Create the Admin Account

Since the account creation API currently has issues, you need to manually insert the admin account into the database. The SQL script has been created in `create_admin.sql`.

To create the admin account, you'll need a SQLite client installed. Once you have SQLite3:

```bash
sqlite3 data/account.sqlite < create_admin.sql
```

Or use any SQL database management tool (DB Browser for SQLite, DBeaver, etc.) to execute the SQL in `create_admin.sql`.

## What the Script Does

1. Creates an admin account with handle `admin.129.222.126.193`
2. Sets the email to `admin@localhost`
3. Creates a password hash for `AuroraAdmin2024!`
4. Grants the account `admin` role in the `admin_roles` table

## API Endpoints Available

Once logged in, you'll have access to:

- Dashboard with server metrics
- User management
- Moderation queue
- Content reports
- Invite code management
- Server settings
- Metrics & analytics

## Troubleshooting

If you cannot login:
1. Verify the server is running on port 3000
2. Check that the admin account exists in the database
3. Verify the admin_roles table has an entry for your DID
4. Check browser console for any JavaScript errors
5. Try accessing from localhost first before remote access

## Security Note

**IMPORTANT**: Change the default password after first login! This is a development/testing password and should not be used in production.
