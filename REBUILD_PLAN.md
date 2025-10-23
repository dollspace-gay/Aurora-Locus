# Aurora Locus Rebuild Plan - Using Rust ATProto SDK

## Current Problem
Aurora Locus has reimplemented the entire ATProto stack from scratch instead of using the Rust ATProto SDK that was already built. This caused:
- Massive code duplication
- Broken admin panel with unfixable issues
- Maintenance nightmare
- Wasted development effort

## Solution
Rebuild Aurora Locus as a thin PDS server layer on top of the Rust ATProto SDK.

---

## Architecture

```
┌─────────────────────────────────────────┐
│         Aurora Locus PDS Server         │
│  (Lightweight server implementation)    │
├─────────────────────────────────────────┤
│                                         │
│  ┌─────────────┐  ┌──────────────────┐ │
│  │ HTTP Server │  │  Admin Panel     │ │
│  │  (Axum)     │  │   (Web UI)       │ │
│  └─────────────┘  └──────────────────┘ │
│                                         │
│  ┌──────────────────────────────────┐  │
│  │   PDS Business Logic             │  │
│  │  - User accounts                 │  │
│  │  - Invite codes                  │  │
│  │  - Moderation                    │  │
│  │  - Storage                       │  │
│  └──────────────────────────────────┘  │
│                                         │
└─────────────────────────────────────────┘
                  ↓ uses
┌─────────────────────────────────────────┐
│       Rust ATProto SDK (atproto)        │
├─────────────────────────────────────────┤
│  - Agent                                │
│  - Client                               │
│  - XRPC                                 │
│  - Types (DID, Handle, etc.)            │
│  - Session Manager                      │
│  - Repository (MST, CAR, etc.)          │
│  - Rich Text                            │
│  - Moderation                           │
│  - OAuth/DPoP                           │
└─────────────────────────────────────────┘
```

---

## What to Keep from Current Aurora Locus
✅ **Database schema** - The SQLite schema for users, invites, etc.
✅ **Admin panel UI** - The HTML/CSS/JS frontend (once endpoints work)
✅ **Configuration** - The config file structure
✅ **Deployment docs** - The DEPLOYMENT.md guide

## What to Replace
❌ **All ATProto implementations** - Use SDK instead
❌ **Agent/Client code** - Use `atproto::agent` and `atproto::client`
❌ **XRPC handling** - Use `atproto::xrpc`
❌ **Type definitions** - Use `atproto::types`
❌ **Session management** - Use `atproto::session_manager`
❌ **Repository logic** - Use `atproto::repo`, `atproto::mst`, `atproto::car`

---

## Implementation Plan

### Phase 1: Setup (30 minutes)
1. ✅ Update `Cargo.toml` to depend on `atproto` SDK
2. ✅ Create new minimal `main.rs` using SDK
3. ✅ Set up basic Axum server with SDK client

### Phase 2: Core PDS Features (2 hours)
1. Implement account creation using SDK types
2. Implement authentication using SDK session manager
3. Implement repository operations using SDK repo module
4. Wire up database to store PDS-specific data

### Phase 3: Admin Endpoints (1 hour)
1. Implement invite code generation (simple, working version)
2. Implement user listing
3. Implement stats endpoint
4. **Test each endpoint as we go!**

### Phase 4: Testing (30 minutes)
1. Test account creation
2. Test posting
3. Test admin panel
4. Test federation

---

## Benefits of This Approach

### For Development
- **90% less code** - SDK handles all ATProto complexity
- **Type safety** - SDK's types prevent bugs
- **Tested code** - SDK is already tested
- **Focus on PDS features** - Not reimplementing ATProto

### For Users
- **Easier compilation** - One dependency (SDK)
- **Smaller binary** - Shared code with SDK
- **Better compatibility** - SDK matches official spec
- **Regular updates** - SDK updates independently

### For Maintenance
- **Bug fixes** - Fixed in SDK, not PDS
- **New features** - SDK adds them, PDS uses them
- **Clear separation** - PDS logic vs ATProto logic

---

## File Structure (New)

```
Aurora Locus/
├── Cargo.toml          # Depends on atproto SDK
├── src/
│   ├── main.rs         # Simple server setup
│   ├── config.rs       # Configuration
│   ├── db/
│   │   ├── mod.rs
│   │   ├── users.rs    # User database operations
│   │   ├── invites.rs  # Invite code database operations
│   │   └── schema.rs   # SQLite schema
│   ├── api/
│   │   ├── mod.rs
│   │   ├── auth.rs     # Authentication endpoints
│   │   ├── repo.rs     # Repository endpoints (uses SDK)
│   │   └── admin.rs    # Admin endpoints
│   └── admin_panel/
│       └── mod.rs      # Admin panel static files
├── static/             # Admin panel UI (unchanged)
└── data/               # SQLite databases
```

---

## Migration Path

### Step 1: Parallel Development
- Keep current Aurora Locus running
- Build new version in parallel
- Test extensively

### Step 2: Data Migration
- Export data from old version
- Import into new version
- Verify integrity

### Step 3: Switchover
- Stop old server
- Start new server
- Monitor for issues

---

## Success Criteria

✅ Server starts successfully
✅ Can create accounts
✅ Can post via ATProto client
✅ Admin panel loads
✅ Can generate invite codes
✅ Can view user list
✅ Can view stats
✅ Federation works
✅ Binary < 20MB
✅ Compile time < 3 minutes

---

## Timeline

- **Phase 1**: Tonight (30 min)
- **Phase 2**: Tomorrow (2 hours)
- **Phase 3**: Tomorrow (1 hour)
- **Phase 4**: Tomorrow (30 min)

**Total**: ~4 hours of focused work

---

**Status**: Ready to begin
**Date**: 2025-10-23
**Next Step**: Update Cargo.toml with SDK dependency
