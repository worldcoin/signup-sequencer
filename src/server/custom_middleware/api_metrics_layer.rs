use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use once_cell::sync::Lazy;
use prometheus::{
    opts, register_counter, register_histogram, register_int_counter_vec, Counter, Histogram,
    IntCounterVec,
};

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

pub async fn middleware<B>(request: Request<B>, next: Next<B>) -> Result<Response, StatusCode> {
    let _timer = LATENCY.start_timer(); // Observes on drop
    REQUESTS.inc();

    let response = next.run(request).await;

    STATUS
        .with_label_values(&[response.status().as_str()])
        .inc();

    Ok(response)
}
