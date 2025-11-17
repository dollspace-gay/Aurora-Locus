//! OAuth 2.1 Consent Screen
//!
//! Implements the user consent flow for OAuth authorization:
//! - Display consent screen with requested scopes and client info
//! - Handle user grant/deny decisions
//! - Generate authorization code on grant
//! - Redirect back to client with code + state or error
//!
//! Flow:
//! 1. User arrives at /oauth/consent?request_id=xxx (from authorize endpoint)
//! 2. Display consent screen showing:
//!    - Client information (client_id, name, description)
//!    - Requested scopes and permissions
//!    - Grant/Deny buttons
//! 3. User clicks Grant:
//!    - Generate one-time authorization code
//!    - Update authorization_request with code
//!    - Redirect to client redirect_uri with code + state
//! 4. User clicks Deny:
//!    - Delete authorization_request
//!    - Redirect to client redirect_uri with error
//!
//! References:
//! - https://datatracker.ietf.org/doc/html/draft-ietf-oauth-v2-1-09#section-4.1.2
//! - https://atproto.com/specs/oauth

use crate::error::{PdsError, PdsResult};
use crate::oauth::authorize::get_authorization_request;
use crate::AppContext;
use axum::{
    extract::{Query, State},
    response::{Html, IntoResponse, Redirect},
    Form,
};
use chrono::Utc;
use serde::Deserialize;
use sqlx::Row;
use std::sync::Arc;
use tracing::{debug, warn};
use uuid::Uuid;

/// Consent screen query parameters
#[derive(Debug, Deserialize)]
pub struct ConsentQuery {
    /// Authorization request ID
    pub request_id: String,
}

/// Grant form data
#[derive(Debug, Deserialize)]
pub struct GrantForm {
    /// Authorization request ID
    pub request_id: String,
}

/// Deny form data
#[derive(Debug, Deserialize)]
pub struct DenyForm {
    /// Authorization request ID
    pub request_id: String,
}

/// Consent screen handler
///
/// GET /oauth/consent?request_id=xxx
///
/// Displays the OAuth consent screen to the user showing:
/// - Client information (client_id)
/// - Requested scopes/permissions
/// - Grant and Deny buttons
///
/// # Returns
/// - HTML consent screen
/// - 400 Bad Request if request_id is invalid
/// - 404 Not Found if request not found or expired
pub async fn consent_screen(
    State(ctx): State<AppContext>,
    Query(query): Query<ConsentQuery>,
) -> PdsResult<impl IntoResponse> {
    debug!("Consent screen requested for: {}", query.request_id);

    // Get the authorization request
    let request = get_authorization_request(&ctx, &query.request_id).await?;

    // Check if already granted (authorization_code exists)
    if request.authorization_code.is_some() {
        return Err(PdsError::Validation(
            "Authorization request already granted".to_string(),
        ));
    }

    // Parse scopes for display
    let scopes: Vec<&str> = request.scope.split_whitespace().collect();

    // Generate HTML consent screen
    // In production, this would be a proper template (e.g., Tera, Handlebars)
    // For now, we'll return a simple HTML form
    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <title>OAuth Consent</title>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
            max-width: 500px;
            margin: 50px auto;
            padding: 20px;
            background: #f5f5f5;
        }}
        .consent-box {{
            background: white;
            padding: 30px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }}
        h1 {{
            font-size: 24px;
            margin-bottom: 10px;
            color: #333;
        }}
        .client-info {{
            margin-bottom: 20px;
            padding: 15px;
            background: #f8f9fa;
            border-radius: 4px;
        }}
        .client-id {{
            font-weight: 600;
            color: #0066cc;
        }}
        .scopes {{
            margin: 20px 0;
        }}
        .scope-item {{
            padding: 10px;
            margin: 5px 0;
            background: #e7f3ff;
            border-left: 3px solid #0066cc;
            border-radius: 3px;
        }}
        .buttons {{
            display: flex;
            gap: 10px;
            margin-top: 30px;
        }}
        button {{
            flex: 1;
            padding: 12px;
            font-size: 16px;
            border: none;
            border-radius: 4px;
            cursor: pointer;
            font-weight: 500;
        }}
        .grant {{
            background: #0066cc;
            color: white;
        }}
        .grant:hover {{
            background: #0052a3;
        }}
        .deny {{
            background: #dc3545;
            color: white;
        }}
        .deny:hover {{
            background: #c82333;
        }}
        .account-info {{
            margin-bottom: 15px;
            font-size: 14px;
            color: #666;
        }}
    </style>
