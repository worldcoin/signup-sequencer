//! Regression coverage for Alloy authentication and HTTP transport compatibility.
use alloy::providers::{Provider, RootProvider};
use alloy::rpc::client::RpcClient;
use alloy::transports::http::{reqwest, Http};
use axum::{http::HeaderMap, routing::post, Json, Router};
use serde_json::{json, Value};

#[test]
fn relayer_wire_formats_are_unchanged() {
    use alloy::primitives::{Address, Bytes, U256};
    use oz_api::data::transactions::{NameOrAddress, SendBaseTransactionRequestOwned};
    let address = Address::repeat_byte(0x12);
    let tx = tx_sitter_client::data::SendTxRequest {
        to: address,
        value: U256::MAX,
        gas_limit: U256::from(21000),
        data: Some(Bytes::from_static(&[0xab, 0xcd])),
        ..Default::default()
    };
    let value = serde_json::to_value(&tx).unwrap();
    assert_eq!(value["value"], U256::MAX.to_string());
    assert_eq!(value["gasLimit"], "21000");
    assert_eq!(value["data"], "0xabcd");
    assert_eq!(value["to"], "0x1212121212121212121212121212121212121212");
    let decoded: tx_sitter_client::data::SendTxRequest = serde_json::from_value(value).unwrap();
    assert_eq!(decoded.value, U256::MAX);

    let oz = SendBaseTransactionRequestOwned {
        to: Some(NameOrAddress::Address(address)),
        value: Some(U256::ZERO),
        gas_limit: Some(U256::from(21000)),
        data: tx.data,
        valid_until: None,
    };
    let value = serde_json::to_value(oz).unwrap();
    assert_eq!(value["value"], "0x0");
    assert_eq!(value["gasLimit"], "0x5208");
    assert_eq!(value["data"], "0xabcd");
    assert!(value.get("validUntil").is_none());
    let name: NameOrAddress = serde_json::from_str("\"example.eth\"").unwrap();
    assert_eq!(serde_json::to_string(&name).unwrap(), "\"example.eth\"");
}

#[tokio::test]
async fn alloy_http_preserves_auth_and_json_rpc() {
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
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        "Bearer regression-test".parse().unwrap(),
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .build()
        .unwrap();
    let transport = Http::with_client(client, format!("http://{addr}").parse().unwrap());
    let provider: RootProvider = RootProvider::new(RpcClient::new(transport, true));
    assert_eq!(provider.get_chain_id().await.unwrap(), 1);
    server.abort();
}
