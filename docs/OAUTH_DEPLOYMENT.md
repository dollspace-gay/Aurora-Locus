# OAuth 2.1 Production Deployment & Rollback Procedures

## Overview

This document outlines the procedures for deploying OAuth 2.1 to production, monitoring its performance, and rolling back if necessary.

## Pre-Deployment Checklist

### 1. Database Migrations

- [ ] Backup all databases (account, sequencer, identity cache)
- [ ] Review migration scripts in `migrations/20250105000000_oauth_tables.sql`
- [ ] Test migrations in staging environment
- [ ] Verify rollback scripts are ready
- [ ] Confirm migration execution plan with DBA

### 2. Configuration

- [ ] Review OAuth feature flags configuration
- [ ] Set appropriate `OAUTH_ROLLOUT_PERCENTAGE` (start with 0%)
- [ ] Configure `OAUTH_REQUIRE_DPOP` for production (recommend `false` initially)
- [ ] Set `JWT_SUNSET_DATE` (recommend 90 days from deployment)
- [ ] Update `OAUTH_MIGRATION_GUIDE_URL` to point to your documentation

### 3. Monitoring

- [ ] Grafana dashboards configured and tested
- [ ] Prometheus metrics endpoint accessible
- [ ] Alert rules configured for OAuth errors
- [ ] Log aggregation ready (check OAuth-related log patterns)
- [ ] On-call rotation notified of deployment

### 4. Documentation

- [ ] OAuth integration guide published
- [ ] Internal runbook updated
- [ ] Support team briefed on common OAuth issues
- [ ] Migration timeline communicated to users

### 5. Testing

- [ ] All OAuth unit tests passing (`cargo test oauth`)
- [ ] Integration tests completed in staging
- [ ] Load testing completed (verify performance under load)
- [ ] Security audit completed (PKCE, DPoP, token rotation)
- [ ] Compatibility testing with major clients

## Deployment Phases

### Phase 1: Infrastructure Preparation (Day 1)

#### Step 1.1: Database Migration

```bash
# 1. Create backup
sqlite3 data/account.sqlite ".backup data/account.sqlite.backup"

# 2. Run OAuth migrations
sqlite3 data/account.sqlite < migrations/20250105000000_oauth_tables.sql

# 3. Verify tables created
sqlite3 data/account.sqlite ".tables"
# Expected: authorization_request, token, device, oauth_client
```

#### Step 1.2: Deploy Code

```bash
# 1. Build release binary
cargo build --release

# 2. Stop current server
systemctl stop aurora-locus

# 3. Deploy new binary
cp target/release/aurora-locus /usr/local/bin/
chmod +x /usr/local/bin/aurora-locus

# 4. Verify binary version
/usr/local/bin/aurora-locus --version
```

#### Step 1.3: Configure Feature Flags

Edit `.env` or environment configuration:

```bash
# OAuth Feature Flags - Phase 1 (Infrastructure Only)
OAUTH_ENABLED=false                    # Keep disabled initially
OAUTH_ROLLOUT_PERCENTAGE=0             # 0% rollout
OAUTH_REQUIRE_DPOP=false               # Optional during testing
OAUTH_ENABLE_AUTHORIZE=false           # Disabled
OAUTH_ENABLE_TOKEN=false               # Disabled
OAUTH_ENABLE_DEVICE_MANAGEMENT=false   # Disabled
OAUTH_ALLOW_JWT_FALLBACK=true          # Allow JWT fallback
```

#### Step 1.4: Start Server

```bash
# Start service
systemctl start aurora-locus

# Verify startup
systemctl status aurora-locus
journalctl -u aurora-locus -f
```

#### Step 1.5: Smoke Tests

```bash
# 1. Health check
curl https://pds.example.com/xrpc/_health

# 2. Verify OAuth endpoints are disabled
curl https://pds.example.com/oauth/authorize
# Expected: 404 or error (endpoints disabled)

# 3. Verify JWT still works
curl -H "Authorization: Bearer jwt_token" \
  https://pds.example.com/xrpc/com.atproto.server.describeServer
# Expected: 200 OK
```

### Phase 2: Gradual Rollout (Days 2-7)

#### Day 2: Enable OAuth for 1% of Users

```bash
# Update feature flags
OAUTH_ENABLED=true
OAUTH_ROLLOUT_PERCENTAGE=1             # 1% rollout
OAUTH_ENABLE_AUTHORIZE=true            # Enable authorize endpoint
OAUTH_ENABLE_TOKEN=true                # Enable token endpoint
OAUTH_ALLOW_JWT_FALLBACK=true          # Keep JWT fallback

# Restart service
systemctl restart aurora-locus
```

