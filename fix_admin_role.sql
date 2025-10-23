-- Fix admin role - insert into correct table
-- First, check which table exists
SELECT name FROM sqlite_master WHERE type='table' AND (name LIKE '%admin%' OR name LIKE '%role%');

-- Insert admin role into the admin_role table (singular)
INSERT OR REPLACE INTO admin_role (did, role, granted_by, granted_at, revoked, notes)
SELECT did, 'admin', 'system', datetime('now'), 0, 'Initial admin setup'
FROM account
WHERE handle LIKE '%admin%';

-- Verify
SELECT ar.did, a.handle, ar.role, ar.granted_at
FROM admin_role ar
JOIN account a ON ar.did = a.did;
