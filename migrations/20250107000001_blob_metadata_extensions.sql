-- Add image metadata columns to blob_metadata table
-- Supports image dimensions, alt text, and thumbnails

-- Add columns for image dimensions
ALTER TABLE blob_metadata ADD COLUMN width INTEGER;
ALTER TABLE blob_metadata ADD COLUMN height INTEGER;

-- Add column for alt text (accessibility)
ALTER TABLE blob_metadata ADD COLUMN alt_text TEXT;

-- Add column for thumbnail CID (references another blob)
ALTER TABLE blob_metadata ADD COLUMN thumbnail_cid TEXT;

-- Create index for thumbnail lookups
CREATE INDEX IF NOT EXISTS idx_blob_thumbnail ON blob_metadata(thumbnail_cid);
