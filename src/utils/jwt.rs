use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum JwtError {
    #[error("Invalid token")]
    InvalidToken,
    #[error("Invalid key: {0}")]
    InvalidKey(#[from] jsonwebtoken::errors::Error),
}

#[derive(Clone)]
pub struct JwtValidator {
    keys: HashMap<String, DecodingKey>,
    require_auth: bool,
}

impl JwtValidator {
    /// Creates a new JWT validator from a map of named PEM public keys.
    ///
    /// # Errors
    /// Returns an error if any of the provided PEM keys are invalid.
    pub fn new(
        pem_keys: &HashMap<String, String>,
        require_auth: bool,
    ) -> Result<Self, JwtError> {
        let mut keys = HashMap::new();
        for (name, pem) in pem_keys {
            let key = DecodingKey::from_ec_pem(pem.as_bytes())?;
            keys.insert(name.clone(), key);
        }
        Ok(Self { keys, require_auth })
    }

    /// Returns whether authentication is required.
    pub fn require_auth(&self) -> bool {
        self.require_auth
    }

    /// Returns whether any keys are configured.
    pub fn has_keys(&self) -> bool {
        !self.keys.is_empty()
    }

    /// Validates a JWT token against all configured keys.
    ///
    /// Returns the name of the key that successfully validated the token.
    ///
    /// # Errors
    /// Returns `JwtError::InvalidToken` if no key validates the token.
    pub fn validate(&self, token: &str) -> Result<String, JwtError> {
        let mut validation = Validation::new(Algorithm::ES256);
        // We don't validate standard claims - the token just needs a valid signature
        validation.validate_exp = false;
        validation.required_spec_claims.clear();

        for (name, key) in &self.keys {
            if decode::<serde_json::Value>(token, key, &validation).is_ok() {
                return Ok(name.clone());
            }
        }
        Err(JwtError::InvalidToken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;

    /// Generates an ES256 key pair for testing.
    /// Returns (private_key_pem, public_key_pem).
    fn generate_es256_keypair() -> (String, String) {
        use std::process::Command;

        // Generate private key
        let private_key_output = Command::new("openssl")
            .args(["ecparam", "-genkey", "-name", "prime256v1", "-noout"])
            .output()
            .expect("Failed to generate private key");

        let private_key_pem = String::from_utf8(private_key_output.stdout)
            .expect("Invalid UTF-8 in private key");

        // Extract public key from private key
        let public_key_output = Command::new("openssl")
            .args(["ec", "-pubout"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("Failed to spawn openssl");

        use std::io::Write;
        public_key_output
            .stdin
            .as_ref()
            .unwrap()
            .write_all(private_key_pem.as_bytes())
            .expect("Failed to write to stdin");

        let output = public_key_output
            .wait_with_output()
            .expect("Failed to extract public key");

        let public_key_pem =
            String::from_utf8(output.stdout).expect("Invalid UTF-8 in public key");

        (private_key_pem, public_key_pem)
    }

    /// Signs a JWT with the given private key and claims.
    fn sign_jwt(private_key_pem: &str, claims: serde_json::Value) -> String {
        let encoding_key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
            .expect("Failed to create encoding key");

        let header = Header::new(Algorithm::ES256);
        encode(&header, &claims, &encoding_key).expect("Failed to encode JWT")
    }

    #[test]
    fn valid_token_returns_key_name() {
        let (private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("test_key".to_string(), public_pem);

        let validator = JwtValidator::new(&keys, true).expect("Failed to create validator");

        let claims = json!({"sub": "user123"});
        let token = sign_jwt(&private_pem, claims);

        let result = validator.validate(&token);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "test_key");
    }

    #[test]
    fn unknown_key_rejected() {
        let (private_pem1, _public_pem1) = generate_es256_keypair();
        let (_private_pem2, public_pem2) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("key2".to_string(), public_pem2);

        let validator = JwtValidator::new(&keys, true).expect("Failed to create validator");

        // Sign with key1, but validator only has key2
        let claims = json!({"sub": "user123"});
        let token = sign_jwt(&private_pem1, claims);

        let result = validator.validate(&token);
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn malformed_token_rejected() {
        let (_private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("test_key".to_string(), public_pem);

        let validator = JwtValidator::new(&keys, true).expect("Failed to create validator");

        let result = validator.validate("not.a.valid.token");
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn multiple_keys_tries_all() {
        let (private_pem1, public_pem1) = generate_es256_keypair();
        let (private_pem2, public_pem2) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("key1".to_string(), public_pem1);
        keys.insert("key2".to_string(), public_pem2);

        let validator = JwtValidator::new(&keys, true).expect("Failed to create validator");

        // Token signed with key1 should match key1
        let claims1 = json!({"sub": "user1"});
        let token1 = sign_jwt(&private_pem1, claims1);
        let result1 = validator.validate(&token1);
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap(), "key1");

        // Token signed with key2 should match key2
        let claims2 = json!({"sub": "user2"});
        let token2 = sign_jwt(&private_pem2, claims2);
        let result2 = validator.validate(&token2);
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap(), "key2");
    }

    #[test]
    fn empty_keys_rejects_all() {
        let keys = HashMap::new();
        let validator =
            JwtValidator::new(&keys, false).expect("Failed to create validator");

        let (private_pem, _public_pem) = generate_es256_keypair();
        let claims = json!({"sub": "user123"});
        let token = sign_jwt(&private_pem, claims);

        let result = validator.validate(&token);
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn require_auth_flag_preserved() {
        let keys = HashMap::new();

        let validator_require =
            JwtValidator::new(&keys, true).expect("Failed to create validator");
        assert!(validator_require.require_auth());

        let validator_no_require =
            JwtValidator::new(&keys, false).expect("Failed to create validator");
        assert!(!validator_no_require.require_auth());
    }

    #[test]
    fn has_keys_returns_correct_value() {
        let empty_keys = HashMap::new();
        let validator_empty =
            JwtValidator::new(&empty_keys, false).expect("Failed to create validator");
        assert!(!validator_empty.has_keys());

        let (_, public_pem) = generate_es256_keypair();
        let mut keys = HashMap::new();
        keys.insert("test".to_string(), public_pem);
        let validator_with_keys =
            JwtValidator::new(&keys, false).expect("Failed to create validator");
        assert!(validator_with_keys.has_keys());
    }
}
