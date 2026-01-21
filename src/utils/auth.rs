//! Unified authentication validator supporting Basic Auth and JWT.

use std::collections::HashMap;

use axum::extract::Request;
use base64::prelude::*;
use thiserror::Error;

use crate::config::AuthMode;
use crate::utils::jwt::JwtValidator;

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("Invalid JWT key: {0}")]
    InvalidJwtKey(#[from] crate::utils::jwt::JwtError),
}

/// Result of authentication validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthResult {
    /// Request is allowed
    Allowed,
    /// Request is allowed but with a warning message
    AllowedWithWarning(String),
    /// Request is denied with a reason
    Denied(String),
}

/// Unified authentication validator.
#[derive(Clone)]
pub struct AuthValidator {
    mode: AuthMode,
    basic_credentials: HashMap<String, String>, // username -> password
    jwt_validator: Option<JwtValidator>,
}

impl AuthValidator {
    /// Creates a new AuthValidator.
    ///
    /// # Errors
    /// Returns an error if any JWT keys are invalid.
    pub fn new(
        mode: AuthMode,
        basic_credentials: HashMap<String, String>,
        jwt_validator: Option<JwtValidator>,
    ) -> Result<Self, AuthError> {
        Ok(Self {
            mode,
            basic_credentials,
            jwt_validator,
        })
    }

    /// Returns the authentication mode.
    pub fn mode(&self) -> &AuthMode {
        &self.mode
    }

    /// Validates a request based on the configured auth mode.
    pub fn validate(&self, request: &Request) -> AuthResult {
        match &self.mode {
            AuthMode::Disabled => AuthResult::Allowed,
            AuthMode::BasicOnly => self.validate_basic_only(request),
            AuthMode::BasicWithSoftJwt => self.validate_basic_with_soft_jwt(request),
            AuthMode::JwtOnly => self.validate_jwt_only(request),
        }
    }

    /// BasicOnly: Requires valid Basic Auth, ignores Bearer token.
    fn validate_basic_only(&self, request: &Request) -> AuthResult {
        match self.extract_and_validate_basic_auth(request) {
            Some(username) => {
                tracing::info!(user = %username, "Basic auth validated");
                AuthResult::Allowed
            }
            None => AuthResult::Denied("Invalid or missing Basic Auth credentials".to_string()),
        }
    }

    /// BasicWithSoftJwt: Requires valid Basic Auth + soft-validates JWT.
    /// - If Basic Auth fails: deny
    /// - If Basic Auth passes but no Bearer token: allow with warning
    /// - If Basic Auth passes and Bearer token is invalid: deny
    /// - If Basic Auth passes and Bearer token is valid: allow
    fn validate_basic_with_soft_jwt(&self, request: &Request) -> AuthResult {
        // First validate Basic Auth
        let basic_username = match self.extract_and_validate_basic_auth(request) {
            Some(username) => username,
            None => {
                return AuthResult::Denied(
                    "Invalid or missing Basic Auth credentials".to_string(),
                );
            }
        };

        // Now check Bearer token
        let bearer_token = self.extract_bearer_token(request);

        match bearer_token {
            Some(token) => {
                // Token present - validate it
                match &self.jwt_validator {
                    Some(validator) => match validator.validate(token) {
                        Ok(key_name) => {
                            tracing::info!(
                                basic_user = %basic_username,
                                jwt_key = %key_name,
                                "Basic + JWT auth validated"
                            );
                            AuthResult::Allowed
                        }
                        Err(e) => AuthResult::Denied(format!("Invalid JWT token: {}", e)),
                    },
                    None => {
                        // No JWT validator configured - misconfiguration, reject
                        AuthResult::Denied(
                            "JWT token provided but no validator configured".to_string(),
                        )
                    }
                }
            }
            None => {
                // No Bearer token - warn but allow
                tracing::info!(basic_user = %basic_username, "Basic auth validated");
                AuthResult::AllowedWithWarning(format!(
                    "Basic auth validated for user '{}' but no JWT token provided",
                    basic_username
                ))
            }
        }
    }

    /// JwtOnly: Requires valid Bearer token, ignores Basic Auth.
    fn validate_jwt_only(&self, request: &Request) -> AuthResult {
        let token = match self.extract_bearer_token(request) {
            Some(token) => token,
            None => return AuthResult::Denied("Missing Authorization Bearer token".to_string()),
        };

        match &self.jwt_validator {
            Some(validator) => match validator.validate(token) {
                Ok(key_name) => {
                    tracing::info!(jwt_key = %key_name, "JWT auth validated");
                    AuthResult::Allowed
                }
                Err(e) => AuthResult::Denied(format!("Invalid JWT token: {}", e)),
            },
            None => {
                // No keys configured - this is a misconfiguration
                AuthResult::Denied("JWT authentication enabled but no keys configured".to_string())
            }
        }
    }

    /// Extracts and validates Basic Auth credentials.
    /// Returns the username if valid, None otherwise.
    fn extract_and_validate_basic_auth(&self, request: &Request) -> Option<String> {
        let auth_header = request
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())?;

