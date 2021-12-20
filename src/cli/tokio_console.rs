use console_subscriber::ConsoleLayer;
use eyre::Result as EyreResult;
use structopt::StructOpt;
use tracing::{info, Subscriber};
use tracing_subscriber::{registry::LookupSpan, Layer};

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    /// Start a tokio-console server on `http://127.0.0.1:6669/`.
    #[structopt(long)]
    pub tokio_console: bool,
}

impl Options {
    pub fn to_layer<S>(&self) -> EyreResult<impl Layer<S>>
    where
        S: Subscriber + for<'span> LookupSpan<'span>,
    {
        Ok(self.tokio_console.then(|| {
            // TODO: Remove when <https://github.com/tokio-rs/tokio/issues/4114> resolves
            // TODO: Configure server addr.
            // TODO: Manage server thread.
            assert!(
                cfg!(tokio_unstable),
                "Enabling --tokio-console requires a build with RUSTFLAGS=\"--cfg tokio_unstable\""
            );
            info!("Tokio-console available at http://127.0.0.1:6669/");
            ConsoleLayer::builder().spawn()
        }))
    }
}
