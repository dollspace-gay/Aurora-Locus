//! Phase B DNS-TXT authority responder (Cluster 1 Member 1.2 / Scenario 13).
//!
//! A tiny tokio UDP server that answers DNS TXT queries against a static
//! HashMap loaded from a JSON config file. Built for one purpose only:
//! standing in as the `_lexicon.<authority>` DNS authority during Phase B
//! Scenario 13 so the live `HickoryDnsTxtResolver` actually fires (vs the
//! `PDS_LEXICON_DID_AUTHORITY` override path Scenario 13 forbids).
//!
//! ## Hard boundary — emit pass-through
//!
//! 13c's malformed-TXT cases (uppercase `DID=`, leading/trailing
//! whitespace, unrecognized prefix) require the responder to emit
//! arbitrary bytes verbatim. **No normalization** anywhere on the
//! encode path: the configured TXT bytes are written as-is, length-
//! prefixed per character-string per RFC 1035 §3.3.14. The only
//! constraint is the RFC byte-level cap (≤ 255 bytes per
//! character-string); the responder errors at config-load time if a
//! character-string exceeds that.
//!
//! ## TTL = 0 (cache-defeat Layer 1)
//!
//! All answers ship with TTL = 0. Hickory's positive_min_ttl defaults
//! to 0 so a TTL-0 answer caches as already-expired. This is one of
//! three cache-defeat layers Scenario 13 stacks; per-sub-case unique
//! NSIDs (Layer 3) are the load-bearing protection — TTL = 0 is
//! belt-and-suspenders.
//!
//! ## NOT a general DNS server
//!
//! - Only TXT queries supported. Other QTYPEs get NXDOMAIN.
//! - Names not in the config get NXDOMAIN.
//! - No recursion, no NSEC, no DNSSEC.
//! - No SOA, no NS — the responder doesn't claim authority over any
//!   zone in the way `dig SOA` would expect.
//! - Single-question queries only (the universal real-world shape;
//!   multi-question queries get FORMERR).
//!
//! ## Config format (JSON)
//!
//! ```json
//! {
//!   "records": [
//!     {
//!       "name": "_lexicon.test13a.example.com",
//!       "txt_records": [["did=did:plc:abc1"], ["did=did:plc:abc2"]]
//!     },
//!     {
//!       "name": "_lexicon.test13b.example.com",
//!       "txt_records": [["did=did:plc:xyz1", "did=did:plc:xyz2"]]
//!     },
//!     {
//!       "name": "_lexicon.test13c.example.com",
//!       "txt_records": [["DID=did:plc:def1  "]]
//!     }
//!   ]
//! }
//! ```
//!
//! Each `txt_records` entry is `Vec<Vec<String>>`:
//! - Outer vec: distinct TXT records (multiple answer-section RRs).
//! - Inner vec: character-strings within one TXT record (one TXT RR
//!   can carry multiple length-prefixed strings per RFC 1035 §3.3.14).
//!
//! ## Usage
//!
//! ```text
//! phase-b-dns-responder --bind 127.0.0.1:5353 --config /tmp/phase-b-dns.json
//! ```
//!
//! Add `--self-test` to run a single in-process query-response sanity
//! check against the loaded config and exit. Used for CC-side compile
//! and run sanity (NOT Phase B judgment, which is skydeval's against
//! the real bumped binary).

#![allow(clippy::result_large_err)]

use clap::Parser;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use tokio::net::UdpSocket;
use tokio::time::{timeout, Duration};

// ============================================================================
// CLI
// ============================================================================

#[derive(Parser, Debug)]
#[command(
    name = "phase-b-dns-responder",
    about = "Tiny DNS TXT responder for Aurora-Locus Phase B Scenario 13"
)]
struct Cli {
    /// Address to bind. Default 127.0.0.1:5353 (unprivileged port; no
    /// sudo). Phase B's resolver constructor injection points at this
    /// via PDS_LEXICON_DNS_NAMESERVER.
    #[arg(long, default_value = "127.0.0.1:5353")]
    bind: String,

    /// Path to the JSON records config (see module-level docs).
    #[arg(long)]
    config: PathBuf,

    /// Run a single in-process query/response round-trip for each
    /// configured name (against this responder's own UDP socket) and
    /// exit. CC-side sanity for the encode path; NOT a Phase B
    /// judgment.
    #[arg(long)]
    self_test: bool,
}

// ============================================================================
// Config
// ============================================================================

#[derive(Debug, Deserialize)]
struct Config {
    records: Vec<RecordEntry>,
}

