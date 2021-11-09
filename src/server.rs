use crate::{identity::{
    inclusion_proof_helper, initialize_commitments, insert_identity_commitment,
    insert_identity_to_contract, Commitment,
}, merkle_tree::MerkleTree, mimc_tree::MimcTree};
use ::prometheus::{opts, register_counter, register_histogram, Counter, Histogram};
use eyre::{bail, ensure, Result as EyreResult, WrapErr as _};
use futures::Future;
use hyper::{
    body::Buf,
    header,
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server, StatusCode,
};
use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, IntCounterVec};
use serde::{Deserialize, Deserializer, de::{DeserializeOwned, MapAccess, Visitor}};
use serde_json::Map;
use std::{fmt, marker::PhantomData, net::{IpAddr, Ipv4Addr, SocketAddr}, sync::{atomic::AtomicUsize, Arc, RwLock}};
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
const IDENTITY_COMMITMENT_KEY: &str = "identityCommitment";
const CONTENT_JSON: &str = "application/json";
const NUM_LEVELS: usize = 4;

pub struct CommitmentRequest {
    identity_commitment: String,
}

struct CommitmentRequestVisitor<K, V> {
    marker: PhantomData<fn() -> Map<K, V>>
}

impl<K, V> CommitmentRequestVisitor<K, V> {
    fn new() -> Self {
        CommitmentRequestVisitor {
            marker: PhantomData
        }
    }
}


impl<'de, K, V> Visitor<'de> for CommitmentRequestVisitor<K, V>
where
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    // The type that our Visitor is going to produce.
    type Value = CommitmentRequest;

    // Format a message stating what data this Visitor expects to receive.
    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str(format!("a map with key {} and associated value.", IDENTITY_COMMITMENT_KEY).as_str())
    }

    fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
    where
        M: MapAccess<'de>,
    {
        let mut req = CommitmentRequest{identity_commitment: String::new()};

        // While there are entries remaining in the input, add them
        // into our map.
        // Note: we currently only expect one key so this is unnecessary, but sets us up for future use of a map struct
        while let Some((key, value)) = access.next_entry::<String, String>()? {
            if key == IDENTITY_COMMITMENT_KEY {
                req.identity_commitment = value;
            }
        }

        Ok(req)
    }
}

// This is the trait that informs Serde how to deserialize MyMap.
impl<'de> Deserialize<'de> for CommitmentRequest
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_map(CommitmentRequestVisitor::<String, String>::new())
    }
}


pub struct App {
    merkle_tree: MimcTree,
}

impl App {
    pub fn new(depth: usize) -> Self {
        App{merkle_tree: MimcTree::new(depth)}
    }

    #[allow(clippy::unused_async)]
    pub async fn inclusion_proof(
        &self,
        commitment_request: CommitmentRequest,
    ) -> Result<Response<Body>, Error> {
        println!("Identity commitment {}", commitment_request.identity_commitment);
        println!("Num leaves {}", self.merkle_tree.num_leaves());
        let _proof = inclusion_proof_helper(&self.merkle_tree, &commitment_request.identity_commitment);
        // let commitment = data["identityCommitment"].to_string();
        // let commitments = commitments.read().unwrap();
        // let _proof = inclusion_proof_helper(&commitment, &commitments).unwrap();
        // TODO handle commitment not found
        let response = "Inclusion Proof!\n"; // TODO: proof
        Ok(Response::new(response.into()))
    }

    #[allow(clippy::unused_async)]
    pub async fn insert_identity(
        &self,
        commitment_request: CommitmentRequest,
    ) -> Result<Response<Body>, Error> {

        // {
        //     let mut commitments = commitments.write().unwrap();
        //     insert_identity_commitment(
        //         &identity_commitment.to_string(),
        //         &mut commitments,
        //         &last_index,
        //     );
        // }

        insert_identity_to_contract(&commitment_request.identity_commitment)
            .await
            .unwrap();
        Ok(Response::new("Insert Identity!\n".into()))
    }
}

pub enum Error {
    InvalidMethod,
    InvalidContentType,
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        println!("Serde error {}", error);
        todo!()
    }
}

impl From<hyper::Error> for Error {
    fn from(_error: hyper::Error) -> Self {
        todo!()
    }
}

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
    // TODO seems unnecessary as the handler passing this here already qualifies the method
    // if request.method() != Method::POST {
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
async fn route(
    request: Request<Body>,
    app: Arc<App>,
) -> Result<Response<Body>, hyper::Error> {
    // Measure and log request
    let _timer = LATENCY.start_timer(); // Observes on drop
    REQUESTS.inc();
    trace!(url = %request.uri(), "Receiving request");

    // Route requests
    let response = match (request.method(), request.uri().path()) {
        (&Method::GET, "/inclusionProof") => json_middleware(request, |c| app.inclusion_proof(c)).await?,
        (&Method::POST, "/insertIdentity") => json_middleware(request, |c| app.insert_identity(c)).await?,
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

    let app = Arc::new(App::new(NUM_LEVELS));

    let make_svc = make_service_fn(move |_| {
        // Clone here as `make_service_fn` is called for every connection
        let app = app.clone();
        async {
            Ok::<_, hyper::Error>(service_fn(move |req| {
                // Clone here as `service_fn` is called for every request
                let app = app.clone();
                route(req, app)// commitments, last_index)
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

    #[tokio::test]
    async fn test_hello_world() {
        // let request = Request::new(Body::empty());
        // let response = hello_world(request).await.unwrap();
        // let bytes = to_bytes(response.into_body()).await.unwrap();
        // assert_eq!(bytes.as_ref(), b"Hello, World!\n");
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
