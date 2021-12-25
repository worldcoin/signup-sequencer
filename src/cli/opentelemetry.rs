use eyre::Result as EyreResult;
use opentelemetry::{
    global::{force_flush_tracer_provider, shutdown_tracer_provider},
    sdk::{
        propagation::TraceContextPropagator,
        trace::{self, IdGenerator, Sampler},
        Resource,
    },
    KeyValue,
};
use opentelemetry_otlp::WithExportConfig;
use std::collections::HashMap;
use structopt::StructOpt;
use tracing::{error, info, Subscriber};
use tracing_subscriber::{registry::LookupSpan, Layer};
use url::Url;

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    /// OpenTelemetry http trace submission endpoint
    #[structopt(long, env)]
    pub otlp_trace: Option<Url>,
}

impl Options {
    #[allow(clippy::unnecessary_wraps)] // Consistency with other modules
    pub fn to_layer<S>(&self) -> EyreResult<impl Layer<S>>
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        let endpoint = if let Some(endpoint) = &self.otlp_trace {
            endpoint
        } else {
            return Ok(None);
        };

        // Set global error handler
        opentelemetry::global::set_error_handler(|error| {
            error!(?error, "{msg}", msg = error);
        })?;

        // Set global propagator
        opentelemetry::global::set_text_map_propagator(TraceContextPropagator::new());

        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], 6831));

        let mut map = HashMap::new();
        // TODO replace <TOKEN> with the authentication token created in section 2 above
        map.insert("Authorization".to_string(), "Api-Token <TOKEN>".to_string());

        let trace_config = trace::config()
            .with_sampler(Sampler::AlwaysOn)
            .with_id_generator(IdGenerator::default())
            .with_max_events_per_span(64)
            .with_max_attributes_per_span(16)
            .with_max_events_per_span(16)
            .with_resource(Resource::new(vec![KeyValue::new(
                "service.name",
                env!("CARGO_CRATE_NAME"),
            )]));

        let exporter = opentelemetry_otlp::new_exporter()
            .http()
            .with_endpoint(endpoint.to_string());

        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(exporter)
            .with_trace_config(trace_config)
            .install_batch(opentelemetry::runtime::Tokio)?;

        info!("OpenTelemetry enabled");

        Ok(Some(tracing_opentelemetry::layer().with_tracer(tracer)))
    }
}

pub fn shutdown() {
    info!("Flushing traces and stop tracing");
    force_flush_tracer_provider();
    shutdown_tracer_provider();
}
