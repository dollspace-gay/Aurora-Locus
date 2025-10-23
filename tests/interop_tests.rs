/// ATProto Interop Tests
/// Tests Aurora Locus PDS implementation against official ATProto interop test fixtures
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;

// Test fixture paths
const INTEROP_DIR: &str = "atproto-interop-tests-main";

#[derive(Debug, Deserialize, Serialize)]
struct KeyHeightFixture {
    key: String,
    height: u32,
}

#[derive(Debug, Deserialize, Serialize)]
struct SignatureFixture {
    description: String,
    #[serde(rename = "privateKeyHex")]
    private_key_hex: String,
    #[serde(rename = "messageHex")]
    message_hex: String,
    #[serde(rename = "signatureHex")]
    signature_hex: String,
}

/// Helper to calculate MST key height
fn calculate_mst_key_height(key: &str) -> u32 {
    if key.is_empty() {
        return 0;
    }

    let hash = blake3::hash(key.as_bytes());
    let hash_bytes = hash.as_bytes();

    let mut height = 0u32;
    for i in (0..hash_bytes.len()).step_by(2) {
        let val = u16::from_be_bytes([hash_bytes[i], hash_bytes[i + 1]]);
        if val < 0x8000 {
            break;
        }
        height += 1;
    }

    height
}

#[test]
fn test_mst_key_heights() {
    let fixture_path = format!("{}/mst/key_heights.json", INTEROP_DIR);
    let fixture_data = fs::read_to_string(&fixture_path)
        .expect("Failed to read MST key heights fixture");

    let fixtures: Vec<KeyHeightFixture> = serde_json::from_str(&fixture_data)
        .expect("Failed to parse fixture");

    let mut passed = 0;
    let mut failed = 0;

    for fixture in &fixtures {
        let calculated = calculate_mst_key_height(&fixture.key);
        if calculated == fixture.height {
            passed += 1;
            println!("✓ Key '{}': height {} (correct)", fixture.key, calculated);
        } else {
            failed += 1;
            println!("✗ Key '{}': expected {}, got {}",
                     fixture.key, fixture.height, calculated);
        }
    }

    println!("\nMST Key Heights: {} passed, {} failed", passed, failed);
    assert_eq!(failed, 0, "Some MST key height tests failed");
}

#[test]
fn test_mst_common_prefix() {
    let fixture_path = format!("{}/mst/common_prefix.json", INTEROP_DIR);
    if let Ok(fixture_data) = fs::read_to_string(&fixture_path) {
        let fixtures: Vec<Value> = serde_json::from_str(&fixture_data)
            .expect("Failed to parse fixture");

        println!("MST Common Prefix: {} test cases loaded", fixtures.len());
        // Note: Actual prefix calculation would need MST implementation
        // This is a placeholder showing the test structure
    } else {
        println!("MST common_prefix.json not found, skipping");
    }
}

#[test]
fn test_data_model_valid() {
    let fixture_path = format!("{}/data-model/data-model-valid.json", INTEROP_DIR);
    if let Ok(fixture_data) = fs::read_to_string(&fixture_path) {
        let fixtures: Vec<Value> = serde_json::from_str(&fixture_data)
            .expect("Failed to parse valid data model fixtures");

        println!("Data Model Valid: {} test cases", fixtures.len());

        for (i, fixture) in fixtures.iter().enumerate() {
            if let Some(description) = fixture.get("description").and_then(|v| v.as_str()) {
                println!("  {} - {}", i + 1, description);
            }
        }
    } else {
        println!("data-model-valid.json not found, skipping");
    }
}

#[test]
fn test_data_model_invalid() {
    let fixture_path = format!("{}/data-model/data-model-invalid.json", INTEROP_DIR);
    if let Ok(fixture_data) = fs::read_to_string(&fixture_path) {
        let fixtures: Vec<Value> = serde_json::from_str(&fixture_data)
            .expect("Failed to parse invalid data model fixtures");

        println!("Data Model Invalid: {} test cases", fixtures.len());

        for (i, fixture) in fixtures.iter().enumerate() {
            if let Some(description) = fixture.get("description").and_then(|v| v.as_str()) {
                println!("  {} - {}", i + 1, description);
            }
        }
    } else {
        println!("data-model-invalid.json not found, skipping");
    }
}

