//! Service Token Generation CLI Command
//!
//! Provides command-line tools for generating service authentication JWT tokens.

use crate::{context::AppContext, error::PdsResult, service_auth::create_service_jwt};

/// Generate a service authentication token
pub async fn generate_service_token(
    ctx: &AppContext,
    aud: &str,
    lifetime: i64,
    lxm: Option<&str>,
) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Service Token Generation");
    println!("════════════════════════════════════════════════════════");

    let iss = &ctx.config.service.service_did;
    println!("Issuer (iss):   {}", iss);
    println!("Audience (aud): {}", aud);
    println!("Lifetime:       {} seconds", lifetime);
    if let Some(method) = lxm {
        println!("Method (lxm):   {}", method);
    }

    println!("\n📝 Generating service token...");

    // Decode the repo signing key from hex
    let signing_key_hex = &ctx.config.authentication.repo_signing_key;
    let signing_key = hex::decode(signing_key_hex).map_err(|e| {
        crate::error::PdsError::Internal(format!("Failed to decode repo signing key: {}", e))
    })?;

    // Validate key length (should be 32 bytes for secp256k1)
    if signing_key.len() != 32 {
        return Err(crate::error::PdsError::Validation(format!(
            "Invalid repo signing key length: expected 32 bytes, got {}",
            signing_key.len()
        )));
    }

    // Generate the JWT token
    let token = create_service_jwt(iss, aud, Some(lifetime), lxm, &signing_key)?;

    println!("✓ Token generated successfully");

    println!("\n════════════════════════════════════════════════════════");
    println!("  Generated Service Token");
    println!("════════════════════════════════════════════════════════\n");
    println!("{}", token);

    println!("\n════════════════════════════════════════════════════════");
    println!("Usage:");
    println!("  Use this token in the Authorization header:");
    println!("  Authorization: Bearer {}", token);
    println!("\n  This token is valid for {} seconds", lifetime);
    if let Some(method) = lxm {
        println!("  Method-specific token for: {}", method);
    } else {
        println!("  General-purpose service token (no method restriction)");
    }
    println!("\n⚠️  Security Warning:");
    println!("  This token grants service-level access.");
    println!("  Keep it secure and do not share it.");
    println!("  Token expires at: {} seconds from now", lifetime);
    println!("════════════════════════════════════════════════════════\n");

    Ok(())
}
