//! Per-table dispatch over multiple `DistributedStore`
//! implementations (Arc 7 Step 2, chainlink #53).
//!
//! Step 1 introduced a single substrate
//! (`PostgresCasStore`) that handled `dpop_jti_replay` and
//! `rate_limit_buckets`. Step 2 adds `OAuthFlowStateAdapter`
//! for `oauth_flow_state`, which is structurally different —
//! it wraps the application's `account_db` pool rather than
//! the maintenance pool, because OAuth state lives in the
//! main schema, not the substrate's isolated maintenance
//! schema.
//!
//! `DistributedStoreRegistry` is the consumer-facing facade
//! that routes per-table operations to the correct impl. It
//! itself implements `DistributedStore`, so consumers stay
//! on `Arc<dyn DistributedStore>` and don't know there's a
//! layer of dispatch underneath. Future backends (e.g., a
//! Redis substrate, a sharded substrate) plug in by adding
//! fields to the registry without breaking consumer code.
//!
//! Unknown-table dispatch falls through to the substrate
//! field, which returns `DistributedError::UnsupportedTable`.
//! This keeps the error surface uniform with Step 1's
//! loud-failure model — operators see one error variant
//! regardless of which impl ultimately rejected the call.

use std::sync::Arc;

use async_trait::async_trait;

use super::{CasResult, DistributedError, DistributedStore, Lease};

/// Per-table router over multiple `DistributedStore` impls.
///
/// Constructed at startup (`AppContext::new`) once both the
/// substrate and the OAuth adapter exist. Cloned cheaply (it's
/// just two `Arc`s) and shared across the application via
/// `AppContext.distributed_store`.
pub struct DistributedStoreRegistry {
    /// Substrate impl. Handles `dpop_jti_replay` and
    /// `rate_limit_buckets` in Step 1's wiring; future
    /// substrate-owned tables also route here. Optional
    /// because `DistributedStateMode::SingleInstanceInmemory`
    /// skips the substrate entirely.
    substrate: Option<Arc<dyn DistributedStore>>,
    /// OAuth-domain adapter. Handles `oauth_flow_state`. Not
    /// `Option` because the underlying `authorization_request`
    /// table exists in every Aurora-Locus deployment (it lives
    /// in `account_db`, not the substrate maintenance pool);
    /// OAuth state needs cross-instance coherence even in
    /// `SingleInstanceInmemory` mode where the substrate is
    /// absent.
    oauth_adapter: Arc<dyn DistributedStore>,
}

impl DistributedStoreRegistry {
    pub fn new(
        substrate: Option<Arc<dyn DistributedStore>>,
        oauth_adapter: Arc<dyn DistributedStore>,
    ) -> Self {
        Self {
            substrate,
            oauth_adapter,
        }
    }

    /// Dispatch table-name → impl. Returns the chosen impl as
    /// `Result` so unknown tables in
    /// `SingleInstanceInmemory` mode (where substrate is
    /// `None`) get the same `UnsupportedTable` error as
    /// unknown tables in any other mode — the failure mode
    /// is uniform.
    fn dispatch(
        &self,
        table: &str,
    ) -> Result<&Arc<dyn DistributedStore>, DistributedError> {
        match table {
            "oauth_flow_state" => Ok(&self.oauth_adapter),
            "dpop_jti_replay" | "rate_limit_buckets" => self
                .substrate
                .as_ref()
                .ok_or_else(|| DistributedError::UnsupportedTable(table.to_string())),
            other => {
                // Unknown table. If a substrate is present, defer
                // to it so the loud-failure error mentions
                // "substrate doesn't know this table"; otherwise
                // return UnsupportedTable directly. Either way the
                // caller sees a consistent error variant.
                if let Some(substrate) = self.substrate.as_ref() {
                    Ok(substrate)
                } else {
                    Err(DistributedError::UnsupportedTable(other.to_string()))
                }
            }
        }
    }
}

#[async_trait]
impl DistributedStore for DistributedStoreRegistry {
    async fn insert(
        &self,
        table: &str,
        key: &str,
        value: &[u8],
        lease: Option<Lease>,
    ) -> Result<(), DistributedError> {
        self.dispatch(table)?.insert(table, key, value, lease).await
    }

