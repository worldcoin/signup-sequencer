use std::{fmt, str::FromStr};
use url::Url;

#[derive(Clone, Eq, PartialEq)]
pub struct SecretUrl(Url);

impl SecretUrl {
    pub fn new(url: Url) -> Self {
        Self(url)
    }

    pub fn expose(&self) -> &str {
        self.as_ref()
    }

    fn format(&self) -> String {
        if self.0.has_authority() {
            let mut url = self.0.clone();
            if url.password().is_some() {
                url.set_password(None).unwrap();
            }
            url.set_username("**********").unwrap();
            return url.to_string();
        }
        self.0.to_string()
    }
}

impl AsRef<str> for SecretUrl {
    fn as_ref(&self) -> &str {
        self.0.as_str()
    }
}

impl FromStr for SecretUrl {
    type Err = <Url as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Url::from_str(s).map(SecretUrl::new)?)
    }
}

impl fmt::Display for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

impl fmt::Debug for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.format())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_expose() {
        let secret =
            SecretUrl::from_str("postgres://user:password@localhost:5432/database").unwrap();
        assert_eq!(
            secret.expose(),
            "postgres://user:password@localhost:5432/database"
        );
    }

    #[test]
    fn test_url_debug() {
        let secret =
            SecretUrl::from_str("postgres://user:password@localhost:5432/database").unwrap();
        assert_eq!(
            format!("{:?}", secret),
            "postgres://**********@localhost:5432/database"
        );
    }
}
