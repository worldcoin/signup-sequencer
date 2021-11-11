use crate::{
    identity::{
        inclusion_proof_helper, insert_identity_commitment, insert_identity_to_contract, Commitment,
    },
    mimc_tree::MimcTree,
    solidity::{initialize_semaphore, SemaphoreContract},
};
use ::prometheus::{opts, register_counter, register_histogram, Counter, Histogram};
use ethers::prelude::{Http, Provider};
use eyre::{bail, ensure, Result as EyreResult, WrapErr as _};
use futures::Future;
use hex_literal::hex;
use hyper::{
    body::Buf,
    header,
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server,
};
use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, IntCounterVec};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, RwLock,
    },
};
use structopt::StructOpt;
use tokio::sync::broadcast;
use tracing::{info, trace};
use url::{Host, Url};
use zkp_u256::U256;

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
const NUM_LEVELS: usize = 20;
const NOTHING_UP_MY_SLEEVE: Commitment =
    hex!("1c4823575d154474ee3e5ac838d002456a815181437afd14f126da58a9912bbe");

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitmentRequest {
    identity_commitment: String,
}

pub struct App {
    merkle_tree:        RwLock<MimcTree>,
    last_leaf:          AtomicUsize,
    provider:           Provider<Http>,
    semaphore_contract: SemaphoreContract,
}

impl App {
    pub async fn new(depth: usize) -> Self {
        let x = initialize_semaphore().await;
        let (provider, semaphore) = match initialize_semaphore().await {
            Ok((provider, semaphore)) => (provider, semaphore),
            Err(e) => panic!("Error building app"),
        };
        Self {
            merkle_tree:        RwLock::new(MimcTree::new(depth, NOTHING_UP_MY_SLEEVE)),
            last_leaf:          AtomicUsize::new(0),
            provider,
            semaphore_contract: semaphore,
        }
    }

    #[allow(clippy::unused_async)]
    pub async fn inclusion_proof(
        &self,
        commitment_request: CommitmentRequest,
    ) -> Result<Response<Body>, Error> {
        let merkle_tree = self.merkle_tree.read().unwrap();
        let proof = inclusion_proof_helper(&merkle_tree, &commitment_request.identity_commitment);
        println!("Proof: {:?}", proof);
        // TODO handle commitment not found
        let response = "Inclusion Proof!\n"; // TODO: proof
        Ok(Response::new(response.into()))
    }

    #[allow(clippy::unused_async)]
    pub async fn insert_identity(
        &self,
        commitment_request: CommitmentRequest,
    ) -> Result<Response<Body>, Error> {
        {
            let mut merkle_tree = self.merkle_tree.write().unwrap();
            let last_leaf = self.last_leaf.fetch_add(1, Ordering::AcqRel);
            let root = merkle_tree.root();
            let root = U256::from_bytes_be(&root);
            // let root = hex::encode(root);
            println!("Merkle tree root {:?}", root);
            insert_identity_commitment(
                &mut merkle_tree,
                &commitment_request.identity_commitment,
                last_leaf,
            );
            let root = merkle_tree.root();
            let root = hex::encode(root);
            println!("After Merkle tree root {:?}", root);
        }

        insert_identity_to_contract(&commitment_request.identity_commitment)
            .await
            .unwrap();
        Ok(Response::new("Insert Identity!\n".into()))
    }
}

#[derive(Debug)]
pub enum Error {
    InvalidMethod,
    InvalidContentType,
}

#[allow(clippy::fallible_impl_from)]
impl From<serde_json::Error> for Error {
    fn from(_error: serde_json::Error) -> Self {
        todo!()
    }
}

#[allow(clippy::fallible_impl_from)]
impl From<hyper::Error> for Error {
    fn from(_error: hyper::Error) -> Self {
        todo!()
    }
}

#[allow(clippy::fallible_impl_from)]
impl From<Error> for hyper::Error {
    fn from(_error: Error) -> Self {
        todo!()
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

#[allow(clippy::unused_async)] // We are implementing an interface
async fn route(request: Request<Body>, app: Arc<App>) -> Result<Response<Body>, hyper::Error> {
    // Measure and log request
    let _timer = LATENCY.start_timer(); // Observes on drop
    REQUESTS.inc();
    trace!(url = %request.uri(), "Receiving request");

    // Route requests
    let response = match (request.method(), request.uri().path()) {
        (&Method::GET, "/inclusionProof") => {
            json_middleware(request, |c| app.inclusion_proof(c)).await?
        }
        (&Method::POST, "/insertIdentity") => {
            json_middleware(request, |c| app.insert_identity(c)).await?
        }
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

pub async fn main(options: Options, shutdown: broadcast::Sender<()>) -> EyreResult<()> {
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

    let app = Arc::new(App::new(NUM_LEVELS).await);

    let make_svc = make_service_fn(move |_| {
        // Clone here as `make_service_fn` is called for every connection
        let app = app.clone();
        async {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                // Clone here as `service_fn` is called for every request
                let app = app.clone();
                route(req, app) // commitments, last_index)
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
    use hyper::{body::to_bytes, Request};
    use pretty_assertions::assert_eq;

    // #[tokio::test]
    // async fn test_hello_world() {
    //     let app = Arc::new(App::new(2));
    //     let request = CommitmentRequest {
    //         identity_commitment:
    // "24C94355810D659EEAA9E0B9E21F831493B50574AA2D3205F0AAB779E2864623"
    //             .to_string(),
    //     };
    //     let response = app.insert_identity(request).await.unwrap();
    //     let bytes = to_bytes(response.into_body()).await.unwrap();
    //     assert_eq!(bytes.as_ref(), b"Insert Identity!\n");
    // }
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
