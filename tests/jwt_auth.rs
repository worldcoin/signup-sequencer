//! Integration tests for JWT authentication

mod common;

use common::prelude::*;
use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};
use reqwest::header::AUTHORIZATION;
use std::collections::HashMap;

/// Generates an ES256 key pair for testing.
/// Returns (private_key_pem, public_key_pem).
fn generate_es256_keypair() -> (String, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};

    // Generate private key in SEC1 format
    let sec1_key_output = Command::new("openssl")
        .args(["ecparam", "-genkey", "-name", "prime256v1", "-noout"])
        .output()
        .expect("Failed to generate private key");

    let sec1_key_pem =
        String::from_utf8(sec1_key_output.stdout).expect("Invalid UTF-8 in private key");

    // Convert to PKCS#8 format (required by jsonwebtoken)
    let mut pkcs8_process = Command::new("openssl")
        .args(["pkcs8", "-topk8", "-nocrypt"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn openssl");

    pkcs8_process
        .stdin
        .as_mut()
        .unwrap()
        .write_all(sec1_key_pem.as_bytes())
        .expect("Failed to write to stdin");

    let pkcs8_output = pkcs8_process
        .wait_with_output()
        .expect("Failed to convert to PKCS#8");

    let private_key_pem =
        String::from_utf8(pkcs8_output.stdout).expect("Invalid UTF-8 in private key");

    // Extract public key from the original SEC1 key
    let mut public_key_process = Command::new("openssl")
        .args(["ec", "-pubout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn openssl");

    public_key_process
        .stdin
        .as_mut()
        .unwrap()
        .write_all(sec1_key_pem.as_bytes())
        .expect("Failed to write to stdin");

    let public_output = public_key_process
        .wait_with_output()
        .expect("Failed to extract public key");

    let public_key_pem =
        String::from_utf8(public_output.stdout).expect("Invalid UTF-8 in public key");

    (private_key_pem, public_key_pem)
}

/// Signs a JWT with the given private key and claims.
fn sign_jwt(private_key_pem: &str, claims: serde_json::Value) -> String {
    let encoding_key = EncodingKey::from_ec_pem(private_key_pem.as_bytes())
        .expect("Failed to create encoding key");

    let header = Header::new(Algorithm::ES256);
    encode(&header, &claims, &encoding_key).expect("Failed to encode JWT")
}

async fn setup_test_app_with_auth(
    require_auth: bool,
    auth_enabled: bool,
    authorized_keys: HashMap<String, String>,
) -> anyhow::Result<(std::sync::Arc<signup_sequencer::app::App>, JoinHandle<()>, std::net::SocketAddr, Shutdown, DockerContainer<'static>, tempfile::TempDir)> {
    let ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Box::leak(Box::new(Cli::default()));
    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) = spawn_deps(
        initial_root,
        &[3],
        &[],
        DEFAULT_TREE_DEPTH as u8,
        docker,
    )
    .await?;

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

    config.server.authorized_keys = authorized_keys;
    config.server.auth_enabled = auth_enabled;
    config.server.require_auth = require_auth;

    let (app, app_handle, local_addr, shutdown) = spawn_app(config).await?;

    Ok((app, app_handle, local_addr, shutdown, db_container, temp_dir))
}

#[tokio::test]
async fn health_no_auth_required() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, true, keys).await?;

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
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, true, keys).await?;

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
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, true, keys).await?;

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
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, true, keys).await?;

    let token = sign_jwt(&private_pem, json!({"sub": "test"}));

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
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    // Auth is disabled entirely
    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, false, keys).await?;

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
async fn require_auth_false_allows_unauthenticated_requests() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    // Auth enabled but not required (soft rollout mode)
    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(false, true, keys).await?;

    let client = Client::new();
    let response = client
        .post(format!("http://{}/insertIdentity", local_addr))
        .header("Content-Type", "application/json")
        .body(
            r#"{"identityCommitment":"0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef"}"#,
        )
        .send()
        .await?;

    // Should not be 401 since require_auth=false
    assert_ne!(response.status(), StatusCode::UNAUTHORIZED);

    shutdown.shutdown();
    app_handle.await?;

    Ok(())
}

#[tokio::test]
async fn v2_insert_identity_requires_auth_when_enforced() -> anyhow::Result<()> {
    init_tracing_subscriber();

    let (_, public_pem) = generate_es256_keypair();
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, true, keys).await?;

    let commitment = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let client = Client::new();
    let response = client
        .post(format!("http://{}/v2/identities/{}", local_addr, commitment))
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
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, true, keys).await?;

    let commitment = "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let client = Client::new();
    let response = client
        .delete(format!("http://{}/v2/identities/{}", local_addr, commitment))
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
    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), public_pem);

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, true, keys).await?;

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

    let mut keys = HashMap::new();
    keys.insert("test_key".to_string(), correct_public_pem);

    let (_app, app_handle, local_addr, shutdown, _db, _temp) =
        setup_test_app_with_auth(true, true, keys).await?;

    // Sign with wrong key
    let token = sign_jwt(&wrong_private_pem, json!({"sub": "test"}));

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
