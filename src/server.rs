use crate::identity::*;
use crate::walletclaims_contract::WalletClaims as WalletClaimsContract;

use ethers::core::types::Address;
use ethers::providers::{Provider, Http};

use ::prometheus::{opts, register_counter, register_histogram, Counter, Histogram};
use anyhow::{anyhow, Context as _, Result as AnyResult};
use hyper::{Body, Method, Request, Response, Server, StatusCode, body::Buf, service::{make_service_fn, service_fn}};
use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, IntCounterVec};
use std::{convert::TryFrom, net::{IpAddr, Ipv4Addr, SocketAddr}, sync::{Arc, RwLock, atomic::AtomicUsize}};
use structopt::StructOpt;
use tokio::sync::broadcast;
use tracing::{info, trace};
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
static MISSING: &[u8] = b"Missing field";

#[allow(clippy::unused_async)]
async fn hello_world(_req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    Ok(Response::new("Hello, World!\n".into()))
}

#[allow(clippy::unused_async)]
pub async fn inclusion_proof(
    req: Request<Body>,
    commitments: Arc<RwLock<Vec<String>>>,
) -> Result<Response<Body>, hyper::Error> {
    let whole_body = hyper::body::aggregate(req).await?;
    let data: serde_json::Value = serde_json::from_reader(whole_body.reader()).unwrap();
    let commitment = data["identityCommitment"].to_string();
    let proof = inclusion_proof_helper(commitment, commitments.clone()).unwrap();
    let response = format!("Inclusion Proof!\n {:?}", proof);
    Ok(Response::new(response.into()))
}

#[allow(clippy::unused_async)]
pub async fn insert_identity(req: Request<Body>, commitments: Arc<RwLock<Vec<String>>>, last_index: Arc<AtomicUsize>) -> Result<Response<Body>, hyper::Error> {
    let whole_body = hyper::body::aggregate(req).await?;
    let data: serde_json::Value = serde_json::from_reader(whole_body.reader()).unwrap();
    let identity_commitment = &data["identityCommitment"];
    if *identity_commitment == serde_json::Value::Null {
        return Ok(Response::builder()
            .status(StatusCode::UNPROCESSABLE_ENTITY)
            .body(MISSING.into())
            .unwrap());
    }

    insert_identity_helper(identity_commitment.to_string(), commitments.clone(), last_index);
    Ok(Response::new("Insert Identity!\n".into()))
}

#[allow(clippy::unused_async)] // We are implementing an interface
async fn route(request: Request<Body>, commitments: Arc<RwLock<Vec<String>>>, last_index: Arc<AtomicUsize>) -> Result<Response<Body>, hyper::Error> {
    // Measure and log request
    let _timer = LATENCY.start_timer(); // Observes on drop
    REQUESTS.inc();
    trace!(url = %request.uri(), "Receiving request");

    // Route requests
    let response = match (request.method(), request.uri().path()) {
        (&Method::GET, "/") => hello_world(request).await?,
        (&Method::GET, "/inclusionProof") => {
            inclusion_proof(request, commitments).await?
        }
        (&Method::POST, "/insertIdentity") => insert_identity(request, commitments, last_index).await?,
        _ => Response::builder()
            .status(404)
            .body(Body::from("404"))
            .unwrap(),
    };

    // Measure result and return
    STATUS
        .with_label_values(&[response.status().as_str()])
        .inc();
    Ok(response)
}

pub async fn main(options: Options, shutdown: broadcast::Sender<()>) -> AnyResult<()> {
    if options.server.scheme() != "http" {
        return Err(anyhow!("Only http:// is supported in {}", options.server));
    }
    if options.server.path() != "/" {
        return Err(anyhow!("Only / is supported in {}", options.server));
    }
    let ip: IpAddr = match options.server.host() {
        Some(Host::Ipv4(ip)) => ip.into(),
        Some(Host::Ipv6(ip)) => ip.into(),
        Some(_) => return Err(anyhow!("Cannot bind {}", options.server)),
        None => Ipv4Addr::LOCALHOST.into(),
    };
    let port = options.server.port().unwrap_or(9998);
    let addr = SocketAddr::new(ip, port);

    // connect to the network
    let provider = Provider::<Http>::try_from(
        "http://localhost:8545"
    ).expect("could not instantiate HTTP Provider");

    // let KEY = "0xee79b5f6e221356af78cf4c36f4f7885a11b67dfcc81c34d80249947330c0f82";

    // Proof of concept contract interaction
    let CONTRACT_ADDRESS = "0xb2f8c8cf0607d1df2E2d1c37e36D4657031438AE".parse::<Address>()?;
    let contract = WalletClaimsContract::new(CONTRACT_ADDRESS, Arc::new(provider));
    let name = contract.name().call().await?;
    println!("Contract name {}", name);

    let commitments = Arc::new(RwLock::new(initialize_commitments()));
    let last_index = Arc::new(AtomicUsize::new(0));

    let make_svc = make_service_fn(move |_| {
        // Clone here as `make_service_fn` is called for every connection
        let commitments = commitments.clone();
        let last_index = last_index.clone();
        async {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                // Clone here as `service_fn` is called for every request
                let commitments = commitments.clone();
                let last_index = last_index.clone();
                route(req,commitments, last_index)
            }))
        }
    });

    let server = Server::try_bind(&addr)
        .context("Could not bind server port")?
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
    use hyper::{body::to_bytes, Request};
    use pretty_assertions::assert_eq;

    #[tokio::test]
    async fn test_hello_world() {
        let request = Request::new(Body::empty());
        let response = hello_world(request).await.unwrap();
        let bytes = to_bytes(response.into_body()).await.unwrap();
        assert_eq!(bytes.as_ref(), b"Hello, World!\n");
    }
}
#[cfg(feature = "bench")]
#[allow(clippy::wildcard_imports, unused_imports)]
pub mod bench {
    use super::*;
    use crate::bench::runtime;
    use criterion::{black_box, Criterion};
    use hyper::body::to_bytes;

    pub fn group(c: &mut Criterion) {
        bench_hello_world(c);
    }

    fn bench_hello_world(c: &mut Criterion) {
        c.bench_function("bench_hello_world", |b| {
            b.to_async(runtime()).iter(|| async {
                let request = Request::new(Body::empty());
                let response = hello_world(request).await.unwrap();
                let bytes = to_bytes(response.into_body()).await.unwrap();
                drop(black_box(bytes));
            });
        });
    }
}
