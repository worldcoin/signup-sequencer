#![doc = include_str!("../Readme.md")]
#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

pub mod app;
mod contracts;
pub mod database;
mod ethereum;
mod ethereum_subscriber;
mod identity_committer;
pub mod identity_tree;
pub mod server;
mod timed_rw_lock;
mod tx_sitter;
mod utils;

use crate::app::App;
use anyhow::Result as AnyhowResult;
use clap::Parser;
use std::sync::Arc;
use tracing::info;

#[derive(Clone, Debug, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    #[clap(flatten)]
    pub app: app::Options,

    #[clap(flatten)]
    pub server: server::Options,
}

/// ```
/// assert!(true);
/// ```
#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
pub async fn main(options: Options) -> AnyhowResult<()> {
    // Create App struct
    let app = Arc::new(App::new(options.app).await?);
    let app_for_server = app.clone();

    // Start server (will stop on shutdown signal)
    server::main(app_for_server, options.server).await?;

    info!("Stopping the app");
    app.shutdown().await?;

    Ok(())
}

#[cfg(test)]
pub mod test {
    use super::*;
    use proptest::proptest;
    use tracing::{error, warn};
    use tracing_test::traced_test;

    #[test]
    #[allow(clippy::eq_op)]
    fn test_with_proptest() {
        proptest!(|(a in 0..5, b in 0..5)| {
            assert_eq!(a + b, b + a);
        });
    }

    #[test]
    #[allow(clippy::disallowed_methods)] // False positive from macro
    #[traced_test]
    fn test_with_log_output() {
        error!("logged on the error level");
        assert!(logs_contain("logged on the error level"));
    }

    #[tokio::test]
    #[allow(clippy::disallowed_methods)] // False positive from macro
    #[traced_test]
    #[allow(clippy::semicolon_if_nothing_returned)] // False positive
    async fn async_test_with_log() {
        // Local log
        info!("This is being logged on the info level");

        // Log from a spawned task (which runs in a separate thread)
        tokio::spawn(async {
            warn!("This is being logged on the warn level from a spawned task");
        })
        .await
        .unwrap();

        // Ensure that `logs_contain` works as intended
        assert!(logs_contain("logged on the info level"));
        assert!(logs_contain("logged on the warn level"));
        assert!(!logs_contain("logged on the error level"));
    }
}

#[cfg(feature = "bench")]
#[doc(hidden)]
pub mod bench {
    use criterion::{black_box, BatchSize, Criterion};
    use proptest::{
        strategy::{Strategy, ValueTree},
        test_runner::TestRunner,
    };
    use std::time::Duration;
    use tokio::runtime;

    pub fn group(criterion: &mut Criterion) {
        crate::server::bench::group(criterion);
        bench_example_proptest(criterion);
        bench_example_async(criterion);
    }

    /// Constructs an executor for async tests
    pub(crate) fn runtime() -> runtime::Runtime {
        runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// Example proptest benchmark
    /// Uses proptest to randomize the benchmark input
    fn bench_example_proptest(criterion: &mut Criterion) {
        let input = (0..5, 0..5);
        let mut runner = TestRunner::deterministic();
        // Note: benchmarks need to have proper identifiers as names for
        // the CI to pick them up correctly.
        criterion.bench_function("example_proptest", move |bencher| {
            bencher.iter_batched(
                || input.new_tree(&mut runner).unwrap().current(),
                |(a, b)| {
                    // Benchmark number addition
                    black_box(a + b)
                },
                BatchSize::LargeInput,
            );
        });
    }

    /// Example async benchmark
    /// See <https://bheisler.github.io/criterion.rs/book/user_guide/benchmarking_async.html>
    fn bench_example_async(criterion: &mut Criterion) {
        let duration = Duration::from_micros(1);
        criterion.bench_function("example_async", move |bencher| {
            bencher.to_async(runtime()).iter(|| async {
                tokio::time::sleep(duration).await;
            });
        });
    }
}
