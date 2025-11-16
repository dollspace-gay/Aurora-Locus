/// Account Management CLI Commands
///
/// Provides command-line tools for account creation and management.

use crate::{
    context::AppContext,
    error::PdsResult,
};

/// Create a new account via CLI
pub async fn create_account(
    ctx: &AppContext,
    email: &str,
    handle: &str,
    password: &str,
    invite_code: Option<&str>,
) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  Creating Account");
    println!("════════════════════════════════════════════════════════");
    println!("Email:   {}", email);
    println!("Handle:  {}", handle);
    if let Some(code) = invite_code {
        println!("Invite:  {}", code);
    }
    println!("════════════════════════════════════════════════════════\n");

    // Validate invite code if provided
    if let Some(code) = invite_code {
        println!("📋 Validating invite code...");
        ctx.account_manager.validate_invite_code(code, None).await?;
        println!("✓ Invite code is valid");
    }

    // Create account
    println!("📝 Creating account...");
    let account = ctx
        .account_manager
        .create_account(
            handle.to_string(),
            Some(email.to_string()),
            password.to_string(),
            invite_code.map(|s| s.to_string()),
        )
        .await?;

    println!("\n✅ Account created successfully!");
    println!("════════════════════════════════════════════════════════");
    println!("DID:    {}", account.did);
    if let Some(handle) = &account.handle {
        println!("Handle: {}", handle);
    }
    if let Some(email_addr) = &account.email {
        println!("Email:  {}", email_addr);
    }
    println!("════════════════════════════════════════════════════════\n");

    // Generate and send verification email if email is configured
    if ctx.mailer.is_configured() && account.email.is_some() {
        println!("📧 Generating email verification token...");

        let token = ctx
            .account_manager
            .generate_email_verification_token(&account.did)
            .await?;

        let base_url = format!("https://{}", ctx.config.service.hostname);

        println!("📤 Sending verification email...");
        ctx.mailer
            .send_verification_email(
                account.email.as_ref().unwrap(),
                account.handle.as_deref().unwrap_or(&account.did),
                &token,
                &base_url,
            )
            .await?;

        println!("✓ Verification email sent to {}", account.email.as_ref().unwrap());
        println!("\n⚠️  Please check your email to verify your account.");
    } else if !ctx.mailer.is_configured() {
        println!("⚠️  Email not configured - verification email not sent");
        println!("   Account created but email verification required");
    }

    Ok(())
}