    async fn get(
        &self,
        table: &str,
        key: &str,
    ) -> Result<Option<Vec<u8>>, DistributedError> {
        self.dispatch(table)?.get(table, key).await
    }

    async fn delete(&self, table: &str, key: &str) -> Result<bool, DistributedError> {
        self.dispatch(table)?.delete(table, key).await
    }

    async fn cas(
        &self,
        table: &str,
        key: &str,
        expected_version: i64,
        new_value: &[u8],
    ) -> Result<CasResult, DistributedError> {
        self.dispatch(table)?
            .cas(table, key, expected_version, new_value)
            .await
    }

    async fn reap_expired(
        &self,
        table: &str,
        now_epoch_ms: i64,
    ) -> Result<usize, DistributedError> {
        self.dispatch(table)?.reap_expired(table, now_epoch_ms).await
    }
}

#[cfg(test)]
mod tests {
    //! Registry-level dispatch verification using a recording
    //! mock impl. No real DB or testcontainer needed — the
    //! tests only care that the registry routes the right
    //! table-name to the right impl.
    use super::*;
    use std::sync::Mutex;

    /// Recording mock: implements `DistributedStore` and
    /// keeps a log of every call's `(method, table)` pair.
    /// All operations succeed with trivial return values; the
    /// tests inspect the call log to verify routing.
    #[derive(Default)]
    struct RecordingStore {
        name: &'static str,
        calls: Mutex<Vec<(&'static str, String)>>,
    }

