use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

/// JWT claims structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject - must match the key name in authorized_keys
    pub sub: String,
    /// Expiration time
    pub exp: u64,
}

impl Claims {
    pub fn new(sub: impl Into<String>, exp: u64) -> Self {
        Self {
            sub: sub.into(),
            exp,
        }
    }
}

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
}

impl JwtValidator {
    /// Creates a new JWT validator from a map of named PEM public keys.
    ///
    /// # Errors
    /// Returns an error if any of the provided PEM keys are invalid.
    pub fn new(pem_keys: &HashMap<String, String>) -> Result<Self, JwtError> {
        let mut keys = HashMap::new();
        for (name, pem) in pem_keys {
            let key = DecodingKey::from_ec_pem(pem.as_bytes())?;
            keys.insert(name.clone(), key);
        }
        Ok(Self { keys })
    }

    /// Returns whether any keys are configured.
    pub fn has_keys(&self) -> bool {
        !self.keys.is_empty()
    }

    /// Validates a JWT token against configured keys.
    ///
    /// The token's `sub` claim must match a configured key name, and the
    /// signature must be valid for that key.
    ///
    /// Returns the validated claims on success.
    ///
    /// # Errors
    /// Returns `JwtError::InvalidToken` if validation fails.
    pub fn validate(&self, token: &str) -> Result<Claims, JwtError> {
        let validation = Validation::new(Algorithm::ES256);

        for (name, key) in &self.keys {
            if let Ok(token_data) = decode::<Claims>(token, key, &validation) {
                // Signature valid - now check sub matches key name
                if token_data.claims.sub == *name {
                    return Ok(token_data.claims);
                }
            }
        }
        Err(JwtError::InvalidToken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};
    use test_utils::{generate_es256_keypair, sign_jwt};

    fn future_exp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600
    }

    fn past_exp() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 3600
    }

    #[test]
    fn valid_token_returns_claims() {
        let (private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("test_key".to_string(), public_pem);

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        let claims = Claims::new("test_key", future_exp());
        let token = sign_jwt(&private_pem, &claims);

        let result = validator.validate(&token);
        assert!(result.is_ok());
        assert_eq!(result.unwrap().sub, "test_key");
    }

    #[test]
    fn expired_token_rejected() {
        let (private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("test_key".to_string(), public_pem);

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        let claims = Claims::new("test_key", past_exp());
        let token = sign_jwt(&private_pem, &claims);

        let result = validator.validate(&token);
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn missing_exp_rejected() {
        let (private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("test_key".to_string(), public_pem);

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        // Use json! to create a token without exp claim
        let claims = json!({"sub": "test_key"});
        let token = sign_jwt(&private_pem, &claims);

        let result = validator.validate(&token);
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn sub_mismatch_rejected() {
        let (private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("test_key".to_string(), public_pem);

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        // Token has valid signature but sub doesn't match key name
        let claims = Claims::new("wrong_sub", future_exp());
        let token = sign_jwt(&private_pem, &claims);

        let result = validator.validate(&token);
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn wrong_key_rejected() {
        let (private_pem1, _public_pem1) = generate_es256_keypair();
        let (_private_pem2, public_pem2) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("test_key".to_string(), public_pem2);

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        // Token claims correct sub but signed with wrong key
        let claims = Claims::new("test_key", future_exp());
        let token = sign_jwt(&private_pem1, &claims);

        let result = validator.validate(&token);
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn malformed_token_rejected() {
        let (_private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("test_key".to_string(), public_pem);

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        let result = validator.validate("not.a.valid.token");
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn multiple_keys_validates_by_sub() {
        let (private_pem1, public_pem1) = generate_es256_keypair();
        let (private_pem2, public_pem2) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert("key1".to_string(), public_pem1);
        keys.insert("key2".to_string(), public_pem2);

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        // Token with sub=key1 signed with key1
        let claims1 = Claims::new("key1", future_exp());
        let token1 = sign_jwt(&private_pem1, &claims1);
        let result1 = validator.validate(&token1);
        assert!(result1.is_ok());
        assert_eq!(result1.unwrap().sub, "key1");

        // Token with sub=key2 signed with key2
        let claims2 = Claims::new("key2", future_exp());
        let token2 = sign_jwt(&private_pem2, &claims2);
        let result2 = validator.validate(&token2);
        assert!(result2.is_ok());
        assert_eq!(result2.unwrap().sub, "key2");
    }

    #[test]
    fn empty_keys_rejects_all() {
        let keys = HashMap::new();
        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        let (private_pem, _public_pem) = generate_es256_keypair();
        let claims = Claims::new("any_key", future_exp());
        let token = sign_jwt(&private_pem, &claims);

        let result = validator.validate(&token);
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn has_keys_returns_correct_value() {
        let empty_keys = HashMap::new();
        let validator_empty = JwtValidator::new(&empty_keys).expect("Failed to create validator");
        assert!(!validator_empty.has_keys());

        let (_, public_pem) = generate_es256_keypair();
        let mut keys = HashMap::new();
        keys.insert("test".to_string(), public_pem);
        let validator_with_keys = JwtValidator::new(&keys).expect("Failed to create validator");
        assert!(validator_with_keys.has_keys());
    }
}
