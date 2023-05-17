use std::{fmt, str::FromStr};
use url::Url;

#[derive(Clone, Eq, PartialEq)]
pub struct Secret<S>(S)
where
    S: fmt::Debug + AsRef<str>;

impl<S> Secret<S>
where
    S: fmt::Debug + AsRef<str>,
{
    pub fn new(value: S) -> Secret<S> {
        Secret(value)
    }

    pub fn expose(&self) -> &str {
        self.0.as_ref()
    }
}
impl<S> fmt::Debug for Secret<S>
where
    S: fmt::Debug + AsRef<str>,
{
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("**********")
    }
}

impl<S> fmt::Display for Secret<S>
where
    S: fmt::Debug + AsRef<str>,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("**********")
    }
}

//////////////////////////////////////
// Specific implementations for Url //

#[derive(Clone, Eq, PartialEq)]
pub struct SecretUrl {
    url: Secret<Url>,
}

impl SecretUrl {
    pub fn new(secret: Secret<Url>) -> Self {
        Self { url: secret }
    }

    pub fn expose(&self) -> &str {
        self.url.expose()
    }

    fn format(&self) -> String {
        if self.url.0.has_authority() {
            let mut url = self.url.0.clone();
            if url.password().is_some() {
                url.set_password(None).unwrap();
            }
            url.set_username("**********").unwrap();
            return url.to_string();
        }
        self.url.0.to_string()
    }
}

impl AsRef<str> for SecretUrl {
    fn as_ref(&self) -> &str {
        self.url.0.as_str()
    }
}

impl FromStr for Secret<Url> {
    type Err = <Url as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Url::from_str(s).map(Secret::new)
    }
}

impl FromStr for SecretUrl {
    type Err = <Url as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let secret = Url::from_str(s).map(Secret::new)?;
        Ok(Self::new(secret))
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
    fn test_expose() {
        let secret = Secret::from_str("password@something!").unwrap();
        assert_eq!(secret.expose(), "password@something!");
    }

    #[test]
    fn test_debug() {
        let secret = Secret::from_str("password@something!").unwrap();
        assert_eq!(format!("{:?}", secret), "**********");
    }

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