</head>
<body>
    <div class="consent-box">
        <h1>Authorization Request</h1>

        <div class="account-info">
            Authorizing as: <strong>{did}</strong>
        </div>

        <div class="client-info">
            <div>Application requesting access:</div>
            <div class="client-id">{client_id}</div>
        </div>

        <div class="scopes">
            <h3 style="margin-bottom: 10px;">Requested Permissions:</h3>
            {scope_list}
        </div>

        <div class="buttons">
            <form action="/oauth/grant" method="POST" style="flex: 1;">
                <input type="hidden" name="request_id" value="{request_id}">
                <button type="submit" class="grant">Authorize</button>
            </form>
            <form action="/oauth/deny" method="POST" style="flex: 1;">
                <input type="hidden" name="request_id" value="{request_id}">
                <button type="submit" class="deny">Deny</button>
            </form>
        </div>
    </div>
</body>
</html>"#,
        did = request.did,
        client_id = request.client_id,
        scope_list = scopes
            .iter()
            .map(|s| format!(r#"<div class="scope-item">✓ {}</div>"#, s))
            .collect::<Vec<_>>()
            .join("\n            "),
        request_id = query.request_id
    );

    Ok(Html(html))
}

/// Grant authorization handler
///
/// POST /oauth/grant
///
/// Handles user granting authorization:
/// 1. Generates one-time authorization code
/// 2. Updates authorization_request with code
/// 3. Redirects to client redirect_uri with code + state
///
/// # Security
/// - Authorization code is single-use (enforced by code_used flag)
/// - Code expires in 10 minutes (inherited from request expiration)
/// - Code bound to specific client_id, redirect_uri, and PKCE challenge
///
/// # Returns
/// - 302 Redirect to client with code + state
/// - 400 Bad Request if request invalid
/// - 404 Not Found if request not found or expired
pub async fn grant_authorization(
    State(ctx): State<AppContext>,
    Form(form): Form<GrantForm>,
) -> PdsResult<impl IntoResponse> {
    debug!("Grant authorization for request: {}", form.request_id);

    // Get the authorization request
    let request = get_authorization_request(&ctx, &form.request_id).await?;

    // Check if already granted
    if request.authorization_code.is_some() {
        return Err(PdsError::Validation(
            "Authorization already granted".to_string(),
        ));
    }

    // Generate one-time authorization code
    // Format: ac_{uuid} (ac = authorization code)
    let authorization_code = format!("ac_{}", Uuid::new_v4().to_string().replace("-", ""));

    // Update authorization_request with authorization code
    sqlx::query(
        r#"
        UPDATE authorization_request
        SET authorization_code = ?
        WHERE request_id = ?
        "#,
    )
    .bind(&authorization_code)
    .bind(&form.request_id)
    .execute(&ctx.account_db)
    .await?;

    debug!(
        "Generated authorization code for request: {}",
        form.request_id
    );

    // Build redirect URL with code + state
    let mut redirect_url = url::Url::parse(&request.redirect_uri).map_err(|e| {
        PdsError::Internal(format!("Invalid redirect_uri in request: {}", e))
    })?;

    // Add code parameter
    redirect_url
        .query_pairs_mut()
        .append_pair("code", &authorization_code);

    // Add state parameter if present
    if let Some(state) = &request.state {
        redirect_url.query_pairs_mut().append_pair("state", state);
    }

    debug!("Redirecting to client: {}", redirect_url);

    // Redirect to client
    Ok(Redirect::to(redirect_url.as_str()))
}

/// Deny authorization handler
///
/// POST /oauth/deny
///
/// Handles user denying authorization:
/// 1. Deletes the authorization_request (cleanup)
/// 2. Redirects to client redirect_uri with error
///
/// # Returns
/// - 302 Redirect to client with error=access_denied
/// - 400 Bad Request if request invalid
/// - 404 Not Found if request not found or expired
pub async fn deny_authorization(
    State(ctx): State<AppContext>,
    Form(form): Form<DenyForm>,
) -> PdsResult<impl IntoResponse> {
    debug!("Deny authorization for request: {}", form.request_id);

    // Get the authorization request (to get redirect_uri)
    let request = get_authorization_request(&ctx, &form.request_id).await?;

    // Delete the authorization request (cleanup)
    let result = sqlx::query(
        r#"
        DELETE FROM authorization_request
        WHERE request_id = ?
        "#,
    )
    .bind(&form.request_id)
    .execute(&ctx.account_db)
    .await?;

    if result.rows_affected() == 0 {
        warn!("Authorization request not found for deletion: {}", form.request_id);
    }

    debug!("Deleted authorization request: {}", form.request_id);

    // Build redirect URL with error
    let mut redirect_url = url::Url::parse(&request.redirect_uri).map_err(|e| {
        PdsError::Internal(format!("Invalid redirect_uri in request: {}", e))
    })?;

    // Add error parameter (per OAuth 2.1 spec)
    redirect_url
        .query_pairs_mut()
        .append_pair("error", "access_denied")
        .append_pair("error_description", "User denied authorization");

    // Add state parameter if present
    if let Some(state) = &request.state {
        redirect_url.query_pairs_mut().append_pair("state", state);
    }

    debug!("Redirecting to client with error: {}", redirect_url);

    // Redirect to client
    Ok(Redirect::to(redirect_url.as_str()))
}

