//! Unified authentication validator supporting Basic Auth and JWT.

use std::collections::HashMap;

use axum::extract::Request;
use axum::response::Response;
use base64::prelude::*;

use crate::config::AuthMode;
use crate::utils::jwt::JwtValidator;

use super::jwt::JwtError;

pub type AuthResponseFormatter = fn(msg: String) -> Response;

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
    jwt_validator: JwtValidator,
}

impl AuthValidator {
    /// Creates a new AuthValidator.
    ///
    /// # Errors
    /// Returns an error if `auth_mode` is `JwtOnly` with no keys, or if any JWT keys are invalid.
    pub fn new(
        mode: AuthMode,
        basic_credentials: HashMap<String, String>,
        jwt_keys: &HashMap<String, String>,
    ) -> Result<Self, JwtError> {
        // Build JWT validator if mode requires it
        let jwt_validator = JwtValidator::new(jwt_keys)?;

        Ok(Self {
            mode,
            basic_credentials,
            jwt_validator,
        })
    }

    /// Returns the authentication mode.
    pub fn mode(&self) -> AuthMode {
        self.mode
    }

    /// Validates a request based on the configured auth mode.
    pub fn validate(&self, request: &Request) -> AuthResult {
        match self.mode {
            AuthMode::Disabled => AuthResult::Allowed,
            AuthMode::BasicOnly => self.validate_basic_only(request),
            AuthMode::BasicOrJwt => self.validate_basic_or_jwt(request),
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

    /// BasicOrJwt: Requires valid Basic Auth OR valid JWT (at least one).
    /// - If JWT present and invalid: deny
    /// - If JWT present and valid: allow
    /// - If no JWT but Basic Auth valid: allow with warning
    /// - If neither valid: deny
    fn validate_basic_or_jwt(&self, request: &Request) -> AuthResult {
        // Check JWT first - if present, it must be valid
        let bearer_token = self.extract_bearer_token(request);

        if let Some(token) = bearer_token {
            match self.jwt_validator.validate(token) {
                Ok(claims) => {
                    tracing::info!(jwt_sub = %claims.sub, "JWT auth validated");
                    return AuthResult::Allowed;
                }
                Err(e) => {
                    // JWT present but invalid - deny immediately
                    return AuthResult::Denied(format!("Invalid JWT token: {e}"));
                }
            }
        }

        // No JWT token - check Basic Auth
        if let Some(username) = self.extract_and_validate_basic_auth(request) {
            tracing::warn!(
                basic_user = %username,
                "Request authenticated with Basic Auth only - JWT recommended"
            );
            return AuthResult::AllowedWithWarning(format!(
                "Authenticated with Basic Auth (user '{username}') but JWT is recommended"
            ));
        }

        // Neither JWT nor Basic Auth valid
        AuthResult::Denied("Authentication required: provide valid JWT or Basic Auth".to_string())
    }

    /// JwtOnly: Requires valid Bearer token, ignores Basic Auth.
    fn validate_jwt_only(&self, request: &Request) -> AuthResult {
        let token = match self.extract_bearer_token(request) {
            Some(token) => token,
            None => return AuthResult::Denied("Missing Authorization Bearer token".to_string()),
        };
        match self.jwt_validator.validate(token) {
            Ok(claims) => {
                tracing::info!(jwt_sub = %claims.sub, "JWT auth validated");
                AuthResult::Allowed
            }
            Err(e) => AuthResult::Denied(format!("Invalid JWT token: {e}")),
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
    use maplit::hashmap;

    fn make_request_with_headers(
        basic_auth: Option<(&str, &str)>,
        bearer_token: Option<&str>,
    ) -> Request {
        let mut builder = HttpRequest::builder().uri("/test").method("GET");

        if let Some((username, password)) = basic_auth {
            let credentials = format!("{username}:{password}");
            let encoded = BASE64_STANDARD.encode(credentials.as_bytes());
            builder = builder.header("Authorization", format!("Basic {encoded}"));
        } else if let Some(token) = bearer_token {
            builder = builder.header("Authorization", format!("Bearer {token}"));
        }

        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn disabled_mode_allows_all() {
        let validator = AuthValidator::new(AuthMode::Disabled, hashmap! {}, &hashmap! {}).unwrap();

        let request = make_request_with_headers(None, None);
        assert_eq!(validator.validate(&request), AuthResult::Allowed);
    }

    #[test]
    fn basic_only_requires_valid_basic_auth() {
        let creds = hashmap! { "user".to_string() => "pass".to_string() };
        let validator = AuthValidator::new(AuthMode::BasicOnly, creds, &hashmap! {}).unwrap();

        // No auth - denied
        let request = make_request_with_headers(None, None);
        assert!(matches!(
            validator.validate(&request),
            AuthResult::Denied(_)
        ));

        // Wrong credentials - denied
        let request = make_request_with_headers(Some(("user", "wrong")), None);
        assert!(matches!(
            validator.validate(&request),
            AuthResult::Denied(_)
        ));

        // Correct credentials - allowed
        let request = make_request_with_headers(Some(("user", "pass")), None);
        assert_eq!(validator.validate(&request), AuthResult::Allowed);
    }

    #[test]
    fn basic_only_ignores_bearer_token() {
        let creds = hashmap! { "user".to_string() => "pass".to_string() };
        let validator = AuthValidator::new(AuthMode::BasicOnly, creds, &hashmap! {}).unwrap();

        // Bearer token without basic auth - denied (basic auth required)
        let request = make_request_with_headers(None, Some("some.jwt.token"));
        assert!(matches!(
            validator.validate(&request),
            AuthResult::Denied(_)
        ));
    }

    #[test]
    fn basic_or_jwt_allows_basic_auth_with_warning() {
        let creds = hashmap! { "user".to_string() => "pass".to_string() };
        let validator = AuthValidator::new(AuthMode::BasicOrJwt, creds, &hashmap! {}).unwrap();

        // Basic auth only - allowed with warning
        let request = make_request_with_headers(Some(("user", "pass")), None);
        assert!(matches!(
            validator.validate(&request),
            AuthResult::AllowedWithWarning(_)
        ));
    }

    #[test]
    fn basic_or_jwt_denies_when_neither_present() {
        let creds = hashmap! { "user".to_string() => "pass".to_string() };
        let validator = AuthValidator::new(AuthMode::BasicOrJwt, creds, &hashmap! {}).unwrap();

        // No auth - denied
        let request = make_request_with_headers(None, None);
        assert!(matches!(
            validator.validate(&request),
            AuthResult::Denied(_)
        ));
    }

    #[test]
    fn basic_or_jwt_denies_invalid_jwt() {
        let creds = hashmap! { "user".to_string() => "pass".to_string() };
        let validator = AuthValidator::new(AuthMode::BasicOrJwt, creds, &hashmap! {}).unwrap();

        // Invalid JWT - denied (even without checking basic auth)
        let request = make_request_with_headers(None, Some("invalid.jwt.token"));
        assert!(matches!(
            validator.validate(&request),
            AuthResult::Denied(_)
        ));
    }
}
