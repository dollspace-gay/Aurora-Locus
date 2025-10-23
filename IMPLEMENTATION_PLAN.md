# Aurora Locus PDS - Complete Implementation Plan

## Current Status (December 2024)

### ✅ Completed Phases (8/8 Core Features)

1. **Phase 1: Foundation** - Database, config, basic infrastructure
2. **Phase 2: Account System** - User registration, authentication, sessions
3. **Phase 3: Repository Operations** - MST implementation, record CRUD
4. **Phase 4: Blob Storage** - Media upload and storage
5. **Phase 5: Synchronization (Partial)** - Sequencer database, event types
6. **Phase 6: Identity & Federation** - DID resolution, handle caching
7. **Phase 7: Admin & Moderation** - Roles, moderation, labels, invites, reports
8. **Phase 8: Production Readiness** - Rate limiting, jobs, Docker, docs

### 📊 Feature Completeness: ~75%

**Implemented:**
- ✅ Account creation and authentication
- ✅ Repository structure and MST
- ✅ Blob storage (disk backend)
- ✅ Identity resolution with caching
- ✅ Admin system with RBAC
- ✅ Database migrations
- ✅ Docker deployment
- ✅ Background jobs

**Partially Implemented:**
- 🟡 Synchronization (sequencer exists, endpoints missing)
- 🟡 Federation (identity resolution done, sync missing)
- 🟡 Rate limiting (code exists, not integrated)
- 🟡 Invite system (created but not enforced)

**Not Implemented:**
- ❌ CAR file export
- ❌ WebSocket firehose
- ❌ PLC operation signing
- ❌ Blob serving (only storage)
- ❌ Account deletion workflow
- ❌ Email verification

---

## Implementation Plan: Phases 9-14

### Phase 9: Complete Synchronization (Week 9)
**Goal**: Finish federation sync protocol

**Priority**: High (Required for ATProto compliance)

#### Tasks:

1. **CAR File Export** (2-3 days)
   - [ ] Implement `RepositoryManager.export_car()`
   - [ ] Generate CAR from MST blocks
   - [ ] Handle `since` parameter for incremental sync
   - [ ] Add compression support
   - **Files**: `src/actor_store/repository.rs`
   - **Depends on**: Existing MST implementation

2. **Sync Endpoints** (2 days)
   - [ ] `com.atproto.sync.getRepo` - Full repository export
   - [ ] `com.atproto.sync.getLatestCommit` - Get latest commit info
   - [ ] `com.atproto.sync.getBlob` - Serve blob data
   - [ ] `com.atproto.sync.listRepos` - List available repositories
   - **Files**: Create `src/api/sync.rs`

3. **Event Recording** (1 day)
   - [ ] Hook sequencer into repository commits
   - [ ] Record commit events automatically
   - [ ] Record identity events on handle changes
   - [ ] Record account events on status changes
   - **Files**: `src/actor_store/repository.rs`, `src/account/manager.rs`

4. **WebSocket Firehose** (3-4 days) - OPTIONAL for later
   - [ ] Implement `com.atproto.sync.subscribeRepos`
   - [ ] WebSocket upgrade handling
   - [ ] Streaming event protocol
   - [ ] Cursor-based resumption
   - [ ] Backfill mechanism
   - **Files**: `src/api/sync.rs`, add `tokio-tungstenite` support
   - **Note**: Can be deferred to Phase 12

**Deliverables**: Other PDSs can sync user data from Aurora Locus

---

### Phase 10: Integration & Wiring (Week 10)
**Goal**: Connect existing components

**Priority**: High (Enables existing features)

#### Tasks:

1. **Rate Limiting Integration** (1 day)
   - [ ] Add rate limiter to AppContext
   - [ ] Wire `rate_limit_middleware` into router
   - [ ] Configure per-endpoint limits
   - [ ] Add rate limit headers to responses
   - **Files**: `src/main.rs`, `src/server.rs`, `src/rate_limit.rs`

2. **Invite Code Enforcement** (1 day)
   - [ ] Check invite code in `createAccount`
   - [ ] Mark code as used
   - [ ] Return error if invalid/expired
   - [ ] Optional: Disable invite requirement via config
   - **Files**: `src/api/server.rs`, `src/account/manager.rs`

