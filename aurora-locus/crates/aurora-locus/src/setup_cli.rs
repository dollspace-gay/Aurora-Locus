//! `aurora-locus setup`: the wizard's steps on the command line, or from a
//! `setup.toml` with `--config`. The plan, the checks and the apply are the
//! wizard's own, so every path ends in the same state.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use locus_core::bootstrap::Bootstrap;
use locus_core::setup::{self as rules, ProxyMode, SetupPlan};
use locus_http::setup::{self, CheckEnv, Outcome};
use locus_store::email::{Sender, SenderStatus};

use crate::node;

/// Runs the command; the exit code.
pub async fn run(
    bootstrap: Bootstrap,
    config: Option<PathBuf>,
    accept_warnings: bool,
    new_token: bool,
) -> i32 {
    match run_inner(bootstrap, config, accept_warnings, new_token).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

async fn run_inner(
    bootstrap: Bootstrap,
    config: Option<PathBuf>,
    accept_warnings: bool,
    new_token: bool,
) -> Result<(), String> {
    let (store, key) = node::open_store(&bootstrap)
        .await
        .map_err(|e| e.to_string())?;
    if store.setup_completed().await.map_err(|e| e.to_string())? {
        return Err("Setup is already complete. Change settings in the console instead.".into());
    }
    let dev = bootstrap.dev_mode_honored();

    if new_token {
        let token = setup::new_token();
        let hash = setup::token_hash(&token);
        let made = store
            .create_setup_token(&hash)
            .await
            .map_err(|e| e.to_string())?
            || store
                .replace_setup_token(&hash)
                .await
                .map_err(|e| e.to_string())?;
        if !made {
            return Err("Setup completed while the token was being replaced.".into());
        }
        println!("{token}");
        eprintln!("This is the new setup token; the old one no longer works. Enter it at /setup.");
        return Ok(());
    }

    // The keys exist before setup ends, as when a node boots.
    let secrets = locus_http::ServerSecrets::load_or_create(&store, &key)
        .await
        .map_err(|e| e.to_string())?;
    let mailer = locus_mail::Mailer::new(store.clone(), key.clone());
    let snap = store.load_snapshot().await.map_err(|e| e.to_string())?;
    let bootstrap_hostname = bootstrap.hostname.as_deref().map(rules::normalize_host);
    let seed = setup::seed_plan(&snap, bootstrap_hostname.as_deref());

    let mut plan = match &config {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .map_err(|e| format!("can't read {}: {e}", path.display()))?;
            let mut p =
                SetupPlan::from_toml(&text).map_err(|e| format!("{}: {e}", path.display()))?;
            if p.hostname.is_empty() {
                p.hostname = seed.hostname.clone();
                p.normalize();
            }
            p
        }
        None => {
            interactive(
                seed,
                &mailer,
                &secrets.plc_keys.current_did().unwrap_or_default(),
                dev,
            )
            .await?
        }
    };

    // An SMTP URL is stored and checked first, so the checks see it.
    let mut email = mailer
        .status(Sender::Main)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(url) = plan.email.smtp_url.clone().filter(|u| !u.is_empty())
        && !plan.email.skipped
    {
        email = Some(
            mailer
                .set_url(Sender::Main, &url)
                .await
                .map_err(|e| e.to_string())?,
        );
    }

    let cf_token = std::env::var("LOCUS_SETUP_CLOUDFLARE_TOKEN").ok();
    let checks = setup::run_checks(
        &CheckEnv {
            store: &store,
            snap: &snap,
            dev,
            bootstrap_hostname: bootstrap_hostname.as_deref(),
            email,
            // Only a node in setup mode answers the probe.
            probe: true,
            cf_token: cf_token.as_deref(),
            cf_api: setup::CLOUDFLARE_API,
        },
        &plan,
    )
    .await;
    let mut unaccepted = Vec::new();
    for c in &checks {
        let word = match c.outcome {
            Outcome::Pass => "pass",
            Outcome::Warn => "WARN",
            Outcome::Fail => "FAIL",
            Outcome::Skip => "skip",
        };
        println!("[{word}] {}: {}", c.label, c.detail);
        if c.outcome == Outcome::Warn && !plan.acknowledged.iter().any(|a| a == c.id) {
            unaccepted.push(c.id);
        }
    }
    if checks.iter().any(|c| c.outcome == Outcome::Fail) {
        return Err("Setup can't finish until the failed checks pass.".into());
    }
    if !unaccepted.is_empty() {
        let accept = accept_warnings
            || (config.is_none() && confirm("Accept these warnings and finish?", false)?);
        if !accept {
            return Err(format!(
                "Not finished: accept the warnings ({}) with --accept-warnings, or list them under `acknowledged` in the config.",
                unaccepted.join(", ")
            ));
        }
        plan.acknowledged
            .extend(unaccepted.iter().map(|s| (*s).to_owned()));
    }
    setup::apply(&store, &key, &plan, dev)
        .await
        .map_err(|e| e.to_string())?;
    println!("Setup is complete. Nodes in setup mode restart into service on their own.");
    if plan.has_operator() {
        println!(
            "Sign in to the console at /admin as {}.",
            plan.operator.username
        );
    }
    Ok(())
}

