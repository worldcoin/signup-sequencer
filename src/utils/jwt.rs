use chrono::{DateTime, Utc};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use thiserror::Error;

use crate::config::AuthorizedKey;

#[derive(Clone)]
struct LoadedKey {
    key: DecodingKey,
    expires_at: Option<DateTime<Utc>>,
}

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
    keys: HashMap<String, Vec<LoadedKey>>,
}

impl JwtValidator {
    /// Creates a new JWT validator from a map of named authorized keys.
    ///
    /// # Errors
    /// Returns an error if any of the provided PEM keys are invalid.
    pub fn new(authorized_keys: &HashMap<String, Vec<AuthorizedKey>>) -> Result<Self, JwtError> {
        let mut keys = HashMap::new();
        for (name, entries) in authorized_keys {
            let mut loaded = Vec::new();
            for entry in entries {
                loaded.push(LoadedKey {
                    key: DecodingKey::from_ec_pem(entry.pem.as_bytes())?,
                    expires_at: entry.expires_at,
                });
            }
            keys.insert(name.clone(), loaded);
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

        let now = Utc::now();
        for (name, keys) in &self.keys {
            for loaded in keys {
                if let Ok(token_data) = decode::<Claims>(token, &loaded.key, &validation) {
                    // Signature valid - now check sub matches key name
                    if token_data.claims.sub != *name {
                        continue;
                    }
                    if loaded.expires_at.is_some_and(|exp| exp <= now) {
                        tracing::warn!(key = %name, "JWT validated against an expired authorized key");
                        return Err(JwtError::InvalidToken);
                    }
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
        keys.insert(
            "test_key".to_string(),
            vec![AuthorizedKey {
                pem: public_pem,
                expires_at: None,
            }],
        );

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
        keys.insert(
            "test_key".to_string(),
            vec![AuthorizedKey {
                pem: public_pem,
                expires_at: None,
            }],
        );

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
        keys.insert(
            "test_key".to_string(),
            vec![AuthorizedKey {
                pem: public_pem,
                expires_at: None,
            }],
        );

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
        keys.insert(
            "test_key".to_string(),
            vec![AuthorizedKey {
                pem: public_pem,
                expires_at: None,
            }],
        );

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
        keys.insert(
            "test_key".to_string(),
            vec![AuthorizedKey {
                pem: public_pem2,
                expires_at: None,
            }],
        );

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
        keys.insert(
            "test_key".to_string(),
            vec![AuthorizedKey {
                pem: public_pem,
                expires_at: None,
            }],
        );

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        let result = validator.validate("not.a.valid.token");
        assert!(matches!(result, Err(JwtError::InvalidToken)));
    }

    #[test]
    fn multiple_keys_validates_by_sub() {
        let (private_pem1, public_pem1) = generate_es256_keypair();
        let (private_pem2, public_pem2) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert(
            "key1".to_string(),
            vec![AuthorizedKey {
                pem: public_pem1,
                expires_at: None,
            }],
        );
        keys.insert(
            "key2".to_string(),
            vec![AuthorizedKey {
                pem: public_pem2,
                expires_at: None,
            }],
        );

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
    fn multiple_pems_per_key_both_validate() {
        let (private_pem1, public_pem1) = generate_es256_keypair();
        let (private_pem2, public_pem2) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert(
            "test_key".to_string(),
            vec![
                AuthorizedKey {
                    pem: public_pem1,
                    expires_at: None,
                },
                AuthorizedKey {
                    pem: public_pem2,
                    expires_at: None,
                },
            ],
        );

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        for private_pem in [&private_pem1, &private_pem2] {
            let claims = Claims::new("test_key", future_exp());
            let token = sign_jwt(private_pem, &claims);
            let result = validator.validate(&token);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().sub, "test_key");
        }
    }

    #[test]
    fn multiple_pems_per_key_wrong_sub_still_rejected() {
        let (private_pem1, public_pem1) = generate_es256_keypair();
        let (_private_pem2, public_pem2) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert(
            "key1".to_string(),
            vec![
                AuthorizedKey {
                    pem: public_pem1,
                    expires_at: None,
                },
                AuthorizedKey {
                    pem: public_pem2,
                    expires_at: None,
                },
            ],
        );

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        // Token signed with key1's private key but claiming sub=key2 — must be rejected
        let claims = Claims::new("key2", future_exp());
        let token = sign_jwt(&private_pem1, &claims);
        assert!(matches!(
            validator.validate(&token),
            Err(JwtError::InvalidToken)
        ));
    }

    #[test]
    fn key_with_future_expiry_accepts_valid_token() {
        let (private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert(
            "test_key".to_string(),
            vec![AuthorizedKey {
                pem: public_pem,
                expires_at: Some(Utc::now() + chrono::Duration::hours(1)),
            }],
        );

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        let claims = Claims::new("test_key", future_exp());
        let token = sign_jwt(&private_pem, &claims);

        assert!(validator.validate(&token).is_ok());
    }

    #[test]
    fn key_with_past_expiry_rejects_valid_token() {
        let (private_pem, public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert(
            "test_key".to_string(),
            vec![AuthorizedKey {
                pem: public_pem,
                expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
            }],
        );

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        let claims = Claims::new("test_key", future_exp());
        let token = sign_jwt(&private_pem, &claims);

        assert!(matches!(
            validator.validate(&token),
            Err(JwtError::InvalidToken)
        ));
    }

    #[test]
    fn new_key_accepted_when_old_key_expired() {
        let (old_private_pem, old_public_pem) = generate_es256_keypair();
        let (new_private_pem, new_public_pem) = generate_es256_keypair();

        let mut keys = HashMap::new();
        keys.insert(
            "test_key".to_string(),
            vec![
                AuthorizedKey {
                    pem: old_public_pem,
                    expires_at: Some(Utc::now() - chrono::Duration::hours(1)),
                },
                AuthorizedKey {
                    pem: new_public_pem,
                    expires_at: None,
                },
            ],
        );

        let validator = JwtValidator::new(&keys).expect("Failed to create validator");

        // Token signed with new key — accepted
        let claims = Claims::new("test_key", future_exp());
        let token = sign_jwt(&new_private_pem, &claims);
        assert!(validator.validate(&token).is_ok());

        // Token signed with old (expired) key — rejected with warning
        let old_token = sign_jwt(&old_private_pem, &claims);
        assert!(matches!(
            validator.validate(&old_token),
            Err(JwtError::InvalidToken)
        ));
    }

    #[test]
    fn has_keys_returns_correct_value() {
        let empty_keys = HashMap::new();
        let validator_empty = JwtValidator::new(&empty_keys).expect("Failed to create validator");
        assert!(!validator_empty.has_keys());

        let (_, public_pem) = generate_es256_keypair();
        let mut keys = HashMap::new();
        keys.insert(
            "test".to_string(),
            vec![AuthorizedKey {
                pem: public_pem,
                expires_at: None,
            }],
        );
        let validator_with_keys = JwtValidator::new(&keys).expect("Failed to create validator");
        assert!(validator_with_keys.has_keys());
    }
}
