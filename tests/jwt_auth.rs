//! Integration tests for JWT authentication

mod common;

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use common::prelude::*;
use common::{generate_es256_keypair, sign_jwt};
use maplit::hashmap;
use reqwest::header::AUTHORIZATION;
use signup_sequencer::config::AuthMode;
use signup_sequencer::utils::jwt::Claims;

fn future_exp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600
}

async fn setup_test_app_with_auth(
    auth_mode: AuthMode,
    authorized_keys: HashMap<String, String>,
    basic_auth_credentials: HashMap<String, String>,
) -> anyhow::Result<(
    std::sync::Arc<signup_sequencer::app::App>,
    JoinHandle<()>,
    std::net::SocketAddr,
    Shutdown,
    DockerContainer<'static>,
    tempfile::TempDir,
)> {
    let ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Box::leak(Box::new(Cli::default()));
    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) =
        spawn_deps(initial_root, &[3], &[], DEFAULT_TREE_DEPTH as u8, docker).await?;

    let prover_mock = &insertion_prover_map[&3];
    let db_socket_addr = db_container.address();
    let db_url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    let temp_dir = tempfile::tempdir()?;

    let mut config = TestConfigBuilder::new()
        .db_url(&db_url)
        .oz_api_url(&micro_oz.endpoint())
        .oz_address(micro_oz.address())
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(prover_mock)
        .offchain_mode(true)
        .build()?;

    config.server.auth_mode = auth_mode;
    config.server.authorized_keys = authorized_keys;
    config.server.basic_auth_credentials = basic_auth_credentials;

    let (app, app_handle, local_addr, shutdown) = spawn_app(config).await?;

    Ok((
        app,
        app_handle,
        local_addr,
        shutdown,
        db_container,
        temp_dir,
    ))
}

#[tokio::test]
async fn health_no_auth_required() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::JwtOnly, keys, hashmap! {}).await?;

    let client = Client::new();
    let response = client
        .get(format!("http://{}/health", local_addr))
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::OK);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn metrics_no_auth_required() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::JwtOnly, keys, hashmap! {}).await?;

    let client = Client::new();
    let response = client
        .get(format!("http://{}/metrics", local_addr))
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::OK);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn insert_identity_requires_auth_when_enforced() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::JwtOnly, keys, hashmap! {}).await?;

    let client = Client::new();
    let response = client
        .post(format!("http://{}/insertIdentity", local_addr))
        .header("Content-Type", "application/json")
        .body(
            r#"{"identityCommitment":"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"}"#,
        )
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn insert_identity_succeeds_with_valid_token() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (private_pem, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::JwtOnly, keys, hashmap! {}).await?;

    let claims = Claims::new("test_key", future_exp());
    let token = sign_jwt(&private_pem, &claims);

    let client = Client::new();
    let response = client
        .post(format!("http://{}/insertIdentity", local_addr))
        .header("Content-Type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {}", token))
        .body(
            r#"{"identityCommitment":"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"}"#,
        )
        .send()
        .await?;

    // Should not be 401 - actual status depends on whether the commitment is valid
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn auth_disabled_bypasses_all_checks() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    // Auth is disabled entirely
    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::Disabled, keys, hashmap! {}).await?;

    let client = Client::new();
    let response = client
        .post(format!("http://{}/insertIdentity", local_addr))
        .header("Content-Type", "application/json")
        .body(
            r#"{"identityCommitment":"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"}"#,
        )
        .send()
        .await?;

    // Should not be 401 since auth is disabled
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn basic_or_jwt_allows_with_basic_auth() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let basic_creds = hashmap! { "testuser".to_string() => "testpass".to_string() };

    // BasicOrJwt mode: either basic auth OR JWT is required
    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::BasicOrJwt, keys, basic_creds).await?;

    let client = Client::new();
    let response = client
        .post(format!("http://{}/insertIdentity", local_addr))
        .header("Content-Type", "application/json")
        .basic_auth("testuser", Some("testpass"))
        .body(
            r#"{"identityCommitment":"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"}"#,
        )
        .send()
        .await?;

    // Should not be 401 since basic auth is valid (allowed with warning)
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn basic_or_jwt_allows_with_jwt_only() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (private_pem, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let basic_creds = hashmap! { "testuser".to_string() => "testpass".to_string() };

    // BasicOrJwt mode: either basic auth OR JWT is required
    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::BasicOrJwt, keys, basic_creds).await?;

    let claims = Claims::new("test_key", future_exp());
    let token = sign_jwt(&private_pem, &claims);

    let client = Client::new();
    let response = client
        .post(format!("http://{}/insertIdentity", local_addr))
        .header("Content-Type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {}", token))
        .body(
            r#"{"identityCommitment":"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"}"#,
        )
        .send()
        .await?;

    // Should not be 401 since JWT is valid (no basic auth needed)
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn v2_insert_identity_requires_auth_when_enforced() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::JwtOnly, keys, hashmap! {}).await?;

    let commitment = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let client = Client::new();
    let response = client
        .post(format!(
            "http://{}/v2/identities/{}",
            local_addr, commitment
        ))
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn v2_delete_identity_requires_auth_when_enforced() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::JwtOnly, keys, hashmap! {}).await?;

    let commitment = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let client = Client::new();
    let response = client
        .delete(format!(
            "http://{}/v2/identities/{}",
            local_addr, commitment
        ))
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn inclusion_proof_no_auth_required() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let keys = hashmap! { "test_key".to_string() => public_pem };

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::JwtOnly, keys, hashmap! {}).await?;

    let commitment = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let client = Client::new();
    let response = client
        .get(format!(
            "http://{}/v2/identities/{}/inclusion-proof",
            local_addr, commitment
        ))
        .send()
        .await?;

    // Should not be 401 - will be 404 (not found) since commitment doesn't exist
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn wrong_key_rejected() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (wrong_private_pem, _) = generate_es256_keypair();
    let (_, correct_public_pem) = generate_es256_keypair();

    let keys = hashmap! { "test_key".to_string() => correct_public_pem };

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(AuthMode::JwtOnly, keys, hashmap! {}).await?;

    // Sign with wrong key
    let claims = Claims::new("test_key", future_exp());
    let token = sign_jwt(&wrong_private_pem, &claims);

    let client = Client::new();
    let response = client
        .post(format!("http://{}/insertIdentity", local_addr))
        .header("Content-Type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {}", token))
        .body(
            r#"{"identityCommitment":"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"}"#,
        )
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}
