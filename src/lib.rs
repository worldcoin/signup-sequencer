#![doc = include_str ! ("../Readme.md")]
#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

pub mod app;
mod contracts;
mod ethereum;
pub mod server;
mod utils;

use crate::{app::App, utils::spawn_or_abort};
use eyre::Result as EyreResult;
use std::sync::Arc;
use structopt::StructOpt;

use tracing_subscriber::prelude::*;
use opentelemetry::global;
use tracing::{error, span, debug, warn, info, Level};
use opentelemetry::global::shutdown_tracer_provider;
use opentelemetry::{
    KeyValue,
    trace::{Span, Tracer},
    Key,
};

use opentelemetry_otlp::WithExportConfig;
use opentelemetry::sdk::{trace::{self, IdGenerator, Sampler}, Resource};

use std::thread;
use std::time::Duration;
use std::env;
use tracing_subscriber::fmt::format;

fn bar() {
    let tracer = global::tracer("component-bar");
    let mut span = tracer.start("bar");
    span.set_attribute(Key::new("span.type").string("sql"));
    span.set_attribute(Key::new("sql.query").string("SELECT * FROM table"));
    thread::sleep(Duration::from_millis(6));
    span.end()
}

#[tracing::instrument]
fn foo() {
    span!(tracing::Level::INFO, "expensive_step_2")
        .in_scope(|| thread::sleep(Duration::from_millis(25)));
    info!("foo bar");
}

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    #[structopt(flatten)]
    pub app: app::Options,

    #[structopt(flatten)]
    pub server: server::Options,
}

/// ```
/// assert!(true);
/// ```
#[allow(clippy::missing_errors_doc, clippy::missing_panics_doc)]
pub async fn main() -> EyreResult<()> {
    let dd_agent_host = env::var("DD_AGENT_HOST")?;

    let tracer = opentelemetry_otlp::new_pipeline()
        .tracing().with_exporter(
        opentelemetry_otlp::new_exporter()
            .tonic()
            .with_endpoint(format!("http://{}:4317", dd_agent_host))
            .with_timeout(Duration::from_secs(3)))
        .with_trace_config(
            trace::config()
                .with_resource(
                    Resource::new(vec![
                        KeyValue::new("service.name", "signup-sequencer-test"),
                        KeyValue::new("env", "stage"),
                        KeyValue::new("version", "0.0.0")])))
        .install_batch(opentelemetry::runtime::Tokio)?;

    tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(tracer))
        .try_init()?;

    loop {
        foo();
        thread::sleep(Duration::from_millis(200));
    }

    // Wait for shutdown
    info!("Program started, waiting for shutdown signal");
    // await_shutdown().await;

    // Wait for server
    info!("Stopping server");
    // server.await?;
    shutdown_tracer_provider();
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
    #[traced_test]
    fn test_with_log_output() {
        error!("logged on the error level");
        assert!(logs_contain("logged on the error level"));
    }

    #[tokio::test]
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
