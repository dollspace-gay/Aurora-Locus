-- An install imported from a reference PDS: when, and what the import
-- brought over (shown in the console). Tokens the reference issued before
-- `imported_at` are accepted until they expire.
CREATE TABLE reference_import (
    id           INTEGER PRIMARY KEY NOT NULL CHECK (id = 1),
    imported_at  INTEGER NOT NULL,
    report       TEXT NOT NULL
) STRICT;
