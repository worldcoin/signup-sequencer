use crate::{app::App, hash::Hash};
use ::prometheus::{opts, register_counter, register_histogram, Counter, Histogram};
use eyre::{bail, ensure, Error as EyreError, Result as EyreResult, WrapErr as _};
use futures::Future;
use hyper::{
    body::Buf,
    header,
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};
use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, IntCounterVec};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
    sync::Arc,
};
use structopt::StructOpt;
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::{error, info, trace};
use url::{Host, Url};

#[derive(Clone, Debug, PartialEq, StructOpt)]
pub struct Options {
    /// API Server url
    #[structopt(long, env = "SERVER", default_value = "http://127.0.0.1:8080/")]
    pub server: Url,
}

static REQUESTS: Lazy<Counter> =
    Lazy::new(|| register_counter!(opts!("api_requests", "Number of requests received.")).unwrap());
static STATUS: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "api_response_status",
        "The API responses by status code.",
        &["status_code"]
    )
    .unwrap()
});
static LATENCY: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!("api_latency_seconds", "The API latency in seconds.").unwrap()
});
const CONTENT_JSON: &str = "application/json";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InsertCommitmentRequest {
    identity_commitment: Hash,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InclusionProofRequest {
    pub identity_index: usize,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid method")]
    InvalidMethod,
    #[error("invalid content type")]
    InvalidContentType,
    #[error("provided identity index out of bounds")]
    IndexOutOfBounds,
    #[error("invalid serialization format")]
    InvalidSerialization(#[from] serde_json::Error),
    #[error(transparent)]
    Hyper(#[from] hyper::Error),
    #[error(transparent)]
    Other(#[from] EyreError),
}

impl Error {
    fn to_response(&self) -> hyper::Response<Body> {
        hyper::Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(hyper::Body::from(self.to_string()))
            .expect("Failed to convert error string into hyper::Body")
    }
}

/// Parse a [`Request<Body>`] as JSON using Serde and handle using the provided
/// method.
async fn json_middleware<F, T, S, U>(
    request: Request<Body>,
    mut next: F,
) -> Result<Response<Body>, Error>
where
    T: DeserializeOwned + Send,
    F: FnMut(T) -> S + Send,
    S: Future<Output = Result<U, Error>> + Send,
    U: Serialize,
{
    let valid_content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .map_or(false, |content_type| content_type == CONTENT_JSON);
    if !valid_content_type {
        return Err(Error::InvalidContentType);
    }
    let body = hyper::body::aggregate(request).await?;
    let request = serde_json::from_reader(body.reader())?;
    let response = next(request).await?;
    let json = serde_json::to_string_pretty(&response)?;
    Ok(Response::new(Body::from(json)))
}

async fn route(request: Request<Body>, app: Arc<App>) -> Result<Response<Body>, hyper::Error> {
    // Measure and log request
    let _timer = LATENCY.start_timer(); // Observes on drop
    REQUESTS.inc();
    trace!(url = %request.uri(), "Receiving request");

    // Route requests
    let result = match (request.method(), request.uri().path()) {
        (&Method::POST, "/inclusionProof") => {
            json_middleware(request, |request: InclusionProofRequest| {
                let app = app.clone();
                async move { app.inclusion_proof(request.identity_index).await }
            })
            .await
        }
        (&Method::POST, "/insertIdentity") => {
            json_middleware(request, |request: InsertCommitmentRequest| {
                let app = app.clone();
                async move { app.insert_identity(&request.identity_commitment).await }
            })
            .await
        }
        _ => Err(Error::InvalidMethod),
    };
    let response = result.unwrap_or_else(|err| err.to_response());

    // Measure result and return
    STATUS
        .with_label_values(&[response.status().as_str()])
        .inc();
    Ok(response)
}

/// # Errors
///
/// Will return `Err` if `options.server` URI is not http, incorrectly includes
/// a path beyond `/`, or cannot be cast into an IP address. Also returns an
/// `Err` if the server cannot bind to the given address.
pub async fn main(
    app: Arc<App>,
    options: Options,
    shutdown: broadcast::Sender<()>,
) -> EyreResult<()> {
    ensure!(
        options.server.scheme() == "http",
        "Only http:// is supported in {}",
        options.server
    );
    ensure!(
        options.server.path() == "/",
        "Only / is supported in {}",
        options.server
    );
    let ip: IpAddr = match options.server.host() {
        Some(Host::Ipv4(ip)) => ip.into(),
        Some(Host::Ipv6(ip)) => ip.into(),
        Some(_) => bail!("Cannot bind {}", options.server),
        None => Ipv4Addr::LOCALHOST.into(),
    };
    let port = options.server.port().unwrap_or(9998);
    let addr = SocketAddr::new(ip, port);

    let listener = TcpListener::bind(&addr)?;

    bind_from_listener(app, listener, shutdown).await?;

    Ok(())
}

/// # Errors
///
/// Will return `Err` if the provided `listener` address cannot be accessed or
/// if the server fails to bind to the given address.
pub async fn bind_from_listener(
    app: Arc<App>,
    listener: TcpListener,
    shutdown: broadcast::Sender<()>,
) -> EyreResult<()> {
    let local_addr = listener.local_addr()?;
    let make_svc = make_service_fn(move |_| {
        // Clone here as `make_service_fn` is called for every connection
        let app = app.clone();
        async {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                // Clone here as `service_fn` is called for every request
                let app = app.clone();
                route(req, app)
            }))
        }
    });

    let server = Server::from_tcp(listener)
        .wrap_err("Failed to bind address")?
        .serve(make_svc)
        .with_graceful_shutdown(async move {
            shutdown.subscribe().recv().await.ok();
        });

    info!(url = %local_addr, "Server listening");

    server.await?;
    Ok(())
}

#[cfg(test)]
#[allow(unused_imports)]
mod test {
    use super::*;
    use hyper::{body::to_bytes, Request, StatusCode};
    use pretty_assertions::assert_eq;
    use serde_json::json;

    // TODO: Fix test
    // #[tokio::test]
    #[allow(dead_code)]
    async fn test_inclusion_proof() {
        let options = crate::app::Options::from_iter_safe(&[""]).unwrap();
        let app = Arc::new(App::new(options).await.unwrap());
        let body = Body::from(
            json!({
                "identityIndex": 0,
            })
            .to_string(),
        );
        let request = Request::builder()
            .method("POST")
            .uri("/inclusionProof")
            .header("Content-Type", "application/json")
            .body(body)
            .unwrap();
        let res = route(request, app).await.unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        // TODO deserialize proof and compare results
    }
}
#[cfg(feature = "bench")]
#[allow(clippy::wildcard_imports, unused_imports)]
pub mod bench {
    use super::*;
    use crate::bench::runtime;
    use criterion::{black_box, Criterion};
    use hyper::body::to_bytes;

    pub fn group(_c: &mut Criterion) {
        //     bench_hello_world(c);
    }

    // fn bench_hello_world(c: &mut Criterion) {
    //     let app = Arc::new(App::new(2));
    //     let request = CommitmentRequest {
    //         identity_commitment:
    // "24C94355810D659EEAA9E0B9E21F831493B50574AA2D3205F0AAB779E2864623"
    //             .to_string(),
    //     };
    //     c.bench_function("bench_insert_identity", |b| {
    //         b.to_async(runtime()).iter(|| async {
    //             let response =
    // app.insert_identity(request.clone()).await.unwrap();             let
    // bytes = to_bytes(response.into_body()).await.unwrap();
    // drop(black_box(bytes));         });
    //     });
    // }
}