fn read_line() -> Result<String, String> {
    let mut s = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut s)
        .map_err(|e| e.to_string())?;
    Ok(s.trim().to_owned())
}

fn ask(prompt: &str, default: &str) -> Result<String, String> {
    if default.is_empty() {
        print!("{prompt}: ");
    } else {
        print!("{prompt} [{default}]: ");
    }
    std::io::stdout().flush().map_err(|e| e.to_string())?;
    let v = read_line()?;
    Ok(if v.is_empty() { default.to_owned() } else { v })
}

fn confirm(prompt: &str, default: bool) -> Result<bool, String> {
    let d = if default { "Y/n" } else { "y/N" };
    let v = ask(&format!("{prompt} ({d})"), "")?;
    Ok(match v.to_ascii_lowercase().as_str() {
        "" => default,
        v => v.starts_with('y'),
    })
}

fn secret(prompt: &str) -> Result<String, String> {
    rpassword::prompt_password(format!("{prompt}: ")).map_err(|e| e.to_string())
}

/// Asks how email is sent (Resend, an SMTP server's details, or a URL) and
/// returns the SMTP URL, or (inner) what's wrong with the answers.
fn smtp_url_prompt() -> Result<Result<String, String>, String> {
    use locus_mail::provider::SmtpChoice;
    println!("  Send email through: 1) Resend  2) another SMTP server  3) an SMTP URL");
    let choice = match ask("  1, 2 or 3", "1")?.as_str() {
        "1" => SmtpChoice::Resend {
            api_key: secret("  Resend API key")?.trim().to_owned(),
            alt_port: confirm(
                "  Does your server's host block the usual mail ports (25, 465, 587)?",
                false,
            )?,
        },
        "2" => {
            let host = ask("  Host", "")?;
            let starttls = ask("  Security: 1) TLS from the start  2) STARTTLS", "1")? == "2";
            let port = ask("  Port", if starttls { "587" } else { "465" })?;
            let username = ask("  Username (empty for none)", "")?;
            let password = if username.is_empty() {
                String::new()
            } else {
                secret("  Password")?
            };
            SmtpChoice::Server {
                host,
                port,
                starttls,
                username,
                password,
            }
        }
        "3" => SmtpChoice::Url(secret("  SMTP URL (smtps://user:password@host)")?),
        _ => return Ok(Err("Pick 1, 2 or 3.".into())),
    };
    Ok(choice.smtp_url())
}

fn say_problems(problems: Vec<rules::Problem>) -> bool {
    for p in &problems {
        println!("  ! {}", p.message);
    }
    problems.is_empty()
}

