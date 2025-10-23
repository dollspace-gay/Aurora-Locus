-- Grant admin privileges to the admin account
-- This script adds admin role to all accounts with 'admin' in the handle

-- First, let's see what accounts exist
SELECT 'Current accounts:' as info;
SELECT did, handle, email FROM account;

-- Grant admin role to the admin account
-- Using INSERT OR IGNORE to avoid errors if already exists
INSERT OR IGNORE INTO admin_roles (did, role, granted_by, granted_at)
SELECT did, 'admin', 'system', datetime('now')
FROM account
WHERE handle LIKE '%admin%';

-- Verify the admin roles were granted
SELECT 'Admin roles granted:' as info;
SELECT ar.did, a.handle, ar.role, ar.granted_at
FROM admin_roles ar
JOIN account a ON ar.did = a.did;