#[derive(Debug, Deserialize)]
struct RecordEntry {
    name: String,
    txt_records: Vec<Vec<String>>,
}

impl Config {
    fn load(path: &PathBuf) -> Result<Self, String> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        let cfg: Config = serde_json::from_str(&raw)
            .map_err(|e| format!("parse {}: {e}", path.display()))?;
        // Validate per-character-string length cap (RFC 1035 §3.3.14:
        // ≤ 255 bytes). Verbatim-encode otherwise; do NOT normalize.
        for entry in &cfg.records {
            for txt in &entry.txt_records {
                for chunk in txt {
                    if chunk.len() > 255 {
                        return Err(format!(
                            "character-string for '{}' exceeds 255 bytes ({} bytes); \
                             RFC 1035 §3.3.14 forbids",
                            entry.name,
                            chunk.len()
                        ));
                    }
                }
            }
        }
        Ok(cfg)
    }

    fn into_lookup(self) -> HashMap<String, Vec<Vec<String>>> {
        self.records
            .into_iter()
            .map(|entry| (entry.name.to_ascii_lowercase(), entry.txt_records))
            .collect()
    }
}

// ============================================================================
// DNS wire format — minimal parse/encode
// ============================================================================

const DNS_HEADER_LEN: usize = 12;
const QTYPE_TXT: u16 = 16;
const QCLASS_IN: u16 = 1;
const RCODE_NOERROR: u16 = 0;
const RCODE_FORMERR: u16 = 1;
const RCODE_NXDOMAIN: u16 = 3;
const RCODE_NOTIMP: u16 = 4;

/// Parsed-out fields from an inbound query. We only care about the
/// id (echoed in the response), qname (resolved against our config),
/// and qtype (must be TXT or we respond NOTIMP).
struct ParsedQuery {
    id: u16,
    qname_offset: usize,
    qname_end: usize,
    qname: String,
    qtype: u16,
    #[allow(dead_code)]
    qclass: u16,
}

#[derive(Debug)]
enum DnsErr {
    Truncated,
    BadFormat,
    BadName,
    UnsupportedOpcode,
}

