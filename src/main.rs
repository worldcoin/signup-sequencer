#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

mod server;

use anyhow::{Context, Result};
use env_logger;
use log::info;
use structopt::StructOpt;
use tokio;

#[derive(Debug, PartialEq, StructOpt)]
struct Options {
    /// Verbose mode (-v, -vv, -vvv, etc.)
    #[structopt(short, long, parse(from_occurrences))]
    verbose: usize,

    #[structopt(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, PartialEq, StructOpt)]
enum Command {
    /// Show version information
    Test,
}

fn main() -> Result<()> {
    // Parse CLI and handle help and version.
    #[rustfmt::skip]
    let version = format!("\
        {version} {commit} ({commit_date})\n\
        {target} ({build_date})\n\
        {author}\n\
        {homepage}\n\
        {description}",
        version     = env!("CARGO_PKG_VERSION"),
        commit      = &env!("COMMIT_SHA")[..8],
        commit_date = env!("COMMIT_DATE"),
        author      = env!("CARGO_PKG_AUTHORS"),
        description = env!("CARGO_PKG_DESCRIPTION"),
        homepage    = env!("CARGO_PKG_HOMEPAGE"),
        target      = env!("TARGET"),
        build_date  = env!("BUILD_DATE"),
    );
    let matches = Options::clap().long_version(version.as_str()).get_matches();
    let options = Options::from_clap(&matches);

    // Initialize log output (prepend verbosity to RUST_LOG)
    let rust_log = match options.verbose {
        0 => "error",
        1 => "warn",
        2 => "info",
        3 => "debug",
        _ => "trace",
    };
    let rust_log_env = std::env::var("RUST_LOG").map_or_else(
        |_| rust_log.to_string(),
        |arg| format!("{},{}", rust_log, arg),
    );
    std::env::set_var("RUST_LOG", rust_log_env);
    env_logger::init();

    // Log version
    info!(
        "{name} {version} {commit}",
        name = env!("CARGO_CRATE_NAME"),
        version = env!("CARGO_PKG_VERSION"),
        commit = &env!("COMMIT_SHA")[..8],
    );

    // Launch Tokio runtime
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Error creating Tokio runtime")?
        .block_on(server::async_main())
        .context("Error in main thread")?;

    // Terminate successfully
    info!("program stopping normally");
    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use float_eq::assert_float_eq;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    #[test]
    fn parse_args() {
        let cmd = "hello -vvv";
        let options = Options::from_iter_safe(cmd.split(' ')).unwrap();
        assert_eq!(options, Options {
            verbose: 3,
            command: None,
        });
    }

    #[test]
    fn add_commutative() {
        proptest!(|(a in 0.0..1.0, b in 0.0..1.0)| {
            let first: f64 = a + b;
            assert_float_eq!(first, b + a, ulps <= 0);
        })
    }
}

#[cfg(feature = "bench")]
pub fn bench_main(c: &mut criterion::Criterion) {
    server::bench::group(c);
}
