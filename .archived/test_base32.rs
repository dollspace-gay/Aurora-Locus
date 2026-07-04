fn main() {
    let data = b"hello";
    
    // Try base32 crate
    let encoded = base32::encode(base32::Alphabet::Rfc4648 { padding: false }, data);
    println!("Encoded: {}", encoded);
    println!("Lowercase: {}", encoded.to_lowercase());
}
