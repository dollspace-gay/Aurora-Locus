//! Arc 17 §17.4 Step 1.5 — `DnsTxtResolver` trait wrapping `hickory-resolver`.
//!
//! Provides a single async TXT-lookup seam so Phase B (and unit tests) can
//! inject a deterministic mock without depending on real DNS. Production uses
//! `HickoryDnsTxtResolver`; tests use `MockDnsTxtResolver`.
//!
//! Step 0.0g rationale: hickory-resolver's only abstraction point is
//! `ConnectionProvider`, which is generic over runtime/connection types and is
//! awkward to mock cleanly. The thin trait below exposes exactly the surface
//! Arc 17's lexicon resolver needs (one TXT lookup, returning the raw record
//! strings), and nothing more.
//!
//! Multi-TXT-record / multi-`did=` strict-parse posture lives in
//! `lexicon_resolver.rs` per round-1 F5 closure; this trait is the
//! transport-level seam, not the policy layer.

use async_trait::async_trait;
use hickory_resolver::proto::rr::{RData, RecordType};
use hickory_resolver::TokioResolver;
use std::sync::Arc;

/// Errors surfaced by `DnsTxtResolver` impls. Coarse-grained on purpose —
/// the resolver's caller (`LexiconResolver`) classifies these into
/// `failure_class` taxonomy values (`"dns_fail"`, `"timeout"`, etc.) for
/// the `lexicon_fetch_failed` forensic log.
#[derive(Debug, thiserror::Error)]
pub enum DnsResolverError {
    /// Name has no `_lexicon.<host>` TXT records, or upstream DNS returned
    /// NXDOMAIN / NODATA. Distinct from `Transport` because it's a definitive
    /// answer (the name has no relevant records), not a connectivity failure.
    #[error("no TXT records for {0}")]
    NoRecords(String),

    /// Upstream DNS lookup itself failed (timeout, SERVFAIL, no reachable
    /// resolver). Caller classifies as `failure_class = "dns_fail"` or
    /// `"timeout"`.
    #[error("DNS transport error for {name}: {source_detail}")]
    Transport { name: String, source_detail: String },
}

/// Wrapper trait around hickory-resolver. The single method returns each
/// TXT record's full character-data joined as a single string (matching the
/// bsky-PDS `chunks.join('')` posture at the Step 0.0a reference SHA).
#[async_trait]
pub trait DnsTxtResolver: Send + Sync {
    /// Look up TXT records for `name` (e.g. `_lexicon.bsky.app`) and return
    /// each record as a single joined-chunks string. Order is whatever the
    /// resolver returns; callers MUST NOT depend on stable ordering.
    async fn resolve_txt(&self, name: &str) -> Result<Vec<String>, DnsResolverError>;
}

/// Production impl backed by hickory-resolver's `TokioResolver`. Constructed
/// once at startup and shared via `Arc`.
pub struct HickoryDnsTxtResolver {
    inner: Arc<TokioResolver>,
}

impl HickoryDnsTxtResolver {
    /// Build a resolver from the system DNS configuration (`/etc/resolv.conf`
    /// on Unix, registry on Windows — `hickory-resolver`'s `system-config`
    /// feature handles both, on by default).
    ///
    /// ## RUSTSEC-2026-0118 (NSEC3 unbounded-loop) — accept-disposition
    ///
    /// The advisory's vulnerable code path is `verify_nsec3` in
    /// `hickory-proto::dnssec::dnssec_dns_handle::nsec3_validation`. That
    /// function is only callable through `DnssecDnsHandle`, which the
    /// resolver builder wires in only when BOTH of the following hold:
    ///
    /// 1. The `__dnssec` feature is active on `hickory-resolver` — we do
    ///    NOT enable it (`Cargo.toml` declares `hickory-resolver = "0.26.1"`
    ///    with default features only; no `dnssec-ring` / `dnssec-aws-lc-rs`
    ///    activation).
    /// 2. `ResolverOpts.validate` is `true` — the `Default` impl sets it to
    ///    `false`, and `read_system_conf`'s `parse_resolv_conf` never
    ///    writes the field.
    ///
    /// Under `builder_tokio().build()` the resulting handle is
    /// unconditionally `LookupEither::Retry`, never `Secure(DnssecDnsHandle)`.
    /// NSEC3 validation is unreachable; the High advisory is therefore an
    /// accepted exposure rather than an active risk in this build. See
    /// `SECURITY.md` for the full reasoning + upstream-tracking note.
    ///
    /// **Reassessment trigger:** any future change that enables DNSSEC
    /// validation (adds the `dnssec-ring` feature on the hickory pin, or
    /// flips `options.validate = true` on this builder) re-exposes the
    /// vulnerable path and must reopen this disposition.
    pub fn from_system() -> Result<Self, DnsResolverError> {
        let builder = TokioResolver::builder_tokio().map_err(|e| {
            DnsResolverError::Transport {
                name: "<system-config>".to_string(),
                source_detail: format!("hickory builder: {e}"),
            }
        })?;
        // hickory-resolver 0.26: `.build()` returns `Result<Resolver<P>, NetError>`
        // (was infallible in 0.25). Map the NetError into our Transport variant
        // so callers don't have to know about hickory's internal error type.
        let resolver = builder.build().map_err(|e| DnsResolverError::Transport {
            name: "<system-config>".to_string(),
            source_detail: format!("hickory build: {e}"),
        })?;
        Ok(Self {
            inner: Arc::new(resolver),
        })
    }
}