**Monitoring:**
- Check `oauth_authorization_requests_total` metric
- Watch for `oauth_token_exchanges_total` increases
- Monitor error rates (`oauth_pkce_verification_failures_total`)
- Review logs for OAuth-related errors

#### Day 3-4: Increase to 5%

```bash
OAUTH_ROLLOUT_PERCENTAGE=5
systemctl restart aurora-locus
```

#### Day 5-6: Increase to 10%

```bash
OAUTH_ROLLOUT_PERCENTAGE=10
systemctl restart aurora-locus
```

#### Day 7: Increase to 25%

```bash
OAUTH_ROLLOUT_PERCENTAGE=25
systemctl restart aurora-locus
```

### Phase 3: Full Rollout (Days 8-14)

#### Day 8: 50% Rollout

```bash
OAUTH_ROLLOUT_PERCENTAGE=50
systemctl restart aurora-locus
```

#### Day 10: 75% Rollout

```bash
OAUTH_ROLLOUT_PERCENTAGE=75
systemctl restart aurora-locus
```

#### Day 12: 100% Rollout

```bash
OAUTH_ROLLOUT_PERCENTAGE=100
systemctl restart aurora-locus
```

#### Day 14: Enable DPoP Enforcement (Optional)

```bash
# Only if DPoP adoption is >90%
OAUTH_REQUIRE_DPOP=true
systemctl restart aurora-locus
```

### Phase 4: JWT Deprecation (Weeks 3-12)

#### Week 3: Add Deprecation Warnings

Configuration already includes deprecation warnings in JWT responses.

**Monitor:**
- `jwt_deprecation_warnings_total` metric
- Track OAuth vs JWT usage ratio

#### Week 6: Communicate Sunset Timeline

- Email all users with JWT sunset date
- Update documentation with migration deadlines
- Provide support resources for migration assistance

#### Week 10: Reduce JWT Sunset Window

```bash
# Reduce JWT sunset date to 30 days
# Update in database or config
```

#### Week 12: Disable JWT (Optional)

```bash
# Only if OAuth adoption >99%
OAUTH_ALLOW_JWT_FALLBACK=false
systemctl restart aurora-locus
```

## Monitoring Dashboard

### Key Metrics to Monitor

1. **Authorization Success Rate**
   ```promql
   rate(oauth_authorization_requests_total{status="success"}[5m]) /
   rate(oauth_authorization_requests_total[5m])
   ```
   - **Target:** >99%
   - **Alert:** <95%

2. **Token Exchange Success Rate**
   ```promql
   rate(oauth_token_exchanges_total{status="success"}[5m]) /
   rate(oauth_token_exchanges_total[5m])
   ```
   - **Target:** >99%
   - **Alert:** <98%

3. **PKCE Verification Failures**
   ```promql
   rate(oauth_pkce_verification_failures_total[5m])
   ```
   - **Target:** <0.01%
   - **Alert:** >1%

4. **DPoP Verification Failures**
   ```promql
   rate(oauth_dpop_verification_failures_total[5m])
   ```
   - **Target:** <0.01%
   - **Alert:** >1%

5. **Token Rotation Success Rate**
   ```promql
   rate(oauth_token_rotations_total{status="success"}[5m]) /
   rate(oauth_token_rotations_total[5m])
   ```
   - **Target:** >99.9%
   - **Alert:** <99%

6. **OAuth vs JWT Usage**
   ```promql
   rate(oauth_token_exchanges_total[5m]) /
   (rate(oauth_token_exchanges_total[5m]) + rate(jwt_deprecation_warnings_total[5m]))
   ```
   - **Target:** Increasing over time
   - **Goal:** >95% OAuth by Week 12

### Alert Rules

```yaml
groups:
  - name: oauth_production
    interval: 30s
    rules:
      - alert: OAuthHighErrorRate
        expr: |
          rate(oauth_token_exchanges_total{status!="success"}[5m]) /
          rate(oauth_token_exchanges_total[5m]) > 0.02
        for: 5m
        labels:
          severity: critical
        annotations:
          summary: "OAuth error rate above 2%"

      - alert: PKCEVerificationFailures
        expr: rate(oauth_pkce_verification_failures_total[5m]) > 10
        for: 5m
        labels:
          severity: warning
        annotations:
          summary: "High rate of PKCE verification failures"

      - alert: RefreshTokenReplayDetected
        expr: rate(oauth_refresh_replay_detections_total[5m]) > 1
        for: 1m
        labels:
          severity: critical
        annotations:
          summary: "Refresh token replay attack detected"
```

## Rollback Procedures

### Scenario 1: Critical OAuth Errors (Emergency Rollback)

**Trigger:** OAuth error rate >5% for >5 minutes