/// The wizard's steps as questions.
async fn interactive(
    mut plan: SetupPlan,
    mailer: &locus_mail::Mailer,
    rotation_key: &str,
    dev: bool,
) -> Result<SetupPlan, String> {
    println!("Setting up this PDS. Press Enter to keep the value in [brackets].\n");

    println!("1. Hostname");
    loop {
        plan.hostname = rules::normalize_host(&ask("  The PDS hostname", &plan.hostname)?);
        plan.handle_domains
            .retain(|d| plan.hostname.ends_with(d.as_str()));
        plan.normalize();
        if say_problems(plan.hostname_problems(dev)) {
            break;
        }
    }
    plan.service_name = ask("  Service name (optional)", &plan.service_name)?;
    plan.contact_email = ask("  Contact email (optional)", &plan.contact_email)?;

    println!("\n2. Proxy and TLS");
    for (i, m) in ProxyMode::ALL.iter().enumerate() {
        println!("  {}) {}", i + 1, m.label());
    }
    let current = ProxyMode::ALL
        .iter()
        .position(|m| *m == plan.proxy)
        .unwrap_or(0)
        + 1;
    let pick = ask("  Which", &current.to_string())?;
    plan.proxy = pick
        .parse::<usize>()
        .ok()
        .and_then(|i| ProxyMode::ALL.get(i.wrapping_sub(1)).copied())
        .unwrap_or(plan.proxy);
    loop {
        let v = ask(
            "  Trusted proxy ranges, comma-separated (empty: this machine only)",
            &plan.trusted_proxies.join(", "),
        )?;
        plan.trusted_proxies = locus_http::web::lines(&v);
        if say_problems(
            plan.hostname_problems(dev)
                .into_iter()
                .filter(|p| p.field == "trusted_proxies")
                .collect(),
        ) {
            break;
        }
    }

    println!("\n3. Handles");
    let suggested = rules::suggested_handle_domains(&plan.hostname);
    println!(
        "  Suggested: {}",
        suggested
            .iter()
            .map(|d| format!("*{d}"))
            .collect::<Vec<_>>()
            .join(" or ")
    );
    loop {
        let v = ask(
            "  Handle domains, comma-separated",
            &plan.handle_domains.join(", "),
        )?;
        plan.handle_domains = locus_http::web::lines(&v);
        let v = ask(
            "  Subdomains already in use (www, mail, …)",
            &plan.existing_subdomains.join(", "),
        )?;
        plan.existing_subdomains = locus_http::web::lines(&v);
        plan.normalize();
        if say_problems(plan.handle_problems()) {
            break;
        }
    }
    println!("  DNS records to create:");
    for (name, to, note) in plan.dns_records() {
        println!("    {name} → {to} ({note})");
    }

    println!("\n4. First operator (the console's owner)");
    loop {
        plan.operator.username = ask("  Username", &plan.operator.username)?;
        let p1 = secret("  Password (12 characters or more)")?;
        let p2 = secret("  The password again")?;
        if p1 != p2 {
            println!("  ! The two passwords differ.");
            continue;
        }
        plan.operator.password = Some(p1);
        if say_problems(plan.operator_problems()) {
            break;
        }
    }

    println!("\n5. Email");
    if confirm("  Set up email now?", true)? {
        loop {
            let url = match smtp_url_prompt()? {
                Ok(u) => u,
                Err(e) => {
                    println!("  ! {e}");
                    continue;
                }
            };
            plan.email.from_address = ask("  Send from", &plan.email.from_address)?;
            plan.email.smtp_url = Some(url.clone());
            plan.email.skipped = false;
            if !say_problems(plan.email_problems()) {
                continue;
            }
            match mailer
                .set_url(Sender::Main, &url)
                .await
                .map_err(|e| e.to_string())?
            {
                SenderStatus::Failed => {
                    match mailer.blocked_port_hint(Sender::Main).await {
                        Some(h) => println!("  ! The SMTP server couldn't be reached. {h}"),
                        None => println!("  ! The SMTP server refused the connection."),
                    }
                    if confirm("  Try again?", true)? {
                        continue;
                    }
                }
                _ => println!("  The SMTP server accepted the connection."),
            }
            break;
        }
    } else {
        plan.email.skipped = true;
        println!(
            "  Without email, people can't confirm their address, reset a password by email or get sign-in codes."
        );
    }

    println!("\n6. Network services (Enter keeps Bluesky's defaults)");
    loop {
        let s = &mut plan.services;
        s.appview_url = ask("  AppView URL", &s.appview_url)?;
        s.appview_did = ask("  AppView DID", &s.appview_did)?;
        s.crawlers = locus_http::web::lines(&ask(
            "  Relays to notify, comma-separated",
            &s.crawlers.join(", "),
        )?);
        s.moderation_url = ask("  Moderation service URL (optional)", &s.moderation_url)?;
        if !s.moderation_url.is_empty() {
            s.moderation_did = ask("  Moderation service DID", &s.moderation_did)?;
        }
        s.report_url = ask("  Reports go to", &s.report_url)?;
        s.report_did = ask("  Its DID", &s.report_did)?;
        if say_problems(plan.services_problems()) {
            break;
        }
    }

    println!("\n7. Private posts");
    plan.private_tier = confirm("  Allow private posts?", plan.private_tier)?;
    if plan.private_tier {
        println!(
            "  Private posts need a private AppView (prism); it can be the same as the AppView above."
        );
        loop {
            let s = &mut plan.services;
            s.private_appview_url = ask("  Private AppView URL", &s.private_appview_url)?;
            if !s.private_appview_url.is_empty() {
                s.private_appview_did = ask("  Private AppView DID", &s.private_appview_did)?;
            }
            if say_problems(plan.services_problems()) {
                break;
            }
        }
    }

    println!("\n8. Keys");
    println!("  PLC rotation key: {rotation_key}");
    println!(
        "  If the master key is lost, every stored key is lost with it. Keep a copy off this server."
    );
    plan.recovery_did_key = ask("  Your recovery did:key (optional)", &plan.recovery_did_key)?;
    plan.keys_backup_acknowledged = confirm("  Do you have a copy of the master key?", false)?;
    plan.normalize();
    if !say_problems(plan.problems(dev)) {
        return Err("Setup isn't complete; run it again.".into());
    }
    println!();
    Ok(plan)
}