3. **Moderation Enforcement** (2 days)
   - [ ] Middleware to check if account suspended/taken down
   - [ ] Block requests from moderated accounts
   - [ ] Return appropriate error messages
   - [ ] Admin bypass for review
   - [ ] Use `ModerationManager.is_taken_down()` and `is_suspended()`
   - **Files**: `src/api/middleware.rs`, create moderation middleware

4. **Label Integration** (1 day)
   - [ ] Return labels with content responses
   - [ ] Filter labeled content based on client preferences
   - [ ] Implement `com.atproto.label.queryLabels` endpoint
   - [ ] Use `LabelManager.get_labels()`
   - **Files**: `src/api/repo.rs`, create `src/api/labels.rs`

5. **Blob Serving** (1 day)
   - [ ] Implement blob GET endpoint
   - [ ] Content-Type header handling
   - [ ] Range request support
   - [ ] CDN-friendly caching headers
   - [ ] Use existing `BlobStore.get()` method
   - **Files**: `src/api/blob.rs`

6. **Session Cleanup Job** (1 day)
   - [ ] Implement `cleanup_expired_sessions` in AccountManager
   - [ ] Delete expired access tokens
   - [ ] Delete expired refresh tokens
   - [ ] Log cleanup results
   - **Files**: `src/account/manager.rs`, `src/jobs/tasks.rs`

**Deliverables**: All implemented features fully functional

---

### Phase 11: Account Lifecycle (Week 11)
**Goal**: Complete account management

**Priority**: Medium (Quality of life)

#### Tasks:

1. **Email Verification** (2-3 days)
   - [ ] Generate verification token on account creation
   - [ ] Send verification email
   - [ ] `com.atproto.server.requestEmailConfirmation` endpoint
   - [ ] `com.atproto.server.confirmEmail` endpoint
   - [ ] Mark email as verified in database
   - [ ] Optional: Require verified email for certain actions
   - **Files**: `src/account/manager.rs`, `src/api/server.rs`
   - **Depends on**: Email system (already exists)

2. **Password Reset** (2 days)
   - [ ] `com.atproto.server.requestPasswordReset` endpoint
   - [ ] Generate reset token
   - [ ] Send reset email
   - [ ] `com.atproto.server.resetPassword` endpoint
   - [ ] Validate token and update password
   - [ ] Invalidate all sessions on password change
   - **Files**: `src/account/manager.rs`, `src/api/server.rs`

3. **Account Deletion** (2 days)
   - [ ] `com.atproto.server.deleteAccount` endpoint
   - [ ] Require password confirmation
   - [ ] Mark account for deletion (soft delete)
   - [ ] Background job to purge data after grace period
   - [ ] Use `ActorStore.destroy()`
   - [ ] Delete blobs with `BlobStore.list_for_user()` and `delete()`
   - [ ] GDPR compliance
   - **Files**: `src/api/server.rs`, `src/jobs/tasks.rs`, `src/actor_store/store.rs`

4. **App Passwords** (2 days)
   - [ ] `com.atproto.server.createAppPassword` endpoint
   - [ ] Generate app-specific password
   - [ ] Store hashed app password
   - [ ] Authenticate with app password
   - [ ] Use `ValidatedSession.is_app_password` field
   - [ ] List/revoke app passwords
   - [ ] Different scopes/permissions for app passwords
   - **Files**: `src/account/manager.rs`, `src/api/server.rs`, `src/db/account.rs`

**Deliverables**: Complete account management workflow

---

### Phase 12: Advanced Features (Week 12)
**Goal**: Production-grade enhancements

**Priority**: Medium (Performance & reliability)

#### Tasks:

1. **Blob Metadata & Processing** (3 days)
   - [ ] Extract image dimensions on upload
   - [ ] Generate thumbnails
   - [ ] Store metadata (dimensions, mime type, alt text)
   - [ ] Use `BlobMetadata` struct
   - [ ] Implement `BlobStore.get_metadata()`
   - [ ] Return metadata with blob responses
   - **Files**: `src/blob_store/disk.rs`
   - **Dependencies**: Add `image` crate for processing

