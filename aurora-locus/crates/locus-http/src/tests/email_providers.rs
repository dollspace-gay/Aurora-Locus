//! The console's email form: a provider and its credentials, another SMTP
//! server's details, or a URL, each saved as the sender's SMTP URL. Saves point at a closed
//! local port, so the check fails at once without the network.

use super::*;

async fn saved(h: &Harness, name: &str) -> Option<String> {
    h.state
        .store
        .get_secret(name, &h.state.master_key)
        .await
        .unwrap()
        .map(|b| String::from_utf8(b.to_vec()).unwrap())
}

/// A local port nothing listens on.
fn closed_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    l.local_addr().unwrap().port()
}

#[tokio::test]
async fn the_console_saves_each_way_of_sending_as_an_smtp_url() {
    let h = harness(false).await;
    h.console_operator("own", "owner", "an owner password")
        .await;
    let o = Browser::default();
    h.console_sign_in(&o, "own", "an owner password").await;
    let (_, _, page) = o.get(&h, "/admin/email").await;
    // One form: the provider first, then each provider's fields marked
    // with the choices they show for.
    assert!(
        page.contains("Who&#x27;s your email provider?")
            && page.contains(r#"<option value="resend" selected>Resend</option>"#)
            && page.contains(r#"<option value="other">Other"#)
            && page.contains(r#"data-for="resend sendgrid mailgun postmark ses brevo""#)
            && page.contains(r#"data-for="resend""#)
            && page.contains(r#"name="resend_key""#)
            && page.contains(r#"name="mailgun_region""#)
            && page.contains(r#"type="checkbox""#)
            && page.contains("<h2>How email is sent</h2>")
            && !page.contains("<h2>Resend</h2>"),
        "{page}"
    );
    // Sending a test email and confirming it share one card.
    let test_card = page
        .split_once("<h2>Send a test email</h2>")
        .map(|(_, rest)| rest.split(r#"<div class="card">"#).next().unwrap_or(rest))
        .expect("the test email card");
    assert!(
        test_card.contains(r#"action="/admin/email/test""#)
            && test_card.contains(r#"action="/admin/email/verify""#)
            && !page.contains("<h2>Confirm delivery</h2>"),
        "{page}"
    );
    // The stylesheet hides other providers' fields for every choice.
    let css = include_str!("../../../locus-web/static/locus.css");
    for (v, _) in locus_mail::provider::provider_options() {
        assert!(
            css.contains(&format!(
                r#"option[value="{v}"]:checked) [data-for]:not([data-for~="{v}"])"#
            )),
            "no stylesheet rule for {v}"
        );
    }
    let csrf = hidden(&page, "csrf");
    let post = |fields: Vec<(&'static str, String)>| {
        let csrf = csrf.clone();
        let o = &o;
        let h = &h;
        async move {
            let mut f: Vec<(&str, &str)> = vec![("csrf", csrf.as_str())];
            f.extend(fields.iter().map(|(k, v)| (*k, v.as_str())));
            o.post(h, "/admin/email/smtp", &f).await
        }
    };

    // A server's details, with a password that needs encoding.
    let port = closed_port();
    let (st, _, page) = post(vec![
        ("sender", "main".into()),
        ("provider", "other".into()),
        ("host", "127.0.0.1".into()),
        ("port", port.to_string()),
        ("security", "starttls".into()),
        ("username", "ops@example.com".into()),
        ("password", "p@ss/word".into()),
    ])
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "nothing listens there");
    assert!(page.contains("refused the connection"), "{page}");
    assert!(
        !page.contains("couldn&#x27;t be reached"),
        "no port hint off the usual ports: {page}"
    );
    assert_eq!(
        saved(&h, "email/smtp_url").await.as_deref(),
        Some(format!("smtp://ops%40example.com:p%40ss%2Fword@127.0.0.1:{port}").as_str())
    );

    // On a usual port, an unreachable server gets the hint (naming no host).
    let (st, _, page) = post(vec![
        ("sender", "moderation".into()),
        ("provider", "other".into()),
        ("host", "127.0.0.1".into()),
        ("port", "25".into()),
        ("security", "starttls".into()),
    ])
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert!(
        page.contains(
            "couldn&#x27;t be reached. Some hosting providers block outgoing mail on ports 25, 465 and 587"
        ),
        "{page}"
    );
    assert_eq!(
        saved(&h, "email/moderation_smtp_url").await.as_deref(),
        Some("smtp://127.0.0.1:25")
    );

    // Bad entries are refused before anything is saved.
    for (fields, says) in [
        (vec![("provider", "resend")], "Enter the Resend API key."),
        (
            vec![
                ("provider", "other"),
                ("host", "smtp.example.com"),
                ("port", "0"),
            ],
            "The port is a number from 1 to 65535.",
        ),
        (
            vec![("provider", "url"), ("smtp_url", "http://x")],
            "An SMTP URL starts with smtp:// or smtps://.",
        ),
    ] {
        let (st, _, page) = post(
            fields
                .into_iter()
                .map(|(k, v)| (k, v.to_owned()))
                .chain([("sender", "main".to_owned())])
                .collect(),
        )
        .await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(page.contains(says), "{says}: {page}");
    }
    assert_eq!(
        saved(&h, "email/smtp_url").await.as_deref(),
        Some(format!("smtp://ops%40example.com:p%40ss%2Fword@127.0.0.1:{port}").as_str()),
        "unchanged by the refused entries"
    );

    // The older single-URL form still saves.
    let (st, _, _) = post(vec![
        ("sender", "main".into()),
        ("url", format!("smtp://127.0.0.1:{port}")),
    ])
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    assert_eq!(
        saved(&h, "email/smtp_url").await.as_deref(),
        Some(format!("smtp://127.0.0.1:{port}").as_str())
    );
}