    impl RecordingStore {
        fn new(name: &'static str) -> Arc<Self> {
            Arc::new(Self {
                name,
                calls: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> Vec<(&'static str, String)> {
            self.calls.lock().unwrap().clone()
        }
        fn record(&self, method: &'static str, table: &str) {
            self.calls
                .lock()
                .unwrap()
                .push((method, table.to_string()));
        }
    }

    #[async_trait]
    impl DistributedStore for RecordingStore {
        async fn insert(
            &self,
            table: &str,
            _key: &str,
            _value: &[u8],
            _lease: Option<Lease>,
        ) -> Result<(), DistributedError> {
            self.record("insert", table);
            Ok(())
        }
        async fn get(
            &self,
            table: &str,
            _key: &str,
        ) -> Result<Option<Vec<u8>>, DistributedError> {
            self.record("get", table);
            Ok(Some(format!("from-{}", self.name).into_bytes()))
        }
        async fn delete(&self, table: &str, _key: &str) -> Result<bool, DistributedError> {
            self.record("delete", table);
            Ok(true)
        }
        async fn cas(
            &self,
            table: &str,
            _key: &str,
            _expected_version: i64,
            _new_value: &[u8],
        ) -> Result<CasResult, DistributedError> {
            self.record("cas", table);
            Ok(CasResult::Success { new_version: 1 })
        }
        async fn reap_expired(
            &self,
            table: &str,
            _now_epoch_ms: i64,
        ) -> Result<usize, DistributedError> {
            self.record("reap_expired", table);
            Ok(0)
        }
    }

    fn registry_with_both() -> (Arc<RecordingStore>, Arc<RecordingStore>, DistributedStoreRegistry) {
        let substrate = RecordingStore::new("substrate");
        let oauth = RecordingStore::new("oauth");
        let registry = DistributedStoreRegistry::new(
            Some(Arc::clone(&substrate) as Arc<dyn DistributedStore>),
            Arc::clone(&oauth) as Arc<dyn DistributedStore>,
        );
        (substrate, oauth, registry)
    }

    #[tokio::test]
    async fn oauth_flow_state_routes_to_oauth_adapter() {
        let (substrate, oauth, registry) = registry_with_both();
        registry
            .insert("oauth_flow_state", "k", b"{}", None)
            .await
            .unwrap();
        registry
            .get("oauth_flow_state", "k")
            .await
            .unwrap();
        registry
            .delete("oauth_flow_state", "k")
            .await
            .unwrap();
        registry
            .reap_expired("oauth_flow_state", 0)
            .await
            .unwrap();

        assert_eq!(
            oauth.calls(),
            vec![
                ("insert", "oauth_flow_state".to_string()),
                ("get", "oauth_flow_state".to_string()),
                ("delete", "oauth_flow_state".to_string()),
                ("reap_expired", "oauth_flow_state".to_string()),
            ]
        );
        assert!(
            substrate.calls().is_empty(),
            "substrate must not be touched for oauth_flow_state, got {:?}",
            substrate.calls()
        );
    }

    #[tokio::test]
    async fn substrate_tables_route_to_substrate() {
        let (substrate, oauth, registry) = registry_with_both();
        registry
            .insert("dpop_jti_replay", "k", b"{}", None)
            .await
            .unwrap();
        registry
            .cas("rate_limit_buckets", "k", 0, b"{}")
            .await
            .unwrap();
        registry
            .reap_expired("dpop_jti_replay", 0)
            .await
            .unwrap();

        assert_eq!(
            substrate.calls(),
            vec![
                ("insert", "dpop_jti_replay".to_string()),
                ("cas", "rate_limit_buckets".to_string()),
                ("reap_expired", "dpop_jti_replay".to_string()),
            ]
        );
        assert!(oauth.calls().is_empty());
    }

    #[tokio::test]
    async fn unknown_table_routes_to_substrate_when_present() {
        let (substrate, oauth, registry) = registry_with_both();
        registry
            .insert("totally_unknown_table", "k", b"{}", None)
            .await
            .unwrap();
        // The substrate's RecordingStore is a happy-path mock
        // so the call records as success; the real substrate
        // would return UnsupportedTable. The point of this
        // test is the routing decision, not the
        // return-value content.
        assert_eq!(
            substrate.calls(),
            vec![("insert", "totally_unknown_table".to_string())]
        );
        assert!(oauth.calls().is_empty());
    }

    #[tokio::test]
    async fn unknown_table_without_substrate_returns_unsupported_table() {
        let oauth = RecordingStore::new("oauth");
        let registry = DistributedStoreRegistry::new(
            None,
            Arc::clone(&oauth) as Arc<dyn DistributedStore>,
        );
        let err = registry
            .insert("totally_unknown_table", "k", b"{}", None)
            .await
            .expect_err("no substrate → unknown table fails");
        assert!(matches!(err, DistributedError::UnsupportedTable(_)));
        assert!(
            oauth.calls().is_empty(),
            "oauth adapter must not receive unknown-table traffic"
        );
    }

    #[tokio::test]
    async fn substrate_tables_without_substrate_return_unsupported_table() {
        let oauth = RecordingStore::new("oauth");
        let registry = DistributedStoreRegistry::new(
            None,
            Arc::clone(&oauth) as Arc<dyn DistributedStore>,
        );
        for table in ["dpop_jti_replay", "rate_limit_buckets"] {
            let err = registry
                .insert(table, "k", b"{}", None)
                .await
                .expect_err("substrate-table-without-substrate fails");
            assert!(
                matches!(err, DistributedError::UnsupportedTable(_)),
                "expected UnsupportedTable for {}, got {:?}",
                table,
                err
            );
        }
        assert!(oauth.calls().is_empty());
    }

    #[tokio::test]
    async fn oauth_table_works_without_substrate() {
        // In SingleInstanceInmemory mode the substrate is
        // None but the OAuth adapter is still constructed —
        // the authorization_request table lives in account_db
        // regardless. This invariant is load-bearing for
        // single-instance deployments.
        let oauth = RecordingStore::new("oauth");
        let registry = DistributedStoreRegistry::new(
            None,
            Arc::clone(&oauth) as Arc<dyn DistributedStore>,
        );
        registry
            .insert("oauth_flow_state", "k", b"{}", None)
            .await
            .unwrap();
        registry
            .get("oauth_flow_state", "k")
            .await
            .unwrap();
        assert_eq!(
            oauth.calls(),
            vec![
                ("insert", "oauth_flow_state".to_string()),
                ("get", "oauth_flow_state".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn registry_passes_through_return_values() {
        // The registry must not transform return values —
        // consumers see exactly what the underlying impl
        // returned. Here the mock returns a tagged byte
        // payload and the test verifies the tag flows
        // through.
        let (_substrate, _oauth, registry) = registry_with_both();
        let bytes = registry
            .get("oauth_flow_state", "k")
            .await
            .unwrap()
            .expect("RecordingStore.get returns Some");
        assert_eq!(bytes, b"from-oauth");
    }
}
