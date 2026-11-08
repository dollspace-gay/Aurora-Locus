-- Account preferences (app.bsky.actor.putPreferences), kept in write order.
CREATE TABLE account_pref (
    id          BIGSERIAL PRIMARY KEY,
    did         TEXT NOT NULL,
    namespace   TEXT NOT NULL,
    name        TEXT NOT NULL,
    value_json  TEXT NOT NULL
);
CREATE INDEX account_pref_did ON account_pref (did, id);

-- Records by rev, for read-after-write (records written after a rev).
CREATE INDEX record_rev ON record (did, rev);
