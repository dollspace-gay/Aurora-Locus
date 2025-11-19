/// DID Key Generation CLI Command
///
/// Provides command-line tools for generating P-256 keypairs in various formats.

use crate::{crypto::keypair::{KeyFormat, KeyPair}, error::PdsResult};
use std::fs;
use std::str::FromStr;

/// Generate a DID keypair
pub fn generate_did_key(
    format_str: &str,
    output: Option<&str>,
    include_private: bool,
) -> PdsResult<()> {
    println!("════════════════════════════════════════════════════════");
    println!("  DID Key Generation");
    println!("════════════════════════════════════════════════════════");

    // Parse format
    let format = KeyFormat::from_str(format_str)?;

    println!("Format: {:?}", format);
    println!("Type:   {}", if include_private { "Private + Public" } else { "Public Only" });

    // Validate format/private combination
    if format == KeyFormat::Did && include_private {
        return Err(crate::error::PdsError::Validation(
            "DID format does not support private keys. Use --format pem or --format jwk instead.".to_string()
        ));
    }

    println!("\n📝 Generating P-256 keypair...");

    // Generate keypair
    let keypair = KeyPair::generate();

    println!("✓ Keypair generated");

    // Export in requested format
    let exported = keypair.export(format, include_private)?;

    // Output to file or stdout
    if let Some(output_path) = output {
        println!("\n📤 Writing to file: {}", output_path);
        fs::write(output_path, &exported).map_err(|e| {
            crate::error::PdsError::Internal(format!("Failed to write output file: {}", e))
        })?;
        println!("✓ File written successfully");
    } else {
        println!("\n════════════════════════════════════════════════════════");
        println!("  Generated Key");
        println!("════════════════════════════════════════════════════════\n");
        println!("{}", exported);
    }

    println!("\n════════════════════════════════════════════════════════");
    println!("✅ Key generation completed successfully");
    println!("════════════════════════════════════════════════════════\n");

    // Print usage hints
    match format {
        KeyFormat::Did => {
            println!("Usage:");
            println!("  This DID can be used as a verification method in DID documents");
            println!("  or as a rotation key for DID:PLC operations.\n");
        }
        KeyFormat::Pem => {
            if include_private {
                println!("Security Warning:");
                println!("  The private key has been generated. Keep it secure!");
                println!("  Do not share the private key with anyone.\n");
            } else {
                println!("Usage:");
                println!("  This public key can be shared freely for verification purposes.\n");
            }
        }
        KeyFormat::Jwk => {
            if include_private {
                println!("Security Warning:");
                println!("  The JWK contains the private key (d parameter).");
                println!("  Keep it secure and do not share it.\n");
            } else {
                println!("Usage:");
                println!("  This public JWK can be used in OAuth/DPoP or other JWT-based systems.\n");
            }
        }
    }

    Ok(())
}