2. **Two-Phase Blob Upload** (2 days)
   - [ ] `com.atproto.repo.uploadBlob` - Stage blob
   - [ ] Store in temp location
   - [ ] Use `TempBlob` struct
   - [ ] Commit blob when record created
   - [ ] Cleanup orphaned temp blobs (background job)
   - [ ] Use `BlobStoreConfig.temp_dir`
   - **Files**: `src/blob_store/mod.rs`, `src/api/blob.rs`

3. **Batch Operations** (2 days)
   - [ ] Batch write API (multiple records in one transaction)
   - [ ] Use `PreparedWrite` struct
   - [ ] Validate all operations first
   - [ ] Atomic commit
   - [ ] Better performance for imports
   - **Files**: `src/actor_store/repository.rs`, `src/api/repo.rs`

4. **Record Validation** (2 days)
   - [ ] Schema validation using lexicons
   - [ ] Validate record structure matches collection schema
   - [ ] Custom validation rules
   - [ ] Return detailed validation errors
   - [ ] Use `WriteOp.validate` field
   - **Files**: `src/actor_store/repository.rs`
   - **Dependencies**: Parse lexicon JSON schemas

5. **WebSocket Firehose** (3-4 days) - If not done in Phase 9
   - [ ] Implement streaming sync protocol
   - [ ] Handle backpressure
   - [ ] Cursor management
   - [ ] Error recovery
   - **Files**: `src/api/sync.rs`

**Deliverables**: Production-quality feature set

---

### Phase 13: Identity & DID Operations (Week 13)
**Goal**: Complete identity management

**Priority**: Medium (Advanced identity features)

#### Tasks:

1. **PLC Operation Signing** (3-4 days)
   - [ ] Load repository signing keys
   - [ ] Construct PLC operation JSON
   - [ ] Sign with secp256k1
   - [ ] Implement `com.atproto.identity.signPlcOperation`
   - [ ] Implement `com.atproto.identity.requestPlcOperationSignature`
   - [ ] Validate operation format
   - **Files**: `src/api/identity.rs`
   - **Dependencies**: Add `secp256k1` crate

2. **PLC Directory Integration** (2 days)
   - [ ] Submit operations to plc.directory
   - [ ] Implement `com.atproto.identity.submitPlcOperation`
   - [ ] Handle directory responses
   - [ ] Invalidate cached DID docs after update
   - [ ] Use `IdentityResolver.invalidate_did()`
   - **Files**: `src/api/identity.rs`, `src/identity/resolver.rs`

3. **Handle Update Flow** (2 days)
   - [ ] Complete `com.atproto.identity.updateHandle`
   - [ ] Update account.handle in database
   - [ ] Emit identity event to sequencer
   - [ ] Update DID document if needed
   - [ ] Invalidate old handle in cache
   - [ ] Use `IdentityResolver.invalidate_handle()`
   - **Files**: `src/api/identity.rs`, `src/account/manager.rs`

4. **DID Web Support** (2 days)
   - [ ] Generate DID document for did:web
   - [ ] Well-known endpoint enhancement
   - [ ] Include verification methods
   - [ ] Include service endpoints
   - [ ] Sign DID document
   - **Files**: `src/api/well_known.rs`

**Deliverables**: Full DID/identity feature set

---

### Phase 14: Observability & Operations (Week 14)
**Goal**: Production monitoring and operations

**Priority**: Medium (Production requirements)

#### Tasks:

1. **Metrics & Telemetry** (3 days)
   - [ ] Add `prometheus` crate
   - [ ] Metrics endpoint `/metrics`
   - [ ] Track request counts, latencies
   - [ ] Track database query times
   - [ ] Track cache hit rates
   - [ ] Track background job execution
   - [ ] Track moderation actions
   - **Files**: Create `src/metrics.rs`, update all modules

2. **Structured Logging Enhancement** (2 days)
   - [ ] Add request IDs to all logs
   - [ ] Log slow queries
   - [ ] Log failed authentications
   - [ ] Log admin actions
   - [ ] JSON output option
   - [ ] Log sampling for high-volume endpoints
   - **Files**: `src/server.rs`, `src/api/middleware.rs`

