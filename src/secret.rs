use std::fmt;
use std::str::FromStr;

use url::Url;

#[derive(Clone, Eq, PartialEq)]
pub struct SecretUrl(Url);

impl SecretUrl {
    #[must_use]
    pub fn new(url: Url) -> Self {
        Self(url)
    }

    #[must_use]
    pub fn expose(&self) -> &str {
        self.0.as_str()
    }

    fn format(&self) -> Url {
        let mut url = self.0.clone();
        if url.has_authority() {
            if url.password().is_some() {
                url.set_password(None).unwrap();
            }
            url.set_username("**********").unwrap();
        }
        url
    }
}

impl FromStr for SecretUrl {
    type Err = <Url as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Url::from_str(s).map(SecretUrl::new)
    }
}

impl fmt::Display for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format().fmt(f)
    }
}

impl fmt::Debug for SecretUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.format().fmt(f)
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
}