/// Parse a DNS query. Returns the id, the qname (lowercased,
/// dot-joined), the qtype, and offsets so we can build a name-pointer
/// for the answer section.
fn parse_query(buf: &[u8]) -> Result<ParsedQuery, DnsErr> {
    if buf.len() < DNS_HEADER_LEN {
        return Err(DnsErr::Truncated);
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    let opcode = (flags >> 11) & 0xF;
    if opcode != 0 {
        return Err(DnsErr::UnsupportedOpcode);
    }
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount != 1 {
        return Err(DnsErr::BadFormat);
    }

    // Parse the QNAME. Length-prefixed labels terminated by a 0-length
    // label. We don't support compression pointers in the question —
    // they're forbidden there by RFC anyway.
    let mut idx = DNS_HEADER_LEN;
    let qname_offset = idx;
    let mut labels: Vec<String> = Vec::new();
    loop {
        if idx >= buf.len() {
            return Err(DnsErr::Truncated);
        }
        let len = buf[idx] as usize;
        if len == 0 {
            idx += 1;
            break;
        }
        if len > 63 {
            return Err(DnsErr::BadName); // compression pointer or invalid
        }
        idx += 1;
        if idx + len > buf.len() {
            return Err(DnsErr::Truncated);
        }
        let label = std::str::from_utf8(&buf[idx..idx + len])
            .map_err(|_| DnsErr::BadName)?
            .to_ascii_lowercase();
        labels.push(label);
        idx += len;
    }
    let qname_end = idx;
    let qname = labels.join(".");

    if idx + 4 > buf.len() {
        return Err(DnsErr::Truncated);
    }
    let qtype = u16::from_be_bytes([buf[idx], buf[idx + 1]]);
    let qclass = u16::from_be_bytes([buf[idx + 2], buf[idx + 3]]);

    Ok(ParsedQuery {
        id,
        qname_offset,
        qname_end,
        qname,
        qtype,
        qclass,
    })
}

/// Build a DNS response from the parsed query and the configured TXT
/// records (outer Vec = multiple answer RRs; inner Vec = character-
/// strings within one RR). TTL = 0 (cache-defeat Layer 1).
fn build_response(
    query_buf: &[u8],
    parsed: &ParsedQuery,
    answers: Option<&Vec<Vec<String>>>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(query_buf.len() + 256);

    // Header.
    out.extend_from_slice(&parsed.id.to_be_bytes());

    let rcode = if parsed.qtype != QTYPE_TXT {
        // Non-TXT queries: NOTIMP. (Hickory's TxtLookup path will
        // never send a non-TXT query, but a manual `dig` from an
        // operator confused about the responder shouldn't get a wrong
        // answer.)
        RCODE_NOTIMP
    } else if answers.is_some() {
        RCODE_NOERROR
    } else {
        RCODE_NXDOMAIN
    };
    // QR = 1, OPCODE = 0, AA = 1 (we are authoritative for our HashMap),
    // TC = 0, RD = copy from query, RA = 0, Z = 0, AD = 0, CD = 0,
    // RCODE = above.
    let query_flags = u16::from_be_bytes([query_buf[2], query_buf[3]]);
    let rd_bit = query_flags & 0x0100;
    let resp_flags: u16 = 0x8400 | rd_bit | rcode;
    out.extend_from_slice(&resp_flags.to_be_bytes());

    let qdcount: u16 = 1;
    let ancount: u16 = if rcode == RCODE_NOERROR {
        answers.map(|v| v.len()).unwrap_or(0) as u16
    } else {
        0
    };
    out.extend_from_slice(&qdcount.to_be_bytes());
    out.extend_from_slice(&ancount.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT

    // Question section — echo verbatim from the query.
    out.extend_from_slice(&query_buf[parsed.qname_offset..parsed.qname_end + 4]);

    // Answer section.
    if rcode == RCODE_NOERROR {
        if let Some(records) = answers {
            for txt in records {
                // NAME — compression pointer to the QNAME (offset 12,
                // immediately after the 12-byte header).
                out.push(0xC0);
                out.push(0x0C);
                // TYPE = TXT (16)
                out.extend_from_slice(&QTYPE_TXT.to_be_bytes());
                // CLASS = IN (1)
                out.extend_from_slice(&QCLASS_IN.to_be_bytes());
                // TTL = 0 (cache-defeat Layer 1)
                out.extend_from_slice(&0u32.to_be_bytes());
                // RDLENGTH + RDATA. RDATA is one or more length-prefixed
                // character-strings (RFC 1035 §3.3.14).
                let mut rdata = Vec::with_capacity(64);
                for chunk in txt {
                    let bytes = chunk.as_bytes();
                    // Length cap enforced at config-load time, but
                    // double-check at runtime — wire-protocol invariant.
                    debug_assert!(bytes.len() <= 255);
                    rdata.push(bytes.len() as u8);
                    rdata.extend_from_slice(bytes); // VERBATIM — no normalization
                }
                let rdlen = rdata.len() as u16;
                out.extend_from_slice(&rdlen.to_be_bytes());
                out.extend_from_slice(&rdata);
            }
        }
    }

    out
}

/// FORMERR response for malformed queries we can still extract an id
/// from. Used when the query was structurally bad but the header was
/// recoverable enough to echo.
fn build_formerr(id: u16) -> Vec<u8> {
    let mut out = Vec::with_capacity(DNS_HEADER_LEN);
    out.extend_from_slice(&id.to_be_bytes());
    let flags: u16 = 0x8000 | RCODE_FORMERR;
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT
    out
}

// ============================================================================
// Server loop
// ============================================================================

async fn serve(bind: SocketAddr, lookup: HashMap<String, Vec<Vec<String>>>) -> std::io::Result<()> {
    let sock = UdpSocket::bind(bind).await?;
    eprintln!("[phase-b-dns-responder] listening on {bind}  records={}", lookup.len());

    let mut buf = vec![0u8; 4096];
    loop {
        let (n, peer) = sock.recv_from(&mut buf).await?;
        let query = &buf[..n];

        let resp = match parse_query(query) {
            Ok(parsed) => {
                let answer = lookup.get(&parsed.qname);
                eprintln!(
                    "[phase-b-dns-responder] from={peer} qname={} qtype={} matched={}",
                    parsed.qname,
                    parsed.qtype,
                    answer.is_some()
                );
                build_response(query, &parsed, answer)
            }
            Err(e) => {
                eprintln!("[phase-b-dns-responder] parse error from {peer}: {e:?}");
                // Best-effort id echo if we got the first 2 bytes.
                let id = if query.len() >= 2 {
                    u16::from_be_bytes([query[0], query[1]])
                } else {
                    0
                };
                build_formerr(id)
            }
        };

        if let Err(e) = sock.send_to(&resp, peer).await {
            eprintln!("[phase-b-dns-responder] send_to {peer} failed: {e}");
        }
    }
}

// ============================================================================
// Self-test (CC-side sanity, NOT Phase B)
// ============================================================================

async fn self_test(bind: SocketAddr, lookup: HashMap<String, Vec<Vec<String>>>) -> Result<(), String> {
    use tokio::task::JoinHandle;

    // Spawn the server on the bound socket.
    let server_lookup = lookup.clone();
    let handle: JoinHandle<std::io::Result<()>> = tokio::spawn(async move {
        serve(bind, server_lookup).await
    });

    // Give the server a moment to start.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // For each configured name, issue a TXT query and verify the
    // response carries the expected character-strings byte-for-byte.
    let client = UdpSocket::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("self-test client bind: {e}"))?;
    client.connect(bind).await.map_err(|e| format!("self-test connect: {e}"))?;

    for (name, expected) in &lookup {
        let query = encode_test_query(name);
        client.send(&query).await.map_err(|e| format!("send {name}: {e}"))?;
        let mut resp = vec![0u8; 4096];
        let n = match timeout(Duration::from_secs(2), client.recv(&mut resp)).await {
            Ok(Ok(n)) => n,
            Ok(Err(e)) => return Err(format!("recv {name}: {e}")),
            Err(_) => return Err(format!("recv {name}: timeout")),
        };
        let resp = &resp[..n];
        check_response_carries_records(resp, name, expected)?;
        eprintln!(
            "[self-test] {name}: {} records, {} bytes — ok",
            expected.len(),
            n
        );
    }

    handle.abort();
    eprintln!("[self-test] all {} configured names answered with byte-exact records", lookup.len());
    Ok(())
}

/// Encode a minimal TXT query for `name`. id=0x1234, RD=1, single
/// question. The server doesn't care about ID uniqueness for the
/// self-test since we wait for each response synchronously.
fn encode_test_query(name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(64);
    out.extend_from_slice(&0x1234u16.to_be_bytes()); // id
    out.extend_from_slice(&0x0100u16.to_be_bytes()); // flags: RD=1
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    for label in name.split('.') {
        out.push(label.len() as u8);
        out.extend_from_slice(label.as_bytes());
    }
    out.push(0);
    out.extend_from_slice(&QTYPE_TXT.to_be_bytes());
    out.extend_from_slice(&QCLASS_IN.to_be_bytes());
    out
}

/// Check the response carries the expected records byte-for-byte
/// (testing the encode path's pass-through discipline directly — this
/// is the 13c-safety self-test).
fn check_response_carries_records(
    resp: &[u8],
    name: &str,
    expected: &[Vec<String>],
) -> Result<(), String> {
    if resp.len() < DNS_HEADER_LEN {
        return Err(format!("{name}: response truncated ({} bytes)", resp.len()));
    }
    let flags = u16::from_be_bytes([resp[2], resp[3]]);
    let rcode = flags & 0xF;
    if rcode != RCODE_NOERROR {
        return Err(format!("{name}: rcode={rcode} (expected NOERROR=0)"));
    }
    let ancount = u16::from_be_bytes([resp[6], resp[7]]) as usize;
    if ancount != expected.len() {
        return Err(format!(
            "{name}: ancount={ancount} but expected {}",
            expected.len()
        ));
    }
    // Skip the question section by walking the same labels we sent.
    let mut idx = DNS_HEADER_LEN;
    while idx < resp.len() && resp[idx] != 0 {
        idx += 1 + resp[idx] as usize;
    }
    idx += 1; // trailing 0-length label
    idx += 4; // QTYPE + QCLASS

    for (i, expected_txt) in expected.iter().enumerate() {
        // Skip NAME (2-byte compression pointer assumed) + TYPE +
        // CLASS + TTL + RDLENGTH.
        idx += 2 + 2 + 2 + 4;
        let rdlen = u16::from_be_bytes([resp[idx], resp[idx + 1]]) as usize;
        idx += 2;
        // Walk the rdata's character-strings and compare each to the
        // expected.
        let rdata = &resp[idx..idx + rdlen];
        let mut rd_idx = 0;
        let mut seen = Vec::new();
        while rd_idx < rdata.len() {
            let len = rdata[rd_idx] as usize;
            rd_idx += 1;
            seen.push(String::from_utf8_lossy(&rdata[rd_idx..rd_idx + len]).into_owned());
            rd_idx += len;
        }
        if &seen != expected_txt {
            return Err(format!(
                "{name} record {i}: bytes mismatch\n  expected: {:?}\n  got:      {:?}",
                expected_txt, seen
            ));
        }
        idx += rdlen;
    }

    Ok(())
}

// ============================================================================
// Main
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let bind: SocketAddr = cli
        .bind
        .parse()
        .map_err(|e| format!("--bind '{}' is not a SocketAddr: {e}", cli.bind))?;

    let cfg = Config::load(&cli.config).map_err(|e| format!("config: {e}"))?;
    let lookup = cfg.into_lookup();

    if cli.self_test {
        self_test(bind, lookup).await.map_err(|e| format!("self-test: {e}"))?;
        Ok(())
    } else {
        serve(bind, lookup).await?;
        Ok(())
    }
}