3. **Health Checks Enhancement** (1 day)
   - [ ] Detailed health status endpoint
   - [ ] Check database connectivity
   - [ ] Check blob storage availability
   - [ ] Check background jobs status
   - [ ] Readiness vs. liveness probes
   - [ ] Use existing `jobs/tasks.rs` health check
   - **Files**: Create `src/api/health.rs`

4. **Admin Dashboard API** (3 days)
   - [ ] Stats endpoint (user count, blob storage used, etc.)
   - [ ] Recent activity logs
   - [ ] Moderation queue endpoint
   - [ ] System health metrics
   - [ ] Use `AuditLogEntry` struct
   - **Files**: `src/api/admin.rs` enhancements

5. **Backup & Recovery** (2 days)
   - [ ] Database backup script
   - [ ] Blob storage backup
   - [ ] Restore procedures
   - [ ] Documentation
   - [ ] Automated backup scheduling
   - **Files**: Create `scripts/backup.sh`, documentation

**Deliverables**: Production operations toolkit

---

## Optional Future Phases (15+)

### Phase 15: Alternative Storage Backends
**Goal**: Scalability options

- [ ] S3-compatible blob storage
- [ ] PostgreSQL database option
- [ ] Redis cache layer
- [ ] Distributed rate limiting

### Phase 16: Advanced Federation
**Goal**: Multi-PDS ecosystem

- [ ] Cross-PDS authentication
- [ ] Federated search
- [ ] Content aggregation
- [ ] Relay support

### Phase 17: Performance Optimization
**Goal**: Handle high load

- [ ] Database query optimization
- [ ] Connection pooling tuning
- [ ] Caching strategy refinement
- [ ] Load testing and benchmarking
- [ ] Horizontal scaling guide

### Phase 18: Admin Dashboard UI
**Goal**: User-friendly administration

- [ ] Web-based admin panel
- [ ] Moderation queue interface
- [ ] User management UI
- [ ] Metrics visualization
- [ ] Report review interface

---

## Priority Matrix

### Must Have (Before Public Launch)
1. ✅ Account system
2. ✅ Repository operations
3. ✅ Blob storage
4. ✅ Basic identity resolution
5. ✅ Admin/moderation system
6. 🔲 Rate limiting integration (Phase 10)
7. 🔲 Invite enforcement (Phase 10)
8. 🔲 Moderation enforcement (Phase 10)
9. 🔲 CAR export (Phase 9)
10. 🔲 Sync endpoints (Phase 9)
11. 🔲 Email verification (Phase 11)

### Should Have (For Federation)
12. 🔲 WebSocket firehose (Phase 9/12)
13. 🔲 Label integration (Phase 10)
14. 🔲 PLC operations (Phase 13)
15. 🔲 Complete identity management (Phase 13)
16. 🔲 Metrics/monitoring (Phase 14)

### Nice to Have (Quality of Life)
17. 🔲 Password reset (Phase 11)
18. 🔲 Account deletion (Phase 11)
19. 🔲 App passwords (Phase 11)
20. 🔲 Blob metadata (Phase 12)
21. 🔲 Batch operations (Phase 12)
22. 🔲 Admin dashboard (Phase 14)

---

## Development Workflow

### For Each Phase:

1. **Planning** (0.5 day)
   - Review task list
   - Identify dependencies
   - Create detailed task breakdown
   - Set acceptance criteria

2. **Implementation** (4-5 days)
   - Write code following existing patterns
   - Follow CLAUDE.md guidelines (no stubs!)
   - Comprehensive error handling
   - Use existing infrastructure

3. **Testing** (1 day)
   - Unit tests for new code
   - Integration tests for endpoints
   - Manual testing with curl/httpie
   - Update test scripts

4. **Documentation** (0.5 day)
   - Update API documentation
   - Update README if needed
   - Create/update phase progress doc
   - Add code comments

5. **Review** (end of day)
   - Run clippy
   - Check for warnings
   - Performance check
   - Security review

### Testing Strategy

**Unit Tests:**
- All business logic
- Data validation
- Error handling

**Integration Tests:**
- API endpoints
- Database operations
- Background jobs

**Manual Tests:**
- End-to-end workflows
- Federation scenarios
- Admin operations