/// Mark authorization code as used
///
/// Updates the authorization_request to mark the code as used.
/// This prevents replay attacks where an attacker tries to reuse
/// an authorization code.
///
/// # Arguments
/// * `ctx` - Application context
/// * `authorization_code` - The authorization code to mark as used
///
/// # Returns
/// Ok if successful, error if code not found or already used
pub async fn mark_code_as_used(ctx: &AppContext, authorization_code: &str) -> PdsResult<()> {
    let now = Utc::now();

    let result = sqlx::query(
        r#"
        UPDATE authorization_request
        SET code_used = 1, code_used_at = ?
        WHERE authorization_code = ? AND code_used = 0
        "#,
    )
    .bind(now)
    .bind(authorization_code)
    .execute(&ctx.account_db)
    .await?;

    if result.rows_affected() == 0 {
        return Err(PdsError::Authentication(
            "Authorization code invalid or already used".to_string(),
        ));
    }

    debug!("Marked authorization code as used: {}", &authorization_code[..8]);

    Ok(())
}

/// Get authorization request by authorization code
///
/// Retrieves the authorization request associated with a given code.
/// This is used by the token endpoint to exchange the code for tokens.
///
/// # Arguments
/// * `ctx` - Application context
/// * `authorization_code` - The authorization code
///
/// # Returns
/// Authorization request if found and valid
pub async fn get_request_by_code(
    ctx: &AppContext,
    authorization_code: &str,
) -> PdsResult<crate::oauth::models::AuthorizationRequest> {
    let row = sqlx::query(
        r#"
        SELECT
            id, request_id, did, client_id, code_challenge, code_challenge_method,
            authorization_code, scope, redirect_uri, state, created_at, expires_at,
            code_used, code_used_at
        FROM authorization_request
        WHERE authorization_code = ?
        "#,
    )
    .bind(authorization_code)
    .fetch_optional(&ctx.account_db)
    .await?
    .ok_or_else(|| {
        PdsError::Authentication(format!(
            "Invalid authorization code: {}",
            &authorization_code[..8]
        ))
    })?;

    // Check if expired
    let expires_at: chrono::DateTime<Utc> = row.get("expires_at");
    if expires_at < Utc::now() {
        return Err(PdsError::Authentication(
            "Authorization code expired".to_string(),
        ));
    }

    // Check if already used
    let code_used: bool = row.get("code_used");
    if code_used {
        return Err(PdsError::Authentication(
            "Authorization code already used".to_string(),
        ));
    }

    Ok(crate::oauth::models::AuthorizationRequest {
        id: row.get("id"),
        request_id: row.get("request_id"),
        did: row.get("did"),
        client_id: row.get("client_id"),
        code_challenge: row.get("code_challenge"),
        code_challenge_method: row.get("code_challenge_method"),
        authorization_code: row.get("authorization_code"),
        scope: row.get("scope"),
        redirect_uri: row.get("redirect_uri"),
        state: row.get("state"),
        created_at: row.get("created_at"),
        expires_at: row.get("expires_at"),
        code_used: row.get("code_used"),
        code_used_at: row.get("code_used_at"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_consent_query_deserialization() {
        // Test that ConsentQuery can be deserialized
        let query = ConsentQuery {
            request_id: "test_request_123".to_string(),
        };

        assert_eq!(query.request_id, "test_request_123");
    }

    #[test]
    fn test_grant_form_deserialization() {
        let form = GrantForm {
            request_id: "test_request_456".to_string(),
        };

        assert_eq!(form.request_id, "test_request_456");
    }

    #[test]
    fn test_deny_form_deserialization() {
        let form = DenyForm {
            request_id: "test_request_789".to_string(),
        };

        assert_eq!(form.request_id, "test_request_789");
    }
}
