//! The shared scratch store for rate-limit counters and replay windows:
//! Redis when the install has one (a bootstrap value, the same on every
//! node), else the database (replay windows) and in-process or database
//! counters (rate limits).

use locus_core::{CoreError, CoreResult};

/// The Redis connection, if the install has one.
#[derive(Clone, Default)]
pub struct Scratch {
    redis: Option<redis::aio::ConnectionManager>,
}

/// `PDS_REDIS_SCRATCH_ADDRESS` is a bare `host:port`; a URL is taken as is.
/// The reference's `PDS_REDIS_SCRATCH_PASSWORD` goes into a bare address.
#[must_use]
pub fn redis_url(configured: &str, password: Option<&str>) -> String {
    let c = configured.trim();
    if c.contains("://") {
        return c.to_owned();
    }
    match password.filter(|p| !p.is_empty()) {
        Some(p) => format!("redis://:{}@{c}", url_escape(p)),
        None => format!("redis://{c}"),
    }
}

fn url_escape(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

const CONSUME: &str = r"
local c = redis.call('INCRBY', KEYS[1], ARGV[1])
local ttl = redis.call('PTTL', KEYS[1])
if ttl < 0 then
  redis.call('PEXPIRE', KEYS[1], ARGV[2])
  ttl = tonumber(ARGV[2])
end
return {c, ttl}
";

/// Keys are namespaced so a shared Redis can hold other things.
const PREFIX: &str = "locus:";

impl Scratch {
    /// Connects (and pings) at boot; a node configured for Redis doesn't
    /// start without it.
    pub async fn connect(url: &str) -> CoreResult<Self> {
        let client =
            redis::Client::open(url).map_err(|e| CoreError::Invalid(format!("Redis URL: {e}")))?;
        let config = redis::aio::ConnectionManagerConfig::new()
            .set_number_of_retries(2)
            .set_connection_timeout(std::time::Duration::from_secs(5))
            .set_response_timeout(std::time::Duration::from_secs(5));
        let connect = async {
            let mut conn = client.get_connection_manager_with_config(config).await?;
            redis::cmd("PING").query_async::<String>(&mut conn).await?;
            Ok::<_, redis::RedisError>(conn)
        };
        let conn = tokio::time::timeout(std::time::Duration::from_secs(10), connect)
            .await
            .map_err(|_| CoreError::Storage("Redis: no answer within 10 seconds".into()))?
            .map_err(|e| CoreError::Storage(format!("Redis: {e}")))?;
        Ok(Self { redis: Some(conn) })
    }

    #[must_use]
    pub fn is_redis(&self) -> bool {
        self.redis.is_some()
    }

    /// Adds `points` to a fixed-window counter: (consumed, ms until the
    /// window ends). None without Redis.
    pub async fn consume(
        &self,
        key: &str,
        points: i64,
        window_ms: i64,
    ) -> Option<CoreResult<(i64, i64)>> {
        let mut conn = self.redis.clone()?;
        let r = redis::Script::new(CONSUME)
            .key(format!("{PREFIX}rl:{key}"))
            .arg(points)
            .arg(window_ms)
            .invoke_async::<(i64, i64)>(&mut conn)
            .await
            .map_err(|e| CoreError::Storage(format!("Redis: {e}")));
        Some(r)
    }

    /// Records `value` in `namespace` for `ttl_ms`; false if it was already
    /// there (a replay). None without Redis.
    pub async fn unique(
        &self,
        namespace: &str,
        value: &str,
        ttl_ms: i64,
    ) -> Option<CoreResult<bool>> {
        let mut conn = self.redis.clone()?;
        let r = redis::cmd("SET")
            .arg(format!("{PREFIX}replay:{namespace}:{value}"))
            .arg(1)
            .arg("NX")
            .arg("PX")
            .arg(ttl_ms.max(1))
            .query_async::<Option<String>>(&mut conn)
            .await
            .map(|set| set.is_some())
            .map_err(|e| CoreError::Storage(format!("Redis: {e}")));
        Some(r)
    }
}

impl crate::state::AppState {
    /// A replay window: in Redis when the install has one, else the
    /// database. Fails closed (an error, not an acceptance).
    pub async fn unique(&self, namespace: &str, value: &str, ttl_ms: i64) -> CoreResult<bool> {
        match self.kv.unique(namespace, value, ttl_ms).await {
            Some(r) => r,
            None => self.store.oauth_unique(namespace, value, ttl_ms).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::redis_url;

    #[test]
    fn bare_addresses_become_urls() {
        assert_eq!(
            redis_url("redis.internal:6379", None),
            "redis://redis.internal:6379"
        );
        assert_eq!(
            redis_url("10.0.0.5:6379", Some("p@ss word")),
            "redis://:p%40ss%20word@10.0.0.5:6379"
        );
        assert_eq!(
            redis_url("rediss://h:6380/1", Some("x")),
            "rediss://h:6380/1"
        );
    }
}
