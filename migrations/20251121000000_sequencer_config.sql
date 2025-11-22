-- Sequencer Configuration Table Migration
-- Adds sequencer_config table for managing sequencer state
--
-- This migration adds:
-- 1. sequencer_config table for key-value configuration storage
--
-- Migration created: 2025-11-21
-- Issue: Aurora-Locus-x9m (Admin Panel: Add sequencer/event stream management endpoints)

-- ====================================================================
-- Sequencer Configuration Table
-- ====================================================================

-- Sequencer configuration table - Manages sequencer runtime state
CREATE TABLE IF NOT EXISTS sequencer_config (
    key TEXT PRIMARY KEY NOT NULL,
    value TEXT NOT NULL,
    updated_at DATETIME NOT NULL DEFAULT (datetime('now'))
);

-- Initialize default values
INSERT OR IGNORE INTO sequencer_config (key, value) VALUES ('paused', '0');
INSERT OR IGNORE INTO sequencer_config (key, value) VALUES ('cursor_position', '0');

-- ====================================================================
-- Migration Complete
-- ====================================================================
