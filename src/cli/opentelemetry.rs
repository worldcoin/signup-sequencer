use eyre::Result as EyreResult;
use opentelemetry::{
    sdk::{
        trace::{self, IdGenerator, Sampler},
        Resource,
    },
    KeyValue,
};
use opentelemetry_http::{HeaderExtractor, HeaderInjector};
use opentelemetry_otlp::WithExportConfig;
use std::collections::HashMap;
use structopt::StructOpt;
use tracing::{info, Subscriber};
use tracing_subscriber::{registry::LookupSpan, Layer};

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    /// OpenTelemetry submission
    #[structopt(long)]
    pub opentelemetry: bool,
}

impl Options {
    pub fn to_layer<S>(&self) -> EyreResult<impl Layer<S>>
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        if !self.opentelemetry {
            return Ok(None);
        }

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
                "example",
            )]));

        let exporter = opentelemetry_otlp::new_exporter()
            .http()
            .with_endpoint("<URL>") // TODO replace <URL> with the URL as determined in section 2 above
            .with_headers(map);

        let tracer = opentelemetry_otlp::new_pipeline()
            .tracing()
            .with_exporter(exporter)
            .with_trace_config(trace_config)
            .install_batch(opentelemetry::runtime::Tokio)
            .unwrap();

        info!("OpenTelemetry enabled");

        Ok(Some(tracing_opentelemetry::layer().with_tracer(tracer)))
    }
}
