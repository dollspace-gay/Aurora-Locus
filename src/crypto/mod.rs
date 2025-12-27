//! Cryptography module for PLC operations and key management
//!
//! Handles secp256k1 signing for DID:PLC operations and P-256 keypair generation

pub mod keypair;
pub mod plc;
pub mod plc_client;
pub mod secp256k1;

pub use secp256k1::Secp256k1KeyPair;