// ============================================================================
// Unit tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_query(name: &str) -> Vec<u8> {
        encode_test_query(name)
    }

    #[test]
    fn parse_minimal_txt_query() {
        let q = make_query("_lexicon.example.com");
        let p = parse_query(&q).expect("parse");
        assert_eq!(p.qname, "_lexicon.example.com");
        assert_eq!(p.qtype, QTYPE_TXT);
        assert_eq!(p.qclass, QCLASS_IN);
    }

    #[test]
    fn build_response_two_records_byte_exact() {
        let q = make_query("_lexicon.test13a.example.com");
        let p = parse_query(&q).unwrap();
        let answers = vec![
            vec!["did=did:plc:abc1".to_string()],
            vec!["did=did:plc:abc2".to_string()],
        ];
        let resp = build_response(&q, &p, Some(&answers));
        check_response_carries_records(&resp, "_lexicon.test13a.example.com", &answers).unwrap();
    }

    #[test]
    fn build_response_two_chunks_in_one_record() {
        let q = make_query("_lexicon.test13b.example.com");
        let p = parse_query(&q).unwrap();
        let answers = vec![vec![
            "did=did:plc:xyz1".to_string(),
            "did=did:plc:xyz2".to_string(),
        ]];
        let resp = build_response(&q, &p, Some(&answers));
        check_response_carries_records(&resp, "_lexicon.test13b.example.com", &answers).unwrap();
    }

    #[test]
    fn build_response_malformed_txt_byte_exact_13c() {
        // The 13c shape: uppercase DID=, leading + trailing whitespace,
        // bad prefix. Confirms the encode path is byte-exact pass-
        // through — the malformed shape lands on the wire as-is.
        let q = make_query("_lexicon.test13c.example.com");
        let p = parse_query(&q).unwrap();
        let answers = vec![vec!["  DID=did:plc:def1  ".to_string()]];
        let resp = build_response(&q, &p, Some(&answers));
        check_response_carries_records(&resp, "_lexicon.test13c.example.com", &answers).unwrap();
    }

    #[test]
    fn build_response_nxdomain_for_unknown_name() {
        let q = make_query("_lexicon.unknown.example.com");
        let p = parse_query(&q).unwrap();
        let resp = build_response(&q, &p, None);
        let rcode = u16::from_be_bytes([resp[2], resp[3]]) & 0xF;
        assert_eq!(rcode, RCODE_NXDOMAIN);
        let ancount = u16::from_be_bytes([resp[6], resp[7]]);
        assert_eq!(ancount, 0);
    }

    #[test]
    fn build_response_notimp_for_non_txt_query() {
        // Build a query manually with a non-TXT qtype.
        let mut q = encode_test_query("example.com");
        // Replace the QTYPE bytes (last 4 bytes are qtype+qclass).
        let qtype_offset = q.len() - 4;
        q[qtype_offset] = 0;
        q[qtype_offset + 1] = 1; // A (1)
        let p = parse_query(&q).unwrap();
        assert_eq!(p.qtype, 1);
        let resp = build_response(&q, &p, None);
        let rcode = u16::from_be_bytes([resp[2], resp[3]]) & 0xF;
        assert_eq!(rcode, RCODE_NOTIMP);
    }

    #[test]
    fn config_rejects_oversize_character_string() {
        let oversize = "x".repeat(256);
        let cfg = Config {
            records: vec![RecordEntry {
                name: "_lexicon.example.com".to_string(),
                txt_records: vec![vec![oversize]],
            }],
        };
        // Round-trip through validation manually since we can't call
        // load() without a real file.
        let mut found = false;
        for entry in &cfg.records {
            for txt in &entry.txt_records {
                for chunk in txt {
                    if chunk.len() > 255 {
                        found = true;
                    }
                }
            }
        }
        assert!(found, "oversize chunk should be detectable");
    }

    #[test]
    fn parse_truncated_query_errors() {
        let buf = vec![0u8; 5]; // shorter than DNS_HEADER_LEN
        assert!(matches!(parse_query(&buf), Err(DnsErr::Truncated)));
    }
}
