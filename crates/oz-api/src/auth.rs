use std::time::{Duration, Instant};

use cognitoauth::cognito_srp_auth::{auth, CognitoAuthInput};
use hyper::http::HeaderValue;
use hyper::HeaderMap;

use crate::error::Error;

// Same for every project, taken from here: https://docs.openzeppelin.com/defender/api-auth
const CLIENT_ID: &str = "1bpd19lcr33qvg5cr3oi79rdap";
const POOL_ID: &str = "us-west-2_iLmIggsiy";

#[derive(Clone, Debug)]
pub struct ExpiringHeaders {
    pub headers:         HeaderMap,
    /// The timestamp at which the headers will expire in seconds
    pub expiration_time: Instant,
}

impl ExpiringHeaders {
    pub fn empty() -> Self {
        Self {
            headers:         HeaderMap::new(),
            expiration_time: Instant::now(),
        }
    }

    pub async fn refresh(api_key: &str, api_secret: &str) -> Result<ExpiringHeaders, Error> {
        let now = Instant::now();

        let input = CognitoAuthInput {
            client_id:     CLIENT_ID.to_string(),
            pool_id:       POOL_ID.to_string(),
            username:      api_key.to_string(),
            password:      api_secret.to_string(),
            mfa:           None,
            client_secret: None,
        };

        let res = auth(input).await?.ok_or(Error::Unauthorized)?;

        let access_token = res.access_token().ok_or(Error::Unauthorized)?;

        let mut auth_value = HeaderValue::from_str(&format!("Bearer {access_token}"))?;
        auth_value.set_sensitive(true);

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::AUTHORIZATION, auth_value);
        headers.insert("X-Api-Key", HeaderValue::from_str(api_key)?);

        let expires_in = Duration::from_secs(res.expires_in() as u64);
        let expiration_time = now + expires_in;

        Ok(Self {
            headers,
            expiration_time,
        })
    }

    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.headers(self.headers.clone())
    }
}
