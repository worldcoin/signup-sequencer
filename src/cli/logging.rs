#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

use core::str::FromStr;
use eyre::{bail, eyre, Error as EyreError, Result as EyreResult, WrapErr as _};
use structopt::StructOpt;
use tracing::{debug, info, Level, Subscriber};
use tracing_subscriber::{
    filter::Directive, fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    Registry,
};

#[derive(Debug, PartialEq)]
enum LogFormat {
    Compact,
    Pretty,
    Json,
}

impl LogFormat {
    fn to_layer<S>(&self) -> impl Layer<S>
    where
        S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a> + Send + Sync,
    {
        match self {
            LogFormat::Compact => Box::new(fmt::Layer::new().event_format(fmt::format().compact()))
                as Box<dyn Layer<S> + Send + Sync>,
            LogFormat::Pretty => Box::new(fmt::Layer::new().event_format(fmt::format().pretty())),
            LogFormat::Json => Box::new(fmt::Layer::new().event_format(fmt::format().json())),
        }
    }
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
        // Log filtering is a combination of `--log-filter` and `--verbose` arguments.
        let log_filter = if self.log_filter.is_empty() {
            EnvFilter::default()
        } else {
            EnvFilter::try_new(&self.log_filter)?
        };
        let log_filter = log_filter.add_directive(match self.verbose {
            0 => Level::INFO.into(),
            1 => format!("{}=debug,lib=debug", env!("CARGO_CRATE_NAME")).parse()?,
            2 => format!("{}=trace,lib=trace", env!("CARGO_CRATE_NAME")).parse()?,
            3 => format!("{}=trace,lib=trace,debug", env!("CARGO_CRATE_NAME")).parse()?,
            _ => Level::TRACE.into(),
        });

        // Support server for tokio-console
        let console_layer = { console_subscriber::ConsoleLayer::builder().spawn() };

        let subscriber = Registry::default().with(console_layer);
        let subscriber = self.log_format.to_layer().with_subscriber(subscriber);
        tracing::subscriber::set_global_default(subscriber)?;

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
