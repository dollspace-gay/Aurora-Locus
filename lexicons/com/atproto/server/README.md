# Vendored `com.atproto.server.*` reference lexicons

**Reference fixtures only. Not served, not used for runtime validation.** Aurora
serves `lexicons/tools/` for kryphocron; this directory is opt-in vendoring for
cycle correctness — a disk reference for the atproto wire shapes that Arc 4
(#185) brought into compliance, and a forward reference for the deferred
`SessionResponse` completeness work (Q5).

## Pin

Vendored from [`bluesky-social/atproto`](https://github.com/bluesky-social/atproto)
at commit `cf4843c339396e98fc0191b5c7ccf8db2e48da5b`.

## Files

| File | NSID | Arc 4 relevance |
|---|---|---|
| `createSession.json` | `com.atproto.server.createSession` | login reference (confirmed compliant) |
| `refreshSession.json` | `com.atproto.server.refreshSession` | fixed in Arc 4 — refresh token in `Authorization: Bearer` |
| `deleteSession.json` | `com.atproto.server.deleteSession` | fixed in Arc 4 — refresh-token auth; logout revokes |
| `getSession.json` | `com.atproto.server.getSession` | forward reference for the deferred Q5 work |
| `revokeAppPassword.json` | `com.atproto.server.revokeAppPassword` | confirmed compliant; Q9 paired refresh-token revoke |

## Refresh procedure

To update the pin to a newer upstream commit `<new-sha>`:

```sh
for f in createSession refreshSession deleteSession getSession revokeAppPassword; do
  curl -s "https://raw.githubusercontent.com/bluesky-social/atproto/<new-sha>/lexicons/com/atproto/server/$f.json" \
    -o "lexicons/com/atproto/server/$f.json"
done
# update the pin reference in this README, then commit.
```

Do not auto-pull from `main`; pin a specific commit so the reference is stable.
