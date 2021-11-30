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
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
};
use structopt::StructOpt;
use thiserror::Error;
use tokio::sync::broadcast;
use tracing::{error, info, trace};
use url::{Host, Url};

#[derive(Debug, PartialEq, StructOpt)]
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
    identity_index: usize,
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
            .unwrap()
    }
}

/// Parse a [`Request<Body>`] as JSON using Serde and handle using the provided
/// method.
async fn json_middleware<F, T, S, U>(request: Request<Body>, mut next: F) -> Result<U, Error>
where
    T: DeserializeOwned + Send,
    F: FnMut(T) -> S + Send,
    S: Future<Output = Result<U, Error>> + Send,
{
    // TODO seems unnecessary as the handler passing this here already qualifies the
    // method if request.method() != Method::POST {
    //     return Err(Error::InvalidMethod);
    // }
    let valid_content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .map_or(false, |content_type| content_type == CONTENT_JSON);
    if !valid_content_type {
        return Err(Error::InvalidContentType);
    }
    let body = hyper::body::aggregate(request).await?;
    let value = serde_json::from_reader(body.reader())?;
    next(value).await
}

fn convert_data_to_response<T: Serialize>(data: Result<T, Error>) -> Response<Body> {
    match data {
        Ok(data) => Response::new(Body::from(serde_json::to_string_pretty(&data).unwrap())),
        Err(e) => e.to_response(),
    }
}

async fn route(request: Request<Body>, app: Arc<App>) -> Result<Response<Body>, hyper::Error> {
    // Measure and log request
    let _timer = LATENCY.start_timer(); // Observes on drop
    REQUESTS.inc();
    trace!(url = %request.uri(), "Receiving request");

    // Route requests
    let response = match (request.method(), request.uri().path()) {
        (&Method::POST, "/inclusionProof") => convert_data_to_response(
            json_middleware(request, |request: InclusionProofRequest| {
                let app = app.clone();
                async move { app.inclusion_proof(request.identity_index).await }
            })
            .await,
        ),
        (&Method::POST, "/insertIdentity") => convert_data_to_response(
            json_middleware(request, |request: InsertCommitmentRequest| {
                let app = app.clone();
                async move { app.insert_identity(&request.identity_commitment).await }
            })
            .await,
        ),
        _ => Error::InvalidMethod.to_response(),
    };

    // Measure result and return
    STATUS
        .with_label_values(&[response.status().as_str()])
        .inc();
    Ok(response)
}

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

    let server = Server::try_bind(&addr)
        .wrap_err("Could not bind server port")?
        .serve(make_svc)
        .with_graceful_shutdown(async move {
            shutdown.subscribe().recv().await.ok();
        });
    info!(url = %options.server, "Server listening");

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
