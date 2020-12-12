use anyhow::{Context, Result};
use futures::{Future, FutureExt as _};
use hyper::{
    service::{make_service_fn, service_fn},
    Body, Request, Response, Server,
};
use log::info;
use std::{convert::Infallible, net::SocketAddr};
use tokio;
use tokio_compat_02::FutureExt as _;

async fn hello_world(_req: Request<Body>) -> std::result::Result<Response<Body>, Infallible> {
    Ok(Response::new("Hello, World!\n".into()))
}

// Start server in separate function so we can call it with
// `tokio_compat_02::FutureExt::compat` since it uses Tokio 0.2.
async fn start_server<F>(socket_addr: &SocketAddr, stop_signal: F) -> Result<()>
where
    F: Future<Output = ()>,
{
    // A `Service` is needed for every connection, so this
    // creates one from our `hello_world` function.
    let service =
        make_service_fn(|_connection| async { Ok::<_, Infallible>(service_fn(hello_world)) });

    Server::bind(socket_addr)
        .serve(service)
        .with_graceful_shutdown(stop_signal)
        .await
        .context("Server error")
}

pub async fn async_main() -> Result<()> {
    // Catch SIGTERM so the container can shutdown without an init process.
    let stop_signal = tokio::signal::ctrl_c().map(|_| {
        info!("SIGTERM received, shutting down.");
    });

    // List on all interfaces on port 8080
    let socket_addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    start_server(&socket_addr, stop_signal).compat().await?;

    Ok(())
}

#[cfg(test)]
mod test {
    use super::*;
    use float_eq::assert_float_eq;
    use futures::stream::{self, StreamExt, TryStreamExt};
    use hyper::{
        body::{to_bytes, HttpBody},
        Request,
    };
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    #[tokio::test]
    async fn test_hello_world() {
        let request = Request::builder()
            .uri("https://www.rust-lang.org/")
            .header("User-Agent", "my-awesome-agent/1.0")
            .body(Body::empty())
            .unwrap();
        let response = hello_world(request).await.unwrap();
        let bytes = to_bytes(response.into_body()).await.unwrap();
        assert_eq!(bytes.as_ref(), b"Hello, World!\n");
    }
}