```bash
# 1. Immediately disable OAuth
OAUTH_ENABLED=false
OAUTH_ENABLE_AUTHORIZE=false
OAUTH_ENABLE_TOKEN=false
systemctl restart aurora-locus

# 2. Verify JWT still works
curl -H "Authorization: Bearer jwt_token" \
  https://pds.example.com/xrpc/com.atproto.server.describeServer

# 3. Notify team and users
# - Update status page
# - Alert support team
# - Investigate root cause

# 4. Review logs
journalctl -u aurora-locus --since "10 minutes ago" | grep -i oauth

# 5. If needed, rollback database
sqlite3 data/account.sqlite ".restore data/account.sqlite.backup"
```

### Scenario 2: Gradual Rollback (High Error Rate)

**Trigger:** OAuth error rate >2% for specific user segment

```bash
# Reduce rollout percentage
OAUTH_ROLLOUT_PERCENTAGE=10  # or previous stable percentage
systemctl restart aurora-locus

# Monitor for improvement
watch 'curl -s localhost:9090/metrics | grep oauth_token_exchanges'
```

### Scenario 3: DPoP Compatibility Issues

**Trigger:** DPoP verification failures >1%

```bash
# Disable DPoP requirement
OAUTH_REQUIRE_DPOP=false
systemctl restart aurora-locus

# Allow clients without DPoP to continue
# Investigate DPoP implementation issues
```

### Scenario 4: Token Rotation Issues

**Trigger:** Token rotation failures >1%

```bash
# Review token rotation manager logs
journalctl -u aurora-locus | grep -i "token_rotation"

# Check for database lock issues
sqlite3 data/account.sqlite "PRAGMA busy_timeout = 10000;"

# If persistent, disable token rotation temporarily
# (requires code change - fallback to non-rotating tokens)
```

## Post-Deployment Validation

### Day 1 After Full Rollout

- [ ] Review all monitoring dashboards
- [ ] Verify no critical alerts
- [ ] Check OAuth adoption rate
- [ ] Review user feedback and support tickets
- [ ] Confirm performance metrics within SLA

### Week 1 After Full Rollout

- [ ] Analyze OAuth vs JWT usage trends
- [ ] Review security events (replay attacks, PKCE failures)
- [ ] Performance tuning based on production load
- [ ] Documentation updates based on issues encountered

### Month 1 After Full Rollout

- [ ] Evaluate DPoP adoption rate
- [ ] Plan for JWT sunset (if not already executed)
- [ ] Review capacity planning for OAuth infrastructure
- [ ] Security audit of production OAuth implementation

## Troubleshooting

### Common Issues

#### Issue: High PKCE Verification Failures

**Symptoms:** `oauth_pkce_verification_failures_total` increasing

**Causes:**
1. Client using wrong code_verifier
2. Authorization code expired
3. Code_challenge method mismatch

**Resolution:**
```bash
# Check logs for specific error patterns
journalctl -u aurora-locus | grep -i "pkce verification failed"

# Verify client implementation
# Contact affected clients for code review
```

#### Issue: Token Rotation Failures

**Symptoms:** `oauth_token_rotations_total{status="failure"}` increasing

**Causes:**
1. Refresh token replay attack
2. Database lock contention
3. Expired refresh tokens

**Resolution:**
```bash
# Check for replay attacks
sqlite3 data/account.sqlite \
  "SELECT * FROM token WHERE id IN (
     SELECT id FROM refresh_token_used
   ) LIMIT 10;"

# Check database locks
sqlite3 data/account.sqlite "PRAGMA busy_timeout = 30000;"
```

#### Issue: DPoP Verification Failures

**Symptoms:** `oauth_dpop_verification_failures_total` increasing

**Causes:**
1. Invalid DPoP proof signature
2. Mismatched JWK thumbprint
3. Incorrect DPoP proof claims

**Resolution:**
```bash
# Review DPoP error logs
journalctl -u aurora-locus | grep -i "dpop verification failed"

# Verify client DPoP implementation
# Provide sample DPoP proofs for testing
```

## Success Criteria

- [ ] OAuth error rate <1%
- [ ] PKCE verification success rate >99%
- [ ] DPoP adoption rate >80% (if enforced)
- [ ] Token rotation success rate >99.9%
- [ ] No critical security incidents
- [ ] OAuth response time <100ms (p95)
- [ ] Zero data loss during migration
- [ ] User complaints <0.1%

## Contact Information

- **On-Call Engineer:** [Pagerduty rotation]
- **Database Admin:** [DBA contact]
- **Security Team:** [Security contact]
- **Product Manager:** [PM contact]

## Version History

| Version | Date | Author | Changes |
|---------|------|--------|---------|
| 1.0.0 | 2025-01-22 | Claude | Initial deployment procedures |

---

**Note:** This is a living document. Update based on lessons learned during deployment.