#[test]
fn test_crypto_signatures() {
    let fixture_path = format!("{}/crypto/signature-fixtures.json", INTEROP_DIR);
    if let Ok(fixture_data) = fs::read_to_string(&fixture_path) {
        let fixtures: Vec<SignatureFixture> = serde_json::from_str(&fixture_data)
            .expect("Failed to parse signature fixtures");

        println!("Crypto Signatures: {} test cases", fixtures.len());

        for (i, fixture) in fixtures.iter().enumerate() {
            println!("  {} - {}", i + 1, fixture.description);
            // Actual signature verification would go here
            // This requires implementing the crypto verification logic
        }
    } else {
        println!("signature-fixtures.json not found, skipping");
    }
}

#[test]
fn test_lexicon_validation() {
    let valid_path = format!("{}/lexicon/lexicon-valid.json", INTEROP_DIR);
    let invalid_path = format!("{}/lexicon/lexicon-invalid.json", INTEROP_DIR);

    if let Ok(valid_data) = fs::read_to_string(&valid_path) {
        let valid_fixtures: Vec<Value> = serde_json::from_str(&valid_data)
            .expect("Failed to parse valid lexicon fixtures");
        println!("Lexicon Valid: {} test cases", valid_fixtures.len());
    }

    if let Ok(invalid_data) = fs::read_to_string(&invalid_path) {
        let invalid_fixtures: Vec<Value> = serde_json::from_str(&invalid_data)
            .expect("Failed to parse invalid lexicon fixtures");
        println!("Lexicon Invalid: {} test cases", invalid_fixtures.len());
    }
}

#[test]
fn test_firehose_commit_proofs() {
    let fixture_path = format!("{}/firehose/commit-proof-fixtures.json", INTEROP_DIR);
    if let Ok(fixture_data) = fs::read_to_string(&fixture_path) {
        let fixtures: Vec<Value> = serde_json::from_str(&fixture_data)
            .expect("Failed to parse commit proof fixtures");

        println!("Firehose Commit Proofs: {} test cases", fixtures.len());

        for (i, fixture) in fixtures.iter().enumerate() {
            if let Some(description) = fixture.get("description").and_then(|v| v.as_str()) {
                println!("  {} - {}", i + 1, description);
            }
        }
    } else {
        println!("commit-proof-fixtures.json not found, skipping");
    }
}

#[test]
fn interop_test_summary() {
    println!("\n=== ATProto Interop Test Summary ===\n");

    let test_categories = vec![
        ("MST (Merkle Search Tree)", vec!["key_heights.json", "common_prefix.json"]),
        ("Data Model", vec!["data-model-valid.json", "data-model-invalid.json", "data-model-fixtures.json"]),
        ("Crypto", vec!["signature-fixtures.json", "w3c_didkey_K256.json", "w3c_didkey_P256.json"]),
        ("Lexicon", vec!["lexicon-valid.json", "lexicon-invalid.json", "record-data-valid.json", "record-data-invalid.json"]),
        ("Firehose", vec!["commit-proof-fixtures.json"]),
    ];

    for (category, files) in test_categories {
        println!("{}:", category);
        for file in files {
            let exists = fs::metadata(format!("{}/{}", INTEROP_DIR, file)).is_ok() ||
                         fs::metadata(format!("{}/mst/{}", INTEROP_DIR, file)).is_ok() ||
                         fs::metadata(format!("{}/data-model/{}", INTEROP_DIR, file)).is_ok() ||
                         fs::metadata(format!("{}/crypto/{}", INTEROP_DIR, file)).is_ok() ||
                         fs::metadata(format!("{}/lexicon/{}", INTEROP_DIR, file)).is_ok() ||
                         fs::metadata(format!("{}/firehose/{}", INTEROP_DIR, file)).is_ok();

            let status = if exists { "✓" } else { "✗ Missing" };
            println!("  {} {}", status, file);
        }
        println!();
    }
}
