-- Check active sessions for admin account
SELECT COUNT(*) as session_count FROM session
WHERE did IN (SELECT did FROM account WHERE handle LIKE '%admin%');

-- Show session details
SELECT id, did, expires_at, last_active FROM session
WHERE did IN (SELECT did FROM account WHERE handle LIKE '%admin%')
LIMIT 5;
