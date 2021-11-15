#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

use core::str::FromStr;
use eyre::{bail, eyre, Error as EyreError, Result as EyreResult, WrapErr as _};
use structopt::StructOpt;
use tracing::{debug, info};
use tracing_subscriber::{fmt, EnvFilter};

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
pub struct LogOptions {
    /// Verbose mode (-v, -vv, -vvv, etc.)
    #[structopt(short, long, parse(from_occurrences))]
    verbose: usize,

    /// Apply an env_filter compatible log filter
    #[structopt(long, env = "LOG_FILTER", default_value)]
    log_filter: String,

    /// Log format, one of 'compact', 'pretty' or 'json'
    #[structopt(long, env = "LOG_FORMAT", default_value = "pretty")]
    log_format: LogFormat,
}

impl LogOptions {
    #[allow(dead_code)]
    pub fn init(&self) -> EyreResult<()> {
        let log_filter = match self.verbose {
            0 => "info".to_owned(),
            1 => format!("{}=debug,lib=debug", env!("CARGO_CRATE_NAME")),
            2 => format!("{}=trace,lib=trace", env!("CARGO_CRATE_NAME")),
            3 => format!("{}=trace,lib=trace,debug", env!("CARGO_CRATE_NAME")),
            _ => "trace".to_owned(),
        };
        let log_filter = if self.log_filter.is_empty() {
            log_filter
        } else {
            format!("{},{}", log_filter, self.log_filter)
        };
        let log_filter = EnvFilter::try_new(log_filter)?;
        let collector = fmt::fmt().with_env_filter(log_filter);
        match self.log_format {
            LogFormat::Compact => collector.compact().try_init(),
            LogFormat::Pretty => collector.pretty().try_init(),
            LogFormat::Json => {
                collector
                    .without_time() // See <https://github.com/tokio-rs/tracing/issues/1509>
                    .json()
                    .try_init()
            }
        }
        .map_err(|err| eyre!(err))
        .wrap_err("setting default log collector")?;

        // Log version information
        info!(
            "{name} {version} {commit}",
            name = env!("CARGO_CRATE_NAME"),
            version = env!("CARGO_PKG_VERSION"),
            commit = &env!("COMMIT_SHA")[..8],
        );

        // Log main address to test ASLR
        debug!("Address of main {:#x}", &crate::main as *const _ as usize);

        Ok(())
    }
}

#[cfg(test)]
pub mod test {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn test_parse_args() {
        let cmd = "arg0 -v --log-filter foo -vvv";
        let options = LogOptions::from_iter_safe(cmd.split(' ')).unwrap();
        assert_eq!(options, LogOptions {
            verbose:    4,
            log_filter: "foo".to_owned(),
            log_format: LogFormat::Pretty,
        });
    }
}