**Load Tests** (Phase 17):
- Concurrent users
- Large repositories
- Blob uploads
- Sync operations

---

## Estimated Timeline

### Minimum Viable Federation (Phases 9-10)
**Time**: 2-3 weeks
**Result**: Can participate in ATProto network

### Complete PDS (Phases 9-14)
**Time**: 6-8 weeks
**Result**: Production-ready with all features

### With Optional Phases (15-18)
**Time**: 10-12 weeks
**Result**: Enterprise-grade PDS

---

## Success Criteria

### Phase 9 Complete When:
- [ ] Other PDSs can fetch user repositories via CAR export
- [ ] Sync endpoints return valid data
- [ ] Events recorded to sequencer on all changes
- [ ] Can sync incremental updates

### Phase 10 Complete When:
- [ ] Rate limiting prevents abuse
- [ ] Invites required for signup (if enabled)
- [ ] Suspended accounts cannot post
- [ ] Labels returned with content
- [ ] Blobs can be retrieved
- [ ] All background jobs running

### Phase 11 Complete When:
- [ ] Users can verify email
- [ ] Users can reset password
- [ ] Users can delete account
- [ ] App passwords work for third-party apps

### Phase 12 Complete When:
- [ ] Images have thumbnails
- [ ] Large uploads work reliably
- [ ] Batch imports are efficient
- [ ] WebSocket firehose streams events

### Phase 13 Complete When:
- [ ] Users can update DID documents
- [ ] Handle changes propagate correctly
- [ ] PLC directory operations succeed
- [ ] Both did:plc and did:web fully supported

### Phase 14 Complete When:
- [ ] Prometheus metrics available
- [ ] Logs are structured and searchable
- [ ] Health checks cover all systems
- [ ] Admin can monitor via API
- [ ] Backup/restore documented

---

## Risk Mitigation

### Technical Risks:

**CAR Export Complexity**
- Risk: CAR format is complex
- Mitigation: Use existing `libipld` crate, reference SDK implementation

**WebSocket Stability**
- Risk: WebSocket connections unstable
- Mitigation: Implement reconnection, cursor-based resumption, extensive testing

**PLC Integration**
- Risk: PLC directory API changes
- Mitigation: Abstract PLC client, version API calls

**Performance at Scale**
- Risk: SQLite doesn't scale
- Mitigation: Profile early, plan PostgreSQL migration (Phase 15)

### Operational Risks:

**Database Corruption**
- Risk: SQLite corruption under load
- Mitigation: WAL mode, regular backups, fsync properly

**Storage Exhaustion**
- Risk: Blobs fill disk
- Mitigation: Quotas, monitoring, cleanup jobs

**Security Vulnerabilities**
- Risk: Authentication bypass, injection
- Mitigation: Security review each phase, use prepared statements, rate limiting

---

## Next Immediate Steps

1. **Start Phase 9** - CAR export and sync endpoints
2. **Set up testing** - Create test suite for sync protocol
3. **Federation testing** - Deploy two instances, test sync between them
4. **Documentation** - Document sync protocol usage

**First Task**: Implement `export_car()` method in RepositoryManager

---

## Resources Needed

### Crates to Add:
- `image` - Image processing (Phase 12)
- `secp256k1` - Signing (Phase 13)
- `prometheus` - Metrics (Phase 14)
- `aws-sdk-s3` - S3 storage (Phase 15)

### Infrastructure:
- Test PDS instance for federation testing
- CI/CD pipeline for automated testing
- Staging environment
- Monitoring stack (Prometheus + Grafana)

### Documentation:
- API documentation (OpenAPI/Swagger)
- Federation guide
- Admin handbook
- Operations runbook

---

## Maintenance Plan

### Ongoing Tasks:
- Security updates
- Dependency updates
- ATProto spec compliance
- Bug fixes
- Performance tuning

### Monthly:
- Review logs for issues
- Check metrics for anomalies
- Database maintenance
- Backup verification

### Quarterly:
- Security audit
- Load testing
- Dependency audit
- Documentation review

---

**Aurora Locus is 75% complete. Phases 9-14 will bring it to 100% production readiness.**

The foundation is solid. The path is clear. Let's build the future of decentralized social networking! 🚀