        let encoded = auth_header.strip_prefix("Basic ")?;
        let decoded = BASE64_STANDARD.decode(encoded).ok()?;
        let credentials = String::from_utf8(decoded).ok()?;

        let (username, password) = credentials.split_once(':')?;

        // Validate against configured credentials
        if let Some(expected_password) = self.basic_credentials.get(username) {
            if expected_password == password {
                return Some(username.to_string());
            }
        }

        None
    }

    /// Extracts Bearer token from Authorization header.
    fn extract_bearer_token<'a>(&self, request: &'a Request) -> Option<&'a str> {
        request
            .headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request as HttpRequest;

    fn make_request_with_headers(
        basic_auth: Option<(&str, &str)>,
        bearer_token: Option<&str>,
    ) -> Request {
        let mut builder = HttpRequest::builder().uri("/test").method("GET");

        if let Some((username, password)) = basic_auth {
            let credentials = format!("{}:{}", username, password);
            let encoded = BASE64_STANDARD.encode(credentials.as_bytes());
            builder = builder.header("Authorization", format!("Basic {}", encoded));
        } else if let Some(token) = bearer_token {
            builder = builder.header("Authorization", format!("Bearer {}", token));
        }

        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn disabled_mode_allows_all() {
        let validator =
            AuthValidator::new(AuthMode::Disabled, HashMap::new(), None).unwrap();

        let request = make_request_with_headers(None, None);
        assert_eq!(validator.validate(&request), AuthResult::Allowed);
    }

    #[test]
    fn basic_only_requires_valid_basic_auth() {
        let mut creds = HashMap::new();
        creds.insert("user".to_string(), "pass".to_string());

        let validator =
            AuthValidator::new(AuthMode::BasicOnly, creds, None).unwrap();

        // No auth - denied
        let request = make_request_with_headers(None, None);
        assert!(matches!(validator.validate(&request), AuthResult::Denied(_)));

        // Wrong credentials - denied
        let request = make_request_with_headers(Some(("user", "wrong")), None);
        assert!(matches!(validator.validate(&request), AuthResult::Denied(_)));

        // Correct credentials - allowed
        let request = make_request_with_headers(Some(("user", "pass")), None);
        assert_eq!(validator.validate(&request), AuthResult::Allowed);
    }

    #[test]
    fn basic_only_ignores_bearer_token() {
        let mut creds = HashMap::new();
        creds.insert("user".to_string(), "pass".to_string());

        let validator =
            AuthValidator::new(AuthMode::BasicOnly, creds, None).unwrap();

        // Bearer token without basic auth - denied (basic auth required)
        let request = make_request_with_headers(None, Some("some.jwt.token"));
        assert!(matches!(validator.validate(&request), AuthResult::Denied(_)));
    }

    #[test]
    fn jwt_only_requires_valid_bearer() {
        // Note: We can't easily create a valid JWT in unit tests without the openssl commands
        // So we test that missing/invalid tokens are rejected

        let validator =
            AuthValidator::new(AuthMode::JwtOnly, HashMap::new(), None).unwrap();

        // No token - denied
        let request = make_request_with_headers(None, None);
        assert!(matches!(validator.validate(&request), AuthResult::Denied(_)));

        // With token but no keys configured - denied
        let request = make_request_with_headers(None, Some("some.jwt.token"));
        assert!(matches!(validator.validate(&request), AuthResult::Denied(_)));
    }

    #[test]
    fn jwt_only_ignores_basic_auth() {
        let mut creds = HashMap::new();
        creds.insert("user".to_string(), "pass".to_string());

        let validator =
            AuthValidator::new(AuthMode::JwtOnly, creds, None).unwrap();

        // Basic auth without bearer - denied (JWT required)
        let request = make_request_with_headers(Some(("user", "pass")), None);
        assert!(matches!(validator.validate(&request), AuthResult::Denied(_)));
    }

    #[test]
    fn basic_with_soft_jwt_requires_basic_auth() {
        let mut creds = HashMap::new();
        creds.insert("user".to_string(), "pass".to_string());

        let validator =
            AuthValidator::new(AuthMode::BasicWithSoftJwt, creds, None).unwrap();

        // No auth - denied
        let request = make_request_with_headers(None, None);
        assert!(matches!(validator.validate(&request), AuthResult::Denied(_)));

        // Only bearer token - denied (basic auth required)
        let request = make_request_with_headers(None, Some("some.jwt.token"));
        assert!(matches!(validator.validate(&request), AuthResult::Denied(_)));
    }

    #[test]
    fn basic_with_soft_jwt_warns_on_missing_bearer() {
        let mut creds = HashMap::new();
        creds.insert("user".to_string(), "pass".to_string());

        let validator =
            AuthValidator::new(AuthMode::BasicWithSoftJwt, creds, None).unwrap();

        // Basic auth only - allowed with warning
        let request = make_request_with_headers(Some(("user", "pass")), None);
        assert!(matches!(
            validator.validate(&request),
            AuthResult::AllowedWithWarning(_)
        ));
    }
}
