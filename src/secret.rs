use serde::Deserialize;
use std::{fmt, str::FromStr};
use url::Url;

#[derive(Clone, Eq, PartialEq, Deserialize)]
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

impl FromStr for Secret<Url> {
    type Err = <Url as FromStr>::Err;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Url::from_str(s).map(Secret::new)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expose() {
        let secret = Secret(String::from("password@something!"));
        assert_eq!(secret.expose(), "password@something!");
    }

    #[test]
    fn test_debug() {
        let secret = Secret(String::from("password@something!"));
        assert_eq!(format!("{:?}", secret), "**********");
    }
}
