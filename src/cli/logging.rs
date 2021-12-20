#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

use core::str::FromStr;
use eyre::{bail, Error as EyreError, Result as EyreResult, WrapErr as _};
use structopt::StructOpt;
use tracing::{Level, Subscriber};
use tracing_subscriber::{filter::Targets, fmt, Layer};

#[derive(Debug, PartialEq)]
enum LogFormat {
    Compact,
    Pretty,
    Json,
}

impl FromStr for LogFormat {
    type Err = EyreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "compact" => Self::Compact,
            "pretty" => Self::Pretty,
            "json" => Self::Json,
            _ => bail!("Invalid log format: {}", s),
        })
    }
}

#[derive(Debug, PartialEq, StructOpt)]
pub struct Options {
    /// Verbose mode (-v, -vv, -vvv, etc.)
    #[structopt(short, long, parse(from_occurrences))]
    verbose: usize,

    /// Apply an env_filter compatible log filter
    #[structopt(long, env, default_value)]
    log_filter: String,

    /// Log format, one of 'compact', 'pretty' or 'json'
    #[structopt(long, env, default_value = "pretty")]
    log_format: LogFormat,
}

impl Options {
    pub fn to_layer<S>(&self) -> EyreResult<impl Layer<S>>
    where
        S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a> + Send + Sync,
    {
        let log_format = match self.log_format {
            LogFormat::Compact => Box::new(fmt::Layer::new().event_format(fmt::format().compact()))
                as Box<dyn Layer<S> + Send + Sync>,
            LogFormat::Pretty => Box::new(fmt::Layer::new().event_format(fmt::format().pretty())),
            LogFormat::Json => Box::new(fmt::Layer::new().event_format(fmt::format().json())),
        };

        // Log filtering is a combination of `--log-filter` and `--verbose` arguments.
        let verbosity = {
            let (all, app) = match self.verbose {
                0 => (Level::INFO, Level::INFO),
                1 => (Level::INFO, Level::DEBUG),
                2 => (Level::INFO, Level::TRACE),
                3 => (Level::DEBUG, Level::TRACE),
                _ => (Level::TRACE, Level::TRACE),
            };
            Targets::new()
                .with_default(all)
                .with_target("lib", app)
                .with_target(env!("CARGO_BIN_NAME"), app)
        };
        let log_filter = if self.log_filter.is_empty() {
            Targets::new()
        } else {
            self.log_filter
                .parse()
                .wrap_err("Error parsing log-filter")?
        };
        // TODO: Waiting for <https://github.com/tokio-rs/tracing/issues/1790>
        let targets = verbosity.with_targets(log_filter);

        Ok(log_format.with_filter(targets))
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_parse_args() {
        let cmd = "arg0 -v --log-filter foo -vvv";
        let options = Options::from_iter_safe(cmd.split(' ')).unwrap();
        assert_eq!(options, Options {
            verbose:    4,
            log_filter: "foo".to_owned(),
            log_format: LogFormat::Pretty,
        });
    }
}
