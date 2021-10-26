use crate::identity::*;

use ::prometheus::{opts, register_counter, register_histogram, Counter, Histogram};
use anyhow::{anyhow, Context as _, Result as AnyResult};
use hyper::{
    service::{make_service_fn, service_fn},
    Body, Method, Request, Response, Server,
};
use once_cell::sync::Lazy;
use prometheus::{register_int_counter_vec, IntCounterVec};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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

#[allow(clippy::unused_async)]
async fn hello_world(_req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    Ok(Response::new("Hello, World!\n".into()))
}

#[allow(clippy::unused_async)]
pub async fn inclusion_proof(
    _req: Request<Body>,
    commitments: Vec<String>,
    commitment: String,
) -> Result<Response<Body>, hyper::Error> {
    let index = commitments.binary_search(&commitment).unwrap();
    let proof = inclusion_proof_helper(commitments, index).unwrap();
    let response = format!("Inclusion Proof!\n {:?}", proof);
    Ok(Response::new(response.into()))
}

#[allow(clippy::unused_async)]
pub async fn insert_identity(_req: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    insert_identity_helper("".to_string(), vec![], 0);

    Ok(Response::new("Insert Identity!\n".into()))
}

#[allow(clippy::unused_async)] // We are implementing an interface
async fn route(request: Request<Body>) -> Result<Response<Body>, hyper::Error> {
    // Measure and log request
    let _timer = LATENCY.start_timer(); // Observes on drop
    REQUESTS.inc();
    trace!(url = %request.uri(), "Receiving request");

    // Route requests
    let response = match (request.method(), request.uri().path()) {
        (&Method::GET, "/") => hello_world(request).await?,
        (&Method::GET, "/inclusionProof") => {
            inclusion_proof(request, vec![], String::from("")).await?
        }
        (&Method::POST, "/insertIdentity") => insert_identity(request).await?,
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

    // TODO how to manage and pass state?
    let commitments = initialize_commitments();
    let mut last_index = 0;

    let server = Server::try_bind(&addr)
        .context("Could not bind server port")?
        .serve(make_service_fn(|_| async {
            Ok::<_, hyper::Error>(service_fn(route))
        }))
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
