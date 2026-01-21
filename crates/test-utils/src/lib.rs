//! Shared test utilities for signup-sequencer.

use std::io::Write;
use std::process::{Command, Stdio};

use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

/// Generates an ES256 key pair for testing.
/// Returns (private_key_pem, public_key_pem).
pub fn generate_es256_keypair() -> (String, String) {
    // Generate private key in SEC1 format
    let sec1_key_output = Command::new("openssl")
        .args(["ecparam", "-genkey", "-name", "prime256v1", "-noout"])
        .output()
        .expect("Failed to generate private key");

    let sec1_key_pem =
        String::from_utf8(sec1_key_output.stdout).expect("Invalid UTF-8 in private key");

    // Convert to PKCS#8 format (required by jsonwebtoken)
    let mut pkcs8_process = Command::new("openssl")
        .args(["pkcs8", "-topk8", "-nocrypt"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn openssl");

    pkcs8_process
        .stdin
        .as_mut()
        .unwrap()
        .write_all(sec1_key_pem.as_bytes())
        .expect("Failed to write to stdin");

    let pkcs8_output = pkcs8_process
        .wait_with_output()
        .expect("Failed to convert to PKCS#8");

    let private_key_pem =
        String::from_utf8(pkcs8_output.stdout).expect("Invalid UTF-8 in private key");

    // Extract public key from the original SEC1 key
    let mut public_key_process = Command::new("openssl")
        .args(["ec", "-pubout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn openssl");

    public_key_process
        .stdin
        .as_mut()
        .unwrap()
        .write_all(sec1_key_pem.as_bytes())
        .expect("Failed to write to stdin");

    let public_output = public_key_process
        .wait_with_output()
        .expect("Failed to extract public key");

    let public_key_pem =
        String::from_utf8(public_output.stdout).expect("Invalid UTF-8 in public key");

    (private_key_pem, public_key_pem)
}

/// Signs a JWT with the given private key and claims.
pub fn sign_jwt(private_key_pem: &str, claims: serde_json::Value) -> String {
    let encoding_key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
        .expect("Failed to create encoding key");

    let header = Header::new(Algorithm::ES256);
    encode(&header, &claims, &encoding_key).expect("Failed to encode JWT")
}