#[async_trait]
impl DnsTxtResolver for HickoryDnsTxtResolver {
    async fn resolve_txt(&self, name: &str) -> Result<Vec<String>, DnsResolverError> {
        let lookup = self.inner.txt_lookup(name).await.map_err(|e| {
            if e.is_no_records_found() || e.is_nx_domain() {
                DnsResolverError::NoRecords(name.to_string())
            } else {
                DnsResolverError::Transport {
                    name: name.to_string(),
                    source_detail: e.to_string(),
                }
            }
        })?;

        // Per Step 0.0a reference posture (bsky-PDS `chunks.join('')`): join
        // each TXT record's character-data chunks into one string. Multiple
        // TXT records remain as separate Vec entries — the caller decides
        // policy (strict-fail-on-multiple vs accept-first-etc).
        //
        // hickory-resolver 0.26 changed the iteration model: `txt_lookup`
        // returns the generic `Lookup` (was the specialized `TxtLookup`),
        // which exposes `.answers() -> &[Record]` instead of an
        // `iter()`-yielding-`&TXT` shape. The semantic is preserved:
        // (a) filter answers to RecordType::TXT (the message can carry
        //     auxiliary records — CNAME for the alias, signatures, etc.,
        //     even though our `txt_lookup` is type-narrow);
        // (b) match `record.data()` on `RData::TXT` to access the chunk
        //     vector (`TXT::txt_data: Box<[Box<[u8]>]>`);
        // (c) join the chunks with empty separator, lossy-utf8 per chunk
        //     (`from_utf8_lossy`), matching the 0.25 behaviour exactly.
        let mut out = Vec::new();
        for record in lookup.answers() {
            if record.record_type() != RecordType::TXT {
                continue;
            }
            if let RData::TXT(txt) = &record.data {
                let joined: String = txt
                    .txt_data
                    .iter()
                    .map(|chunk| String::from_utf8_lossy(chunk).into_owned())
                    .collect::<Vec<_>>()
                    .join("");
                out.push(joined);
            }
        }

        if out.is_empty() {
            return Err(DnsResolverError::NoRecords(name.to_string()));
        }
        Ok(out)
    }
}

/// Deterministic test impl. Construct with a `HashMap<name, result>`; lookups
/// return the configured value verbatim (or a configurable default error).
#[cfg(test)]
pub struct MockDnsTxtResolver {
    responses: std::collections::HashMap<String, Result<Vec<String>, DnsResolverError>>,
}

#[cfg(test)]
impl Default for MockDnsTxtResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl MockDnsTxtResolver {
    pub fn new() -> Self {
        Self {
            responses: std::collections::HashMap::new(),
        }
    }

    pub fn with_txt(mut self, name: &str, records: Vec<String>) -> Self {
        self.responses.insert(name.to_string(), Ok(records));
        self
    }

    pub fn with_error(mut self, name: &str, err: DnsResolverError) -> Self {
        self.responses.insert(name.to_string(), Err(err));
        self
    }
}

#[cfg(test)]
#[async_trait]
impl DnsTxtResolver for MockDnsTxtResolver {
    async fn resolve_txt(&self, name: &str) -> Result<Vec<String>, DnsResolverError> {
        match self.responses.get(name) {
            Some(Ok(records)) => Ok(records.clone()),
            Some(Err(DnsResolverError::NoRecords(_))) => {
                Err(DnsResolverError::NoRecords(name.to_string()))
            }
            Some(Err(DnsResolverError::Transport { source_detail, .. })) => {
                Err(DnsResolverError::Transport {
                    name: name.to_string(),
                    source_detail: source_detail.clone(),
                })
            }
            None => Err(DnsResolverError::NoRecords(name.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_returns_configured_records() {
        let m = MockDnsTxtResolver::new()
            .with_txt("_lexicon.bsky.app", vec!["did=did:plc:abc".to_string()]);
        let out = m.resolve_txt("_lexicon.bsky.app").await.unwrap();
        assert_eq!(out, vec!["did=did:plc:abc".to_string()]);
    }

    #[tokio::test]
    async fn mock_returns_no_records_for_unconfigured_name() {
        let m = MockDnsTxtResolver::new();
        let err = m.resolve_txt("_lexicon.unknown.example").await.unwrap_err();
        assert!(matches!(err, DnsResolverError::NoRecords(_)));
    }

    #[tokio::test]
    async fn mock_returns_configured_transport_error() {
        let m = MockDnsTxtResolver::new().with_error(
            "_lexicon.broken.example",
            DnsResolverError::Transport {
                name: "_lexicon.broken.example".to_string(),
                source_detail: "simulated SERVFAIL".to_string(),
            },
        );
        let err = m.resolve_txt("_lexicon.broken.example").await.unwrap_err();
        assert!(matches!(err, DnsResolverError::Transport { .. }));
    }

    #[tokio::test]
    async fn mock_returns_multiple_records_for_ambiguity_test() {
        let m = MockDnsTxtResolver::new().with_txt(
            "_lexicon.ambiguous.example",
            vec![
                "did=did:plc:one".to_string(),
                "did=did:plc:two".to_string(),
            ],
        );
        let out = m.resolve_txt("_lexicon.ambiguous.example").await.unwrap();
        assert_eq!(out.len(), 2);
    }
}
