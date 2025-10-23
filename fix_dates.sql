-- Fix invite code dates from 2025 to 2024
UPDATE invite_code 
SET created_at = REPLACE(created_at, '2025-', '2024-')
WHERE created_at LIKE '2025-%';
