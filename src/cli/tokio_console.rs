use console_subscriber::ConsoleLayer;
use tracing::Subscriber;
use tracing_subscriber::{registry::LookupSpan, Layer};

pub fn layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    cfg!(tokio_unstable).then(|| 
        ConsoleLayer::builder().spawn()
    )
}
