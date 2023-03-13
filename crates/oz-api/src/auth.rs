use anyhow::{Context, Result as AnyhowResult};
use cognitoauth::cognito_srp_auth::{auth, CognitoAuthInput};
use hyper::{http::HeaderValue, HeaderMap};

// Same for every project, taken from here: https://docs.openzeppelin.com/defender/api-auth
const CLIENT_ID: &str = "1bpd19lcr33qvg5cr3oi79rdap";
const POOL_ID: &str = "us-west-2_iLmIggsiy";

#[derive(Clone, Debug)]
pub struct ExpiringHeaders {
    pub headers:         HeaderMap,
    /// The timestamp at which the headers will expire in seconds
    pub expiration_time: i64,
}

impl ExpiringHeaders {
    pub async fn refresh(api_key: &str, api_secret: &str) -> AnyhowResult<ExpiringHeaders> {
        let now = chrono::Utc::now().timestamp();

        let input = CognitoAuthInput {
            client_id:     CLIENT_ID.to_string(),
            pool_id:       POOL_ID.to_string(),
            username:      api_key.to_string(),
            password:      api_secret.to_string(),
            mfa:           None,
            client_secret: None,
        };

        let res = auth(input)
            .await
            .context("Auth request failed")?
            .context("Authentication failed")?;

        let access_token = res.access_token().context("Authentication failed")?;

        let mut auth_value = HeaderValue::from_str(&format!("Bearer {access_token}"))?;
        auth_value.set_sensitive(true);

        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(reqwest::header::AUTHORIZATION, auth_value);
        headers.insert("X-Api-Key", HeaderValue::from_str(api_key)?);

        Ok(Self {
            headers,
            expiration_time: now + i64::from(res.expires_in()),
        })
    }

    pub fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.headers(self.headers.clone())
    }
}
