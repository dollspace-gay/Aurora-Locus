# Aurora Locus PDS - Scalability Guide

## Table of Contents

1. [Overview](#overview)
2. [S3-Compatible Blob Storage](#s3-compatible-blob-storage)
3. [PostgreSQL Database](#postgresql-database)
4. [Redis Caching Layer](#redis-caching-layer)
5. [Distributed Rate Limiting](#distributed-rate-limiting)
6. [Horizontal Scaling](#horizontal-scaling)
7. [Performance Tuning](#performance-tuning)
8. [Monitoring](#monitoring)
9. [Best Practices](#best-practices)

---

## Overview

Aurora Locus PDS includes multiple scalability options to support growing workloads and distributed deployments:

| Feature | Purpose | Default | Scalability Limit |
|---------|---------|---------|-------------------|
| SQLite | Development database | Enabled | Single node, ~10K users |
| PostgreSQL | Production database | Optional | Multi-node, millions of users |
| Disk Storage | Local blob storage | Enabled | Single node, limited by disk |
| S3 Storage | Distributed blob storage | Optional | Virtually unlimited |
| In-Memory Cache | Fast local caching | Enabled | Single node only |
| Redis Cache | Distributed caching | Optional | Multi-node, high performance |
| Local Rate Limiting | Per-instance limits | Enabled | Single node only |
| Distributed Rate Limiting | Shared rate limits | Optional | Multi-node deployments |

---

## S3-Compatible Blob Storage

### Overview

S3-compatible storage allows you to store blobs (images, videos, files) in distributed object storage instead of local disk. This enables:

- **Unlimited scalability**: No disk space limitations
- **High availability**: Built-in replication and durability
- **Cost efficiency**: Pay for what you use, cheaper than local SSDs at scale
- **Multi-region support**: Store data closer to users

### Supported Providers

- **AWS S3**: Original S3 service
- **MinIO**: Self-hosted S3-compatible storage
- **DigitalOcean Spaces**: Managed S3-compatible storage
- **Backblaze B2**: Cost-effective S3-compatible storage
- **Cloudflare R2**: Zero egress fees
- **Wasabi**: Hot cloud storage

### Configuration

#### Environment Variables

```bash
# S3 Configuration
PDS_BLOBSTORE_TYPE=s3
PDS_BLOBSTORE_S3_BUCKET=aurora-locus-blobs
PDS_BLOBSTORE_S3_REGION=us-east-1
PDS_BLOBSTORE_S3_ACCESS_KEY_ID=AKIAXXXXXXXXXXXXXXXX
PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY=your-secret-key-here

# For S3-compatible providers (MinIO, DO Spaces, etc.)
PDS_BLOBSTORE_S3_ENDPOINT=https://nyc3.digitaloceanspaces.com
```

#### AWS S3 Setup

```bash
# Create S3 bucket
aws s3 mb s3://aurora-locus-blobs --region us-east-1

# Create IAM user with S3 access
aws iam create-user --user-name aurora-locus-s3

# Attach policy (create policy.json first)
aws iam put-user-policy --user-name aurora-locus-s3 \
  --policy-name S3BlobAccess \
  --policy-document file://policy.json

# Create access keys
aws iam create-access-key --user-name aurora-locus-s3
```

**policy.json:**
```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "s3:PutObject",
        "s3:GetObject",
        "s3:DeleteObject",
        "s3:ListBucket"
      ],
      "Resource": [
        "arn:aws:s3:::aurora-locus-blobs",
        "arn:aws:s3:::aurora-locus-blobs/*"
      ]
    }
  ]
}
```

#### MinIO Setup (Self-Hosted)

```bash
# Run MinIO with Docker
docker run -d \
  -p 9000:9000 \
  -p 9001:9001 \
  --name minio \
  -e "MINIO_ROOT_USER=admin" \
  -e "MINIO_ROOT_PASSWORD=password" \
  -v /data/minio:/data \
  minio/minio server /data --console-address ":9001"

# Create bucket
mc alias set local http://localhost:9000 admin password
mc mb local/aurora-locus-blobs
```

**Environment variables for MinIO:**
```bash
PDS_BLOBSTORE_TYPE=s3
PDS_BLOBSTORE_S3_BUCKET=aurora-locus-blobs
PDS_BLOBSTORE_S3_REGION=us-east-1
PDS_BLOBSTORE_S3_ENDPOINT=http://localhost:9000
PDS_BLOBSTORE_S3_ACCESS_KEY_ID=admin
PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY=password
```

#### DigitalOcean Spaces Setup

```bash
# Create Space via DO console, then:
PDS_BLOBSTORE_TYPE=s3
PDS_BLOBSTORE_S3_BUCKET=aurora-locus
PDS_BLOBSTORE_S3_REGION=nyc3
PDS_BLOBSTORE_S3_ENDPOINT=https://nyc3.digitaloceanspaces.com
PDS_BLOBSTORE_S3_ACCESS_KEY_ID=DO00XXXXXXXXXXXXXX
PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY=your-secret-key
```

### Performance Considerations

- **Latency**: S3 adds ~10-50ms latency compared to local disk
- **Throughput**: S3 can handle thousands of concurrent requests
- **Cost**: Consider egress costs for high-traffic deployments
- **Caching**: Use CDN (CloudFront, Cloudflare) in front of S3

---

## PostgreSQL Database

### Overview

PostgreSQL provides a robust, scalable database backend for production deployments:

- **Concurrent connections**: Handle 1000+ simultaneous connections
- **ACID transactions**: Data integrity guarantees
- **Replication**: Master-slave replication for read scaling
- **High availability**: Automatic failover with pg_auto_failover
- **Advanced features**: Full-text search, JSON support, extensions

### When to Use PostgreSQL

| Scenario | SQLite | PostgreSQL |
|----------|--------|------------|
| Development | ✅ Recommended | ⚠️ Optional |
| Single-user PDS | ✅ Fine | ⚠️ Overkill |
| Multi-user PDS (<1K users) | ✅ Acceptable | ✅ Recommended |
| Multi-user PDS (>1K users) | ❌ Limited | ✅ Required |
| Distributed deployment | ❌ Not supported | ✅ Required |
| High concurrency | ❌ Lock contention | ✅ Excellent |

### Configuration

#### Environment Variables

```bash
# PostgreSQL Configuration
DATABASE_TYPE=postgres
DATABASE_URL=postgresql://user:password@localhost:5432/aurora_locus

# Connection pool settings
POSTGRES_MAX_CONNECTIONS=100
POSTGRES_MIN_CONNECTIONS=10
POSTGRES_CONNECT_TIMEOUT=30
POSTGRES_MAX_LIFETIME=1800
POSTGRES_IDLE_TIMEOUT=600
```

### Setup

#### Local Development

```bash
# Install PostgreSQL (Ubuntu/Debian)
sudo apt install postgresql postgresql-contrib

# Create database and user
sudo -u postgres psql
```

```sql
CREATE DATABASE aurora_locus;
CREATE USER aurora WITH PASSWORD 'secure_password';
GRANT ALL PRIVILEGES ON DATABASE aurora_locus TO aurora;
```

#### Docker

```bash
docker run -d \
  --name postgres \
  -e POSTGRES_DB=aurora_locus \
  -e POSTGRES_USER=aurora \
  -e POSTGRES_PASSWORD=secure_password \
  -p 5432:5432 \
  -v postgres_data:/var/lib/postgresql/data \
  postgres:16
```

#### Managed Services

**AWS RDS:**
```bash
aws rds create-db-instance \
  --db-instance-identifier aurora-locus-db \
  --db-instance-class db.t3.medium \
  --engine postgres \
  --engine-version 16.1 \
  --master-username aurora \
  --master-user-password secure_password \
  --allocated-storage 100 \
  --vpc-security-group-ids sg-xxxxx \
  --db-subnet-group-name my-subnet-group \
  --backup-retention-period 7
```

**DigitalOcean Managed Databases:**
- Create via console or API
- Copy connection string
- Update `DATABASE_URL` environment variable

### Migrations

Aurora Locus uses SQLx for migrations:

```bash
# Run migrations
cargo sqlx migrate run --database-url $DATABASE_URL

# Create new migration
cargo sqlx migrate add create_accounts_table

# Revert last migration
cargo sqlx migrate revert --database-url $DATABASE_URL
```

### Performance Tuning

**postgresql.conf optimizations:**

```conf
# Memory
shared_buffers = 2GB
effective_cache_size = 6GB
work_mem = 64MB
maintenance_work_mem = 512MB

# Connections
max_connections = 200

# Checkpoint
checkpoint_completion_target = 0.9
wal_buffers = 16MB

# Query planner
random_page_cost = 1.1  # For SSD
effective_io_concurrency = 200
```

### Replication

**Primary-Replica Setup:**

```bash
# On primary server
# Edit postgresql.conf
wal_level = replica
max_wal_senders = 10
wal_keep_size = 1GB

# Create replication user
CREATE USER replicator WITH REPLICATION ENCRYPTED PASSWORD 'rep_password';

# On replica server
pg_basebackup -h primary_host -D /var/lib/postgresql/14/main -U replicator -P -v -R

# Start replica
systemctl start postgresql
```

**Read-only replicas** can be used for:
- Analytics queries
- Backups
- Load balancing read-heavy endpoints

---

## Redis Caching Layer

### Overview

Redis provides distributed caching to improve performance and reduce database load:

- **Sub-millisecond latency**: 100x faster than database queries
- **Distributed**: Share cache across multiple PDS instances
- **Hit rate**: 80-95% cache hit rates typical
- **Reduced database load**: 70-90% fewer database queries

### Use Cases

| Data Type | TTL | Benefit |
|-----------|-----|---------|
| DID documents | 1 hour | Reduce PLC directory queries |
| Handle resolution | 30 min | Fast identity lookups |
| Session tokens | 10 min | Validate sessions without DB |
| Repository metadata | 5 min | Faster feed generation |
| Rate limit counters | 1 min | Distributed rate limiting |

### Configuration

```bash
# Redis Configuration
CACHE_ENABLED=true
REDIS_URL=redis://localhost:6379
CACHE_KEY_PREFIX=aurora:
CACHE_DEFAULT_TTL=300

# TTL settings (seconds)
CACHE_DID_DOC_TTL=3600    # 1 hour
CACHE_HANDLE_TTL=1800      # 30 minutes
CACHE_SESSION_TTL=600      # 10 minutes
```

### Setup

#### Local Development

```bash
# Install Redis (Ubuntu/Debian)
sudo apt install redis-server

# Start Redis
sudo systemctl start redis
sudo systemctl enable redis

# Test connection
redis-cli ping
# Should return: PONG
```

#### Docker

```bash
docker run -d \
  --name redis \
  -p 6379:6379 \
  -v redis_data:/data \
  redis:7-alpine \
  redis-server --appendonly yes
```

#### Managed Services

**AWS ElastiCache:**
```bash
aws elasticache create-cache-cluster \
  --cache-cluster-id aurora-locus-cache \
  --cache-node-type cache.t3.micro \
  --engine redis \
  --num-cache-nodes 1
```

**Redis Cloud:**
- Free tier: 30MB
- Pro tier: High availability, clustering

### Monitoring

```bash
# Redis CLI monitoring
redis-cli INFO stats
redis-cli INFO memory

# Key statistics
redis-cli DBSIZE
redis-cli --scan --pattern 'aurora:*' | wc -l

# Cache hit rate
redis-cli INFO stats | grep keyspace
```

### Cache Patterns

#### Cache-Aside (Lazy Loading)

```rust
// Try cache first
if let Some(did_doc) = cache.get("did:doc:", did).await? {
    return Ok(did_doc);
}

// On miss, fetch from source
let did_doc = fetch_from_plc_directory(did).await?;

// Store in cache
cache.set("did:doc:", did, &did_doc, Some(3600)).await?;

Ok(did_doc)
```

#### Write-Through

```rust
// Update database
db.update_handle(did, handle).await?;

// Invalidate cache
cache.delete("did:handle:", did).await?;
```

---

## Distributed Rate Limiting

### Overview

Distributed rate limiting shares rate limit state across multiple PDS instances using Redis:

- **Consistent limits**: Same limits regardless of which instance handles request
- **Accurate counting**: No per-instance loopholes
- **Flexible algorithms**: Fixed window, sliding window, token bucket

### Configuration

```bash
# Enable distributed rate limiting
RATE_LIMIT_DISTRIBUTED=true
REDIS_URL=redis://localhost:6379

# Rate limits (requests per minute)
RATE_LIMIT_AUTHENTICATED=6000
RATE_LIMIT_UNAUTHENTICATED=600
RATE_LIMIT_ADMIN=60000
```

### Algorithms

#### 1. Fixed Window Counter

**Pros:**
- Simple to implement
- Memory efficient
- Fast

**Cons:**
- Boundary issues (2x burst at window edges)

**Use case:** General API rate limiting

#### 2. Token Bucket

**Pros:**
- Allows bursts
- Smooth traffic
- Fair

**Cons:**
- More complex
- Slightly more memory

**Use case:** Upload endpoints, write-heavy APIs

#### 3. Sliding Window Log

**Pros:**
- Most accurate
- No boundary issues

**Cons:**
- Memory intensive
- Slower

**Use case:** Critical endpoints with strict limits

---

## Horizontal Scaling

### Architecture

```
                    ┌─────────────┐
                    │ Load Balancer │
                    │  (Nginx/HAProxy)│
                    └───────┬───────┘
                            │
              ┌─────────────┼─────────────┐
              │             │             │
         ┌────▼────┐   ┌────▼────┐   ┌────▼────┐
         │  PDS 1  │   │  PDS 2  │   │  PDS 3  │
         └────┬────┘   └────┬────┘   └────┬────┘
              │             │             │
              └─────────────┼─────────────┘
                            │
              ┌─────────────┼─────────────┐
              │             │             │
         ┌────▼────┐   ┌────▼────┐   ┌────▼────┐
         │PostgreSQL│   │  Redis  │   │   S3   │
         │(Primary) │   │ Cluster │   │ Bucket │
         └─────────┘   └─────────┘   └─────────┘
```

### Load Balancer Configuration

**Nginx:**

```nginx
upstream aurora_locus {
    least_conn;  # Route to least busy server

    server pds1.internal:2583 max_fails=3 fail_timeout=30s;
    server pds2.internal:2583 max_fails=3 fail_timeout=30s;
    server pds3.internal:2583 max_fails=3 fail_timeout=30s;
}

server {
    listen 443 ssl http2;
    server_name pds.example.com;

    ssl_certificate /etc/nginx/ssl/cert.pem;
    ssl_certificate_key /etc/nginx/ssl/key.pem;

    location / {
        proxy_pass http://aurora_locus;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;

        # WebSocket support
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
    }

    # Health check endpoint
    location /health/live {
        proxy_pass http://aurora_locus;
        access_log off;
    }
}
```

### Session Affinity

For features requiring sticky sessions:

```nginx
upstream aurora_locus {
    ip_hash;  # Route same IP to same server
    server pds1.internal:2583;
    server pds2.internal:2583;
    server pds3.internal:2583;
}
```

Or use Redis for session storage to avoid sticky sessions entirely.

---

## Performance Tuning

### Database Optimization

```sql
-- Add indexes for common queries
CREATE INDEX idx_accounts_handle ON accounts(handle);
CREATE INDEX idx_sessions_token ON sessions(access_token);
CREATE INDEX idx_repos_did ON repos(did);

-- Analyze query performance
EXPLAIN ANALYZE SELECT * FROM accounts WHERE handle = 'alice.bsky.social';

-- Vacuum regularly (PostgreSQL)
VACUUM ANALYZE;
```

### Caching Strategy

```rust
// Cache expensive operations
let repos = cache.get_or_set("repos:list", || async {
    db.list_repositories().await
}, 300).await?;

// Prefetch commonly accessed data
tokio::spawn(async move {
    for did in popular_dids {
        cache.set("did:doc:", did, &fetch_did_doc(did).await, 3600).await;
    }
});
```

### Connection Pooling

```bash
# PostgreSQL
POSTGRES_MAX_CONNECTIONS=100
POSTGRES_MIN_CONNECTIONS=20

# Redis
REDIS_POOL_SIZE=50
```

---

## Monitoring

### Metrics to Track

| Metric | Target | Alert |
|--------|--------|-------|
| Request latency (p95) | <100ms | >500ms |
| Database query time (p95) | <50ms | >200ms |
| Cache hit rate | >80% | <60% |
| Error rate | <0.1% | >1% |
| Active connections (DB) | <80% max | >90% max |
| Memory usage (Redis) | <80% | >90% |
| Blob storage latency | <100ms | >1s |

### Prometheus Metrics

Aurora Locus exposes metrics at `/metrics`:

```
# Database
aurora_db_queries_total{operation="SELECT",table="accounts"}
aurora_db_query_duration_seconds{operation="SELECT",table="accounts"}
aurora_db_connections{state="active"}

# Cache
aurora_cache_hits_total{category="did:doc"}
aurora_cache_misses_total{category="did:doc"}
aurora_cache_hit_rate{category="did:doc"}

# Blob storage
aurora_blob_uploads_total{backend="s3"}
aurora_blob_storage_bytes{backend="s3"}
```

---

## Best Practices

### 1. Start Simple, Scale Gradually

```
Development     → Production (Small) → Production (Large)
SQLite + Disk   → PostgreSQL + S3    → PG + Redis + S3 + Multi-node
```

### 2. Use CDN for Blobs

```
User Request → CDN (CloudFlare) → S3 → Aurora Locus
              └─ 95% cache hit      └─ 5% cache miss
```

### 3. Database Connection Limits

```bash
# Rule of thumb: (CPU cores * 2) + effective_spindle_count
# For 4-core server: 10-20 connections
# For 16-core server: 30-50 connections
```

### 4. Cache Warming

```bash
# Warm cache on startup
curl https://pds.example.com/xrpc/com.atproto.sync.listRepos
```

### 5. Graceful Degradation

```rust
// Fallback when cache is unavailable
let result = match cache.get("key").await {
    Ok(Some(value)) => value,
    Ok(None) | Err(_) => {
        warn!("Cache miss or error, falling back to DB");
        db.get("key").await?
    }
};
```

---

## Environment Variables Reference

```bash
# Database
DATABASE_TYPE=sqlite|postgres
DATABASE_URL=postgresql://user:pass@host:5432/db
POSTGRES_MAX_CONNECTIONS=100
POSTGRES_MIN_CONNECTIONS=10

# Blob Storage
PDS_BLOBSTORE_TYPE=disk|s3
PDS_BLOBSTORE_S3_BUCKET=my-bucket
PDS_BLOBSTORE_S3_REGION=us-east-1
PDS_BLOBSTORE_S3_ACCESS_KEY_ID=...
PDS_BLOBSTORE_S3_SECRET_ACCESS_KEY=...
PDS_BLOBSTORE_S3_ENDPOINT=https://...  # Optional, for S3-compatible

# Redis Cache
CACHE_ENABLED=true|false
REDIS_URL=redis://localhost:6379
CACHE_KEY_PREFIX=aurora:
CACHE_DEFAULT_TTL=300
CACHE_DID_DOC_TTL=3600
CACHE_HANDLE_TTL=1800
CACHE_SESSION_TTL=600

# Rate Limiting
RATE_LIMIT_DISTRIBUTED=true|false
RATE_LIMIT_AUTHENTICATED=6000
RATE_LIMIT_UNAUTHENTICATED=600
RATE_LIMIT_ADMIN=60000
```

---

## Troubleshooting

### S3 Connection Issues

```bash
# Test S3 connectivity
aws s3 ls s3://your-bucket --region us-east-1

# Check credentials
aws sts get-caller-identity

# Test with curl
curl -I https://your-bucket.s3.amazonaws.com/
```

### PostgreSQL Performance

```sql
-- Check slow queries
SELECT query, mean_exec_time, calls
FROM pg_stat_statements
ORDER BY mean_exec_time DESC
LIMIT 10;

-- Check connection pool
SELECT count(*), state
FROM pg_stat_activity
GROUP BY state;
```

### Redis Connection Issues

```bash
# Test connectivity
redis-cli -h host -p 6379 PING

# Check memory
redis-cli INFO memory

# Monitor commands
redis-cli MONITOR
```

---

For more information:
- [Deployment Guide](DEPLOYMENT.md)
- [Backup & Recovery](BACKUP_RECOVERY.md)
- [Monitoring Guide](MONITORING.md)
