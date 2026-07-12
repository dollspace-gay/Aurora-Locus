//! v0.10 Arc 1 Phase D (#414) — per-account did:web document serving.
//!
//! Serves `GET /user/{slug}/did.json` (SD-3 α path-form): a did:web account's
//! DID document, composed at request time from stored inputs (no on-disk
//! `did.json` files — mirrors the server-own doc at `well_known::generate_did_document`).
//!
//! The served document advertises the holder's stored `identity_public_key` as
//! the immutable `#atproto` verification method (LOCKED §5 field table), the
//! service endpoint composed from config (SD-2 β), and `alsoKnownAs` composed
//! from `actor.handle` (AD-2 β). The AD-1 serve-side gate returns 404 when the
//! account is deactivated or taken down. Minting (Phase C) gates on Arc 2; this
//! is the read/serve half.

use crate::context::AppContext;
use crate::identity::did_document::{build_did_document, DidDocument};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Json,
    routing::get,
    Router,
};

/// Per-account did:web serve routes. Merged in `api::routes` alongside
/// `well_known::routes()`. The `/user/` prefix keeps the path namespace
/// collision-free (LOCKED §5).
pub fn routes() -> Router<AppContext> {
    Router::new().route("/user/:slug/did.json", get(serve_did_web_document))
}

/// `GET /user/{slug}/did.json` — compose and serve a did:web account's DID
/// document (LOCKED §5 four-step resolution).
async fn serve_did_web_document(
    State(ctx): State<AppContext>,
    Path(slug): Path<String>,
) -> Result<Json<DidDocument>, StatusCode> {
    // 1. Reverse-lookup by slug.
    let account = match ctx.account_manager.get_did_web_account_by_slug(&slug).await {
        Ok(Some(a)) => a,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!(slug = %slug, error = %e, "did:web serve: slug lookup failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    // 2. AD-1 serve-side gate + the handle for alsoKnownAs, read from the actor
    //    table only (no account join — serving an identity must not depend on an
    //    account row). A did_web row with no actor row (shouldn't happen — the FK
    //    guarantees it) is treated as absent → 404.
    let actor = match ctx.account_manager.get_actor_serve_state(&account.did).await {
        Ok(Some(a)) => a,
        Ok(None) => return Err(StatusCode::NOT_FOUND),
        Err(e) => {
            tracing::error!(did = %account.did, error = %e, "did:web serve: actor lookup failed");
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
    };
    if actor.deactivated || actor.taken_down {
        return Err(StatusCode::NOT_FOUND);
    }

    // 3. Operator observability — also the Arc-1 reader for the slug/domain/
    //    created_at columns (the serve route is their consumer this phase).
    tracing::debug!(
        did = %account.did,
        slug = %account.slug,
        domain = %account.domain,
        created_at = %account.created_at,
        "did:web serve: composing document"
    );

    // 4. Compose: verificationMethod = the holder's stored public key (immutable
    //    identity); service = composed from config (SD-2 β); alsoKnownAs =
    //    composed from actor.handle (AD-2 β), omitted if the actor has no handle.
    let also_known_as = actor.handle.as_deref().map(|h| format!("at://{h}"));
    let doc = build_did_document(
        &account.did,
        &account.identity_public_key,
        ctx.service_url(),
        also_known_as.as_deref(),
    );
    Ok(Json(doc))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_ctx() -> AppContext {
        crate::api::federation_peers::test_support::create_test_context_with(|_| {}).await
    }

    async fn seed(ctx: &AppContext, did: &str, slug: &str, handle: &str) {
        sqlx::query("INSERT INTO actor (did, handle, created_at) VALUES ($1, $2, $3)")
            .bind(did)
            .bind(handle)
            .bind("2026-01-01T00:00:00Z")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO did_web_account (did, domain, slug, identity_public_key, created_at) \
             VALUES ($1, $2, $3, $4, $5)",
        )
        .bind(did)
        .bind("pds.example.com")
        .bind(slug)
        .bind("zHolderKey")
        .bind("2026-01-01T00:00:00Z")
        .execute(&ctx.account_db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn serves_active_did_web_account() {
        let ctx = test_ctx().await;
        seed(&ctx, "did:web:pds.example.com:user:alice", "alice", "alice.pds.example.com").await;
        let Json(doc) = serve_did_web_document(State(ctx.clone()), Path("alice".into()))
            .await
            .expect("active account serves a document");
        assert_eq!(doc.id, "did:web:pds.example.com:user:alice");
        assert_eq!(
            doc.get_signing_key().and_then(|v| v.public_key_multibase.as_deref()),
            Some("zHolderKey")
        );
        assert_eq!(doc.also_known_as, vec!["at://alice.pds.example.com".to_string()]);
        assert!(doc.get_service_endpoint().is_some());
    }

    #[tokio::test]
    async fn unknown_slug_404() {
        let ctx = test_ctx().await;
        let res = serve_did_web_document(State(ctx), Path("ghost".into())).await;
        assert_eq!(res.err(), Some(StatusCode::NOT_FOUND));
    }

    #[tokio::test]
    async fn deactivated_account_404() {
        let ctx = test_ctx().await;
        seed(&ctx, "did:web:pds.example.com:user:bob", "bob", "bob.pds.example.com").await;
        sqlx::query("UPDATE actor SET deactivated_at = $1 WHERE did = $2")
            .bind("2026-02-01T00:00:00Z")
            .bind("did:web:pds.example.com:user:bob")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let res = serve_did_web_document(State(ctx), Path("bob".into())).await;
        assert_eq!(res.err(), Some(StatusCode::NOT_FOUND));
    }

    #[tokio::test]
    async fn taken_down_account_404() {
        let ctx = test_ctx().await;
        seed(&ctx, "did:web:pds.example.com:user:carol", "carol", "carol.pds.example.com").await;
        sqlx::query("UPDATE actor SET takedown_ref = $1 WHERE did = $2")
            .bind("takedown-ref-1")
            .bind("did:web:pds.example.com:user:carol")
            .execute(&ctx.account_db)
            .await
            .unwrap();
        let res = serve_did_web_document(State(ctx), Path("carol".into())).await;
        assert_eq!(res.err(), Some(StatusCode::NOT_FOUND));
    }
}
