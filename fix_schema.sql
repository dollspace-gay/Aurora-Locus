-- Fix invite_code table
ALTER TABLE invite_code ADD COLUMN expires_at DATETIME;
ALTER TABLE invite_code ADD COLUMN note TEXT;

-- Fix account_moderation table 
ALTER TABLE account_moderation ADD COLUMN moderated_by TEXT;
ALTER TABLE account_moderation ADD COLUMN moderated_at DATETIME;
ALTER TABLE account_moderation ADD COLUMN reversed INTEGER NOT NULL DEFAULT 0;
ALTER TABLE account_moderation ADD COLUMN reversed_at DATETIME;
ALTER TABLE account_moderation ADD COLUMN reversed_by TEXT;
ALTER TABLE account_moderation ADD COLUMN reversal_reason TEXT;
ALTER TABLE account_moderation ADD COLUMN report_id INTEGER;
ALTER TABLE account_moderation ADD COLUMN notes TEXT;
