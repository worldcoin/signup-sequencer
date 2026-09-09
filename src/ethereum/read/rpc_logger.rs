use std::fmt::Debug;

use ::prometheus::{register_histogram, register_int_counter_vec, Histogram, IntCounterVec};
use alloy::rpc::json_rpc::{RequestPacket, ResponsePacket};
use alloy::transports::{TransportError, TransportFut};
use once_cell::sync::Lazy;
use std::task::{Context, Poll};
use tower::Service;

static REQUESTS: Lazy<IntCounterVec> = Lazy::new(|| {
    register_int_counter_vec!(
        "eth_rpc_requests",
        "Number of Ethereum provider requests made by method.",
        &["method"]
    )
    .unwrap()
});
static LATENCY: Lazy<Histogram> = Lazy::new(|| {
    register_histogram!(
        "eth_rpc_latency_seconds",
        "The Ethereum provider latency in seconds."
    )
    .unwrap()
});

#[derive(Debug, Clone)]
pub struct RpcLogger<Inner> {
    inner: Inner,
}

impl<Inner> RpcLogger<Inner> {
    pub const fn new(inner: Inner) -> Self {
        Self { inner }
    }
}

impl<Inner> Service<RequestPacket> for RpcLogger<Inner>
where
    Inner:
        Service<RequestPacket, Response = ResponsePacket, Error = TransportError> + Send + 'static,
    Inner::Future: Send + 'static,
{
    type Response = ResponsePacket;
    type Error = TransportError;
    type Future = TransportFut<'static>;
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }
    fn call(&mut self, request: RequestPacket) -> Self::Future {
        match &request {
            RequestPacket::Single(req) => REQUESTS.with_label_values(&[req.method()]).inc(),
            RequestPacket::Batch(reqs) => {
                for req in reqs {
                    REQUESTS.with_label_values(&[req.method()]).inc();
                }
            }
        }
        let timer = LATENCY.start_timer();
        let future = self.inner.call(request);
        Box::pin(async move {
            let result = future.await;
            timer.observe_duration();
            result
        })
    }
}
