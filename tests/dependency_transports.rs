//! Regression coverage for the patched Ethers transport dependencies.
use axum::{http::HeaderMap, routing::post, Json, Router};
use ethers::providers::{Authorization, Http, JsonRpcClient, JwtAuth, JwtKey};
use serde_json::{json, Value};

#[tokio::test]
async fn ethers_http_preserves_auth_and_json_rpc() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/",
        post(
            |headers: HeaderMap, Json(request): Json<Value>| async move {
                assert_eq!(headers["authorization"], "Bearer regression-test");
                assert_eq!(request["method"], "eth_chainId");
                Json(json!({"jsonrpc": "2.0", "id": request["id"], "result": "0x1"}))
            },
        ),
    );
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let client = Http::new_with_auth(
        format!("http://{addr}").parse::<url::Url>().unwrap(),
        Authorization::bearer("regression-test"),
    )
    .unwrap();
    let result: String = client.request("eth_chainId", ()).await.unwrap();
    assert_eq!(result, "0x1");
    server.abort();
}

#[test]
fn ethers_jwt_accepts_valid_key_and_rejects_wrong_key() {
    let key = [42; 32];
    let auth = JwtAuth::new(JwtKey::from_slice(&key).unwrap(), None, None);
    let token = auth.generate_token().unwrap();
    assert!(JwtAuth::validate_token(&token, &JwtKey::from_slice(&key).unwrap()).is_ok());
    assert!(JwtAuth::validate_token(&token, &JwtKey::from_slice(&[43; 32]).unwrap()).is_err());
}
