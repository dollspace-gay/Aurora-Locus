//! Differential conformance harness (design §24.1): drives the same XRPC
//! scenarios against locus and the pinned reference PDS and compares
//! status codes, error names, response shapes and firehose events, after
//! normalizing values that legitimately differ. Differences listed in the
//! allowlist (each with the design section that justifies it) are
//! reported but don't fail the run.
//!
//! `locus-conformance plc <port>` runs the shared mock PLC directory,
//! `locus-conformance appview <port>` the mock AppView.
//! `locus-conformance run --locus <url> --reference <url> --allowlist <file>
//!   --plc <url> [--mail <mail catcher API> --mail-from-locus <addr>
//!   --mail-from-reference <addr>] [--admin-locus <pw>]
//!   [--admin-reference <pw>] [--report <file>]`. With `--mail` both
//! servers send email and the run is in `mail` mode, otherwise `no-mail`
//! (allowlist entries can be limited to one with `when`).

mod appview;
mod firehose;
mod groups;
mod normalize;
mod plc;
mod runner;
mod scenarios;

use std::process::ExitCode;
use std::time::Duration;

use serde_json::{Value, json};

use runner::{Server, compare, load_allowlist};

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("plc") => {
            let port = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(2582);
            if let Err(e) = plc::serve(port).await {
                eprintln!("mock PLC: {e}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Some("appview") => {
            let port = args.get(2).and_then(|p| p.parse().ok()).unwrap_or(2583);
            if let Err(e) = appview::serve(port).await {
                eprintln!("mock AppView: {e}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Some("run") => run(&args).await,
        _ => {
            eprintln!(
                "usage: locus-conformance plc <port> | appview <port> | run --locus <url> --reference <url> --allowlist <file>"
            );
            ExitCode::FAILURE
        }
    }
}

#[allow(clippy::too_many_lines)]
async fn run(args: &[String]) -> ExitCode {
    let (Some(locus_url), Some(ref_url), Some(allow_path)) = (
        arg(args, "--locus"),
        arg(args, "--reference"),
        arg(args, "--allowlist"),
    ) else {
        eprintln!("--locus, --reference and --allowlist are required");
        return ExitCode::FAILURE;
    };
    let allow_json: Value = match std::fs::read_to_string(&allow_path)
        .map_err(|e| e.to_string())
        .and_then(|s| serde_json::from_str(&s).map_err(|e| e.to_string()))
    {
        Ok(v) => v,
        Err(e) => {
            eprintln!("allowlist {allow_path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mail = arg(args, "--mail");
    let mode = if mail.is_some() { "mail" } else { "no-mail" };
    println!("mode: {mode}");
    let allow = load_allowlist(&allow_json, mode);
    let plc = arg(args, "--plc").unwrap_or_else(|| "http://localhost:2582".into());
    let mut locus = Server::new(
        &locus_url,
        &arg(args, "--admin-locus").unwrap_or_else(|| "admin-pw".into()),
        &plc,
    );
    let mut reference = Server::new(
        &ref_url,
        &arg(args, "--admin-reference").unwrap_or_else(|| "admin-pw".into()),
        &plc,
    );
    locus.norm.peer.clone_from(&reference.norm.host);
    reference.norm.peer.clone_from(&locus.norm.host);
    if let Some(api) = &mail {
        locus.mail = arg(args, "--mail-from-locus").map(|f| (api.clone(), f));
        reference.mail = arg(args, "--mail-from-reference").map(|f| (api.clone(), f));
    }
    let (fl, fr) = match (
        firehose::collect(&locus_url).await,
        firehose::collect(&ref_url).await,
    ) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(e), _) | (_, Err(e)) => {
            eprintln!("firehose: {e}");
            return ExitCode::FAILURE;
        }
    };

    let mut report = Vec::new();
    let (mut unexpected, mut allowed) = (0usize, 0usize);
    for step in scenarios::all() {
        let a = locus.run(&step, &reference).await;
        let b = reference.run(&step, &locus).await;
        if step.target == runner::Target::Peer {
            // The other server's half of a migration: not compared.
            println!("ok   {} (peer: {} / {})", step.name, a.status, b.status);
            continue;
        }
        let out = compare(&step.name, &a, &b, &allow);
        unexpected += out.unexpected.len();
        allowed += out.allowed.len();
        let mark = if out.unexpected.is_empty() {
            "ok  "
        } else {
            "DIFF"
        };
        println!("{mark} {} ({} / {})", step.name, a.status, b.status);
        for d in &out.unexpected {
            println!(
                "       {}  locus={}  reference={}",
                d.path, d.locus, d.reference
            );
        }
        if !out.unexpected.is_empty() && (a.message.is_some() || b.message.is_some()) {
            println!(
                "       messages: locus={:?} reference={:?}",
                a.message, b.message
            );
        }
        report.push(json!({
            "step": out.step,
            "status": [a.status, b.status],
            "unexpected": out.unexpected.iter().map(|d| json!({"path": d.path, "locus": d.locus, "reference": d.reference})).collect::<Vec<_>>(),
            "allowed": out.allowed.iter().map(|(d, a)| json!({"path": d.path, "section": a.section, "why": a.why})).collect::<Vec<_>>(),
        }));
    }

    // The firehose, once both have caught up.
    tokio::time::sleep(Duration::from_secs(3)).await;
    let events = |e: &firehose::Events, n: &mut normalize::Normalizer| -> Value {
        let v = e.lock().map(|v| v.clone()).unwrap_or_default();
        n.value(&Value::Array(v))
    };
    let el = events(&fl, &mut locus.norm);
    let er = events(&fr, &mut reference.norm);
    let out = compare(
        "firehose",
        &runner::Answer {
            status: 0,
            body: el.clone(),
            message: None,
        },
        &runner::Answer {
            status: 0,
            body: er.clone(),
            message: None,
        },
        &allow,
    );
    unexpected += out.unexpected.len();
    allowed += out.allowed.len();
    let mark = if out.unexpected.is_empty() {
        "ok  "
    } else {
        "DIFF"
    };
    println!(
        "{mark} firehose ({} / {} events)",
        el.as_array().map_or(0, Vec::len),
        er.as_array().map_or(0, Vec::len)
    );
    for d in &out.unexpected {
        println!(
            "       {}  locus={}  reference={}",
            d.path, d.locus, d.reference
        );
    }
    report.push(json!({"step": "firehose", "locus": el, "reference": er,
        "unexpected": out.unexpected.iter().map(|d| json!({"path": d.path, "locus": d.locus, "reference": d.reference})).collect::<Vec<_>>()}));

    println!("{unexpected} unexpected differences, {allowed} allowed");
    if let Some(p) = arg(args, "--report")
        && let Err(e) = std::fs::write(
            &p,
            serde_json::to_string_pretty(&report).unwrap_or_default(),
        )
    {
        eprintln!("report {p}: {e}");
    }
    if unexpected == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
