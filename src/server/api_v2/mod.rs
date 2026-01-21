use crate::app::error::{
    DeleteIdentityV2Error, InclusionProofV2Error, InsertIdentityV2Error,
    VerifySemaphoreProofV2Error,
};
use crate::app::App;
use crate::identity_tree::Hash;
use crate::server::api_v2::data::{
    ErrorResponse, InclusionProofResponse, VerifySemaphoreProofRequest,
    VerifySemaphoreProofResponse,
};
use crate::utils::auth::AuthValidator;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{middleware, Json, Router};
use error::Error;
use prometheus::{Encoder, TextEncoder};
use regex::Regex;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::log::error;

mod custom_middleware;
mod data;
mod error;

async fn insert_identity(
    State(app): State<Arc<App>>,
    Path(raw_commitment): Path<String>,
) -> Result<StatusCode, Error> {
    let commitment = parse_commitment(raw_commitment)?;

    app.insert_identity_v2(commitment)
        .await
        .map_err(|err| match err {
            InsertIdentityV2Error::InvalidCommitment => {
                Error::BadRequest(ErrorResponse::new("invalid_commitment", &err.to_string()))
            }
            InsertIdentityV2Error::UnreducedCommitment => {
                Error::BadRequest(ErrorResponse::new("unreduced_commitment", &err.to_string()))
            }
            InsertIdentityV2Error::DuplicateCommitment => {
                Error::Conflict(ErrorResponse::new("duplicate_commitment", &err.to_string()))
            }
            InsertIdentityV2Error::DeletedCommitment => {
                Error::Gone(ErrorResponse::new("deleted_commitment", &err.to_string()))
            }
            InsertIdentityV2Error::Database(_) | InsertIdentityV2Error::Sqlx(_) => {
                error!("Database error: {}", err);
                Error::InternalServerError(ErrorResponse::new(
                    "internal_database_error",
                    &err.to_string(),
                ))
            }
        })?;

    Ok(StatusCode::ACCEPTED)
}

async fn delete_identity(
    State(app): State<Arc<App>>,
    Path(raw_commitment): Path<String>,
) -> Result<StatusCode, Error> {
    let commitment = parse_commitment(raw_commitment)?;

    app.delete_identity_v2(commitment)
        .await
        .map_err(|err| match err {
            DeleteIdentityV2Error::InvalidCommitment => {
                Error::BadRequest(ErrorResponse::new("invalid_commitment", &err.to_string()))
            }
            DeleteIdentityV2Error::UnreducedCommitment => {
                Error::BadRequest(ErrorResponse::new("unreduced_commitment", &err.to_string()))
            }
            DeleteIdentityV2Error::UnprocessedCommitment => Error::Conflict(ErrorResponse::new(
                "unprocessed_commitment",
                &err.to_string(),
            )),
            DeleteIdentityV2Error::CommitmentNotFound => {
                Error::NotFound(ErrorResponse::new("not_found", &err.to_string()))
            }
            DeleteIdentityV2Error::DeletedCommitment => {
                Error::Gone(ErrorResponse::new("deleted_commitment", &err.to_string()))
            }
            DeleteIdentityV2Error::DuplicateCommitmentDeletion => Error::Conflict(
                ErrorResponse::new("duplicate_commitment_deletion", &err.to_string()),
            ),
            DeleteIdentityV2Error::Database(_) | DeleteIdentityV2Error::Sqlx(_) => {
                error!("Database error: {}", err);
                Error::InternalServerError(ErrorResponse::new(
                    "internal_database_error",
                    &err.to_string(),
                ))
            }
        })?;

    Ok(StatusCode::ACCEPTED)
}

async fn inclusion_proof(
    State(app): State<Arc<App>>,
    Path(raw_commitment): Path<String>,
) -> Result<(StatusCode, Json<InclusionProofResponse>), Error> {
    let commitment = parse_commitment(raw_commitment)?;

    let (_, root, proof) = app
        .inclusion_proof_v2(commitment)
        .await
        .map_err(|err| match err {
            InclusionProofV2Error::InvalidCommitment => {
                Error::BadRequest(ErrorResponse::new("invalid_commitment", &err.to_string()))
            }
            InclusionProofV2Error::UnreducedCommitment => {
                Error::BadRequest(ErrorResponse::new("unreduced_commitment", &err.to_string()))
            }
            InclusionProofV2Error::UnprocessedCommitment => Error::Conflict(ErrorResponse::new(
                "unprocessed_commitment",
                &err.to_string(),
            )),
            InclusionProofV2Error::CommitmentNotFound => {
                Error::NotFound(ErrorResponse::new("not_found", &err.to_string()))
            }
            InclusionProofV2Error::DeletedCommitment => {
                Error::Gone(ErrorResponse::new("deleted_commitment", &err.to_string()))
            }
            InclusionProofV2Error::Database(_) | InclusionProofV2Error::Sqlx(_) => {
                error!("Database error: {}", err);
                Error::InternalServerError(ErrorResponse::new(
                    "internal_database_error",
                    &err.to_string(),
                ))
            }
            InclusionProofV2Error::InvalidInternalState | InclusionProofV2Error::AnyhowError(_) => {
                error!("Error: {}", err);
                Error::InternalServerError(ErrorResponse::new("internal_error", &err.to_string()))
            }
        })?;

    Ok((StatusCode::OK, Json(InclusionProofResponse { root, proof })))
}

async fn verify_semaphore_proof(
    State(app): State<Arc<App>>,
    Json(request): Json<VerifySemaphoreProofRequest>,
) -> Result<(StatusCode, Json<VerifySemaphoreProofResponse>), Error> {
    let valid = app
        .verify_semaphore_proof_v2(
            request.root,
            request.signal_hash,
            request.nullifier_hash,
            request.external_nullifier_hash,
            request.proof,
            request.max_root_age_seconds,
        )
        .await
        .map_err(|err| match err {
            VerifySemaphoreProofV2Error::InvalidRoot => {
                Error::BadRequest(ErrorResponse::new("invalid_root", &err.to_string()))
            }
            VerifySemaphoreProofV2Error::RootTooOld => {
                Error::BadRequest(ErrorResponse::new("root_too_old", &err.to_string()))
            }
            VerifySemaphoreProofV2Error::DecompressingProofError => Error::BadRequest(
                ErrorResponse::new("decompressing_proof_error", &err.to_string()),
            ),
            VerifySemaphoreProofV2Error::ProverError => {
                Error::InternalServerError(ErrorResponse::new("prover_error", &err.to_string()))
            }
            VerifySemaphoreProofV2Error::RootAgeCheckingError(_) => Error::InternalServerError(
                ErrorResponse::new("root_age_checking_error", &err.to_string()),
            ),
            VerifySemaphoreProofV2Error::Database(_) => {
                error!("Database error: {}", err);
                Error::InternalServerError(ErrorResponse::new(
                    "internal_database_error",
                    &err.to_string(),
                ))
            }
        })?;

    Ok((StatusCode::OK, Json(VerifySemaphoreProofResponse { valid })))
}

async fn health() -> Result<(), Error> {
    Ok(())
}

async fn metrics() -> Result<Response<Body>, Error> {
    let encoder = TextEncoder::new();

    let metric_families = prometheus::gather();
    let mut buffer = vec![];
    encoder
        .encode(&metric_families, &mut buffer)
        .map_err(|err| {
            Error::InternalServerError(ErrorResponse::new("metrics_error", &err.to_string()))
        })?;

    Ok((
        StatusCode::OK,
        [(
            CONTENT_TYPE,
            HeaderValue::from_str(encoder.format_type()).map_err(|err| {
                Error::InternalServerError(ErrorResponse::new("metrics_error", &err.to_string()))
            })?,
        )]
        .into_iter()
        .collect::<HeaderMap>(),
        Body::from(buffer),
    )
        .into_response())
}

fn parse_commitment(raw_commitment: String) -> Result<Hash, Error> {
    let re = Regex::new(r"^(0x)?[a-fA-F0-9]{1,64}$").unwrap();
    if !re.is_match(&raw_commitment) {
        return Err(
            Error::BadRequest(ErrorResponse::new(
                "invalid_path_param",
                &format!(
                    "Path parameter 'commitment' has invalid value '{}' which doesn't match required format '{}'",
                    raw_commitment,
                    re
                ),
            ))
        );
    }

    let commitment = if raw_commitment.starts_with("0x") {
        raw_commitment.clone()
    } else {
        format!("0x{}", raw_commitment)
    };

    Hash::from_str(&commitment).map_err(|parse_err| {
        Error::BadRequest(ErrorResponse::new(
            "invalid_path_param",
            &format!(
                "Path parameter 'commitment' has invalid value '{}': {}",
                raw_commitment, parse_err
            ),
        ))
    })
}

pub fn api_v2_router(
    app: Arc<App>,
    serve_timeout: Duration,
    auth_validator: Option<AuthValidator>,
) -> Router {
    // Protected routes that require authentication
    // Note: POST and DELETE on /v2/identities/:commitment need auth
    // Apply remove_auth FIRST (inner), then auth LAST (outer) so auth runs before remove_auth
    let protected_routes = Router::new()
        .route(
            "/v2/identities/:commitment",
            post(insert_identity).delete(delete_identity),
        )
        .layer(middleware::from_fn(
            custom_middleware::remove_auth_layer::middleware,
        ));

    // Apply auth layer to protected routes if validator is configured
    let protected_routes = if let Some(validator) = auth_validator {
        protected_routes.layer(middleware::from_fn_with_state(
            validator,
            custom_middleware::auth_layer::middleware,
        ))
    } else {
        protected_routes
    };

    // Public routes that don't require authentication
    let public_routes = Router::new()
        .route(
            "/v2/identities/:commitment/inclusion-proof",
            get(inclusion_proof),
        )
        .route("/v2/semaphore-proof/verify", post(verify_semaphore_proof))
        .route("/v2/health", get(health))
        .route("/v2/metrics", get(metrics))
        .layer(middleware::from_fn(
            custom_middleware::remove_auth_layer::middleware,
        ));

    // Merge protected and public routes
    Router::new()
        .merge(protected_routes)
        .merge(public_routes)
        .layer(middleware::from_fn_with_state(
            serve_timeout,
            custom_middleware::timeout_layer::middleware,
        ))
        .layer(middleware::from_fn(
            custom_middleware::logging_layer::middleware,
        ))
        .with_state(app.clone())
}

#[cfg(test)]
mod test {
    use crate::app::App;
    use crate::config::{
        default, AppConfig, AuthMode, Config, DatabaseConfig, OffchainModeConfig,
        ServerConfig, ServiceConfig, TreeConfig,
    };
    use crate::database::methods::DbMethods;
    use crate::database::IsolationLevel;
    use crate::identity_tree::db_sync::sync_tree;
    use crate::identity_tree::{Hash, TreeVersionReadOps};
    use crate::server::api_v2::api_v2_router;
    use crate::utils::secret::SecretUrl;
    use axum::http::StatusCode;
    use axum_test::{TestRequest, TestServer};
    use postgres_docker_utils::DockerContainer;
    use semaphore_rs::hash_to_field;
    use semaphore_rs::identity::Identity;
    use semaphore_rs::poseidon_tree::Proof;
    use semaphore_rs::protocol::generate_proof;
    use semaphore_rs_poseidon::poseidon;
    use serde_json::json;
    use std::net::SocketAddr;
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;
    use testcontainers::clients::Cli;
    use tokio::time::sleep;

    async fn setup_server(
        docker: &Cli,
    ) -> anyhow::Result<(TestServer, Arc<App>, DockerContainer, TempDir)> {
        let (db_config, db_container) = setup_db(docker).await?;

        let temp_dir = tempfile::tempdir()?;

        let config = Config {
            app: AppConfig {
                provers_urls: vec![].into(),
                batch_insertion_timeout: default::batch_insertion_timeout(),
                batch_deletion_timeout: default::batch_deletion_timeout(),
                min_batch_deletion_size: default::min_batch_deletion_size(),
                scanning_window_size: default::scanning_window_size(),
                scanning_chain_head_offset: default::scanning_chain_head_offset(),
                time_between_scans: default::time_between_scans(),
                monitored_txs_capacity: default::monitored_txs_capacity(),
                shutdown_timeout: default::shutdown_timeout(),
                shutdown_delay: default::shutdown_delay(),
            },
            tree: TreeConfig {
                tree_depth: default::tree_depth(),
                dense_tree_prefix_depth: 8,
                tree_gc_threshold: default::tree_gc_threshold(),
                cache_file: temp_dir
                    .path()
                    .join("testfile")
                    .to_str()
                    .unwrap()
                    .to_string(),
                force_cache_purge: default::force_cache_purge(),
                initial_leaf_value: default::initial_leaf_value(),
            },
            network: None,
            providers: None,
            relayer: None,
            database: db_config,
            server: ServerConfig {
                address: SocketAddr::from(([127, 0, 0, 1], 0)),
                serve_timeout: default::serve_timeout(),
                auth_mode: AuthMode::Disabled,
                basic_auth_credentials: Default::default(),
                authorized_keys: Default::default(),
            },
            service: ServiceConfig::default(),
            offchain_mode: OffchainModeConfig { enabled: true },
        };

        let app = App::new(config).await?;

        app.clone().init_tree().await?;

        Ok((
            TestServer::new(api_v2_router(app.clone(), Duration::from_secs(300), None))?,
            app,
            db_container,
            temp_dir,
        ))
    }

    async fn setup_db(docker: &Cli) -> anyhow::Result<(DatabaseConfig, DockerContainer)> {
        let db_container = postgres_docker_utils::setup(docker).await?;
        let url = format!(
            "postgres://postgres:postgres@{}/database",
            db_container.address()
        );

        let db_config = DatabaseConfig {
            database: SecretUrl::from_str(&url)?,
            migrate: true,
            max_connections: 1,
        };

        Ok((db_config, db_container))
    }

    async fn add_identity(app: Arc<App>, commitment: &str) -> anyhow::Result<()> {
        app.database
            .insert_unprocessed_identity(Hash::from_str(commitment)?)
            .await?;

        Ok(())
    }

    async fn pending_identity(app: Arc<App>, commitment: &str) -> anyhow::Result<(Hash, Proof)> {
        let commitment = Hash::from_str(commitment)?;
        let (pre_root, res) = {
            let tree_state = app.tree_state().await?;
            let tree = tree_state.latest_tree();
            (tree.get_root(), tree.simulate_append_many(&[commitment]))
        };
        let (root, proof, leaf_index) = res.first().unwrap();

        app.database
            .insert_pending_identity(*leaf_index, &commitment, None, root, &pre_root)
            .await?;

        app.database.trim_unprocessed().await?;

        {
            let mut tx = app
                .database
                .begin_tx(IsolationLevel::RepeatableRead)
                .await?;
            sync_tree(&mut tx, &app.tree_state().await?).await?;
            tx.commit().await?;
        }

        Ok((*root, proof.clone()))
    }

    async fn delete_identity(app: Arc<App>, commitment: &str) -> anyhow::Result<(Hash, Proof)> {
        let commitment = Hash::from_str(commitment)?;
        let tree_item = app.database.get_tree_item(&commitment).await?.unwrap();
        let (pre_root, res) = {
            let tree_state = app.tree_state().await?;
            let tree = tree_state.latest_tree();
            (
                tree.get_root(),
                tree.simulate_delete_many(&[tree_item.leaf_index]),
            )
        };
        let (root, proof) = res.first().unwrap();

        app.database
            .insert_pending_identity(tree_item.leaf_index, &Hash::ZERO, None, root, &pre_root)
            .await?;

        {
            let mut tx = app
                .database
                .begin_tx(IsolationLevel::RepeatableRead)
                .await?;
            sync_tree(&mut tx, &app.tree_state().await?).await?;
            tx.commit().await?;
        }

        Ok((*root, proof.clone()))
    }

    async fn process_root(app: Arc<App>, root: &Hash) -> anyhow::Result<()> {
        app.database.mark_root_as_processed(root).await?;

        {
            let mut tx = app
                .database
                .begin_tx(IsolationLevel::RepeatableRead)
                .await?;
            sync_tree(&mut tx, &app.tree_state().await?).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn mine_root(app: Arc<App>, root: &Hash) -> anyhow::Result<()> {
        app.database.mark_root_as_mined(root).await?;

        {
            let mut tx = app
                .database
                .begin_tx(IsolationLevel::RepeatableRead)
                .await?;
            sync_tree(&mut tx, &app.tree_state().await?).await?;
            tx.commit().await?;
        }

        Ok(())
    }

    async fn test_invalid_commitment_path_param(
        server: &TestServer,
        query: fn(&TestServer, &str) -> TestRequest,
    ) {
        for commitment in [
            "abcz",
            "abcdefgh",
            "0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0",
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef0",
        ] {
            let res = query(server, commitment).await;
            res.assert_status(StatusCode::BAD_REQUEST);
            res.assert_json(&json!({
                "errorId": "invalid_path_param",
                "errorMessage": format!("Path parameter 'commitment' has invalid value '{}' which doesn't match required format '^(0x)?[a-fA-F0-9]{{1,64}}$'", commitment),
            }));
        }
    }

    async fn test_unreduced_commitment(
        server: &TestServer,
        query: fn(&TestServer, &str) -> TestRequest,
    ) {
        for commitment in [
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
            "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
        ] {
            let res = query(server, commitment).await;
            res.assert_status(StatusCode::BAD_REQUEST);
            res.assert_json(&json!({
                "errorId": "unreduced_commitment",
                "errorMessage": "provided identity commitment is not in reduced form",
            }));
        }
    }

    async fn test_invalid_commitment(
        server: &TestServer,
        query: fn(&TestServer, &str) -> TestRequest,
    ) {
        for commitment in [
            "0000000000000000000000000000000000000000000000000000000000000000",
            "0x0000000000000000000000000000000000000000000000000000000000000000",
        ] {
            let res = query(server, commitment).await;
            res.assert_status(StatusCode::BAD_REQUEST);
            res.assert_json(&json!({
                "errorId": "invalid_commitment",
                "errorMessage": "provided identity commitment is invalid",
            }));
        }
    }

    #[tokio::test]
    async fn insert_identity_test() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (server, app, _db_container, _temp_dir) = setup_server(&docker).await?;

        let query = |server: &TestServer, commitment: &str| {
            server.post(&format!("/v2/identities/{}", commitment))
        };

        let test_success = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::ACCEPTED);
        };

        let test_duplicate = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::CONFLICT);
            res.assert_json(&json!({
                "errorId": "duplicate_commitment",
                "errorMessage": "provided identity commitment is already included",
            }));
        };

        let test_deleted = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::GONE);
            res.assert_json(&json!({
                "errorId": "deleted_commitment",
                "errorMessage": "provided identity commitment was deleted",
            }));
        };

        test_invalid_commitment_path_param(&server, query).await;
        test_unreduced_commitment(&server, query).await;
        test_invalid_commitment(&server, query).await;

        test_success("0x0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").await;
        test_success("1123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").await;

        test_duplicate("0x123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").await;
        test_duplicate("123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef").await;

        add_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await?;
        pending_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await?;
        test_duplicate("0x0000000000000000000000000000000000000000000000000000000000000001").await;

        add_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000002",
        )
        .await?;
        pending_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000002",
        )
        .await?;
        add_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000003",
        )
        .await?;
        pending_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000003",
        )
        .await?;
        let (root, _) = delete_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000002",
        )
        .await?;
        process_root(app.clone(), &root).await?;
        mine_root(app.clone(), &root).await?;

        test_deleted("0x0000000000000000000000000000000000000000000000000000000000000002").await;

        Ok(())
    }

    #[tokio::test]
    async fn delete_identity_test() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (server, app, _db_container, _temp_dir) = setup_server(&docker).await?;

        let query = |server: &TestServer, commitment: &str| {
            server.delete(&format!("/v2/identities/{}", commitment))
        };

        let test_success = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::ACCEPTED);
        };

        let test_unprocessed = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::CONFLICT);
            res.assert_json(&json!({
                "errorId": "unprocessed_commitment",
                "errorMessage": "provided identity commitment is not yet added to the tree",
            }));
        };

        let test_not_found = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::NOT_FOUND);
            res.assert_json(&json!({
                "errorId": "not_found",
                "errorMessage": "provided identity commitment was not found",
            }));
        };

        let test_duplicated_deletion = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::CONFLICT);
            res.assert_json(&json!({
                "errorId": "duplicate_commitment_deletion",
                "errorMessage": "provided identity commitment already scheduled for deletion",
            }));
        };

        let test_deleted = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::GONE);
            res.assert_json(&json!({
                "errorId": "deleted_commitment",
                "errorMessage": "provided identity commitment was deleted",
            }));
        };

        test_invalid_commitment_path_param(&server, query).await;
        test_unreduced_commitment(&server, query).await;
        test_invalid_commitment(&server, query).await;

        add_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await?;

        test_unprocessed("0x0000000000000000000000000000000000000000000000000000000000000001")
            .await;
        test_not_found("0x0000000000000000000000000000000000000000000000000000000000000002").await;

        pending_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await?;

        test_success("0x0000000000000000000000000000000000000000000000000000000000000001").await;
        test_duplicated_deletion(
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await;

        let (root, _) = delete_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await?;
        process_root(app.clone(), &root).await?;
        mine_root(app.clone(), &root).await?;

        test_deleted("0x0000000000000000000000000000000000000000000000000000000000000001").await;

        Ok(())
    }

    #[tokio::test]
    async fn get_identity_inclusion_proof_test() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (server, app, _db_container, _temp_dir) = setup_server(&docker).await?;

        let query = |server: &TestServer, commitment: &str| {
            server.get(&format!("/v2/identities/{}/inclusion-proof", commitment))
        };

        let test_success = async |commitment, result_json| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::OK);
            res.assert_json(&result_json);
        };

        let test_unprocessed = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::CONFLICT);
            res.assert_json(&json!({
                "errorId": "unprocessed_commitment",
                "errorMessage": "provided identity commitment is not yet added to the tree",
            }));
        };

        let test_not_found = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::NOT_FOUND);
            res.assert_json(&json!({
                "errorId": "not_found",
                "errorMessage": "provided identity commitment was not found",
            }));
        };

        let test_deleted = async |commitment| {
            let res = query(&server, commitment).await;
            res.assert_status(StatusCode::GONE);
            res.assert_json(&json!({
                "errorId": "deleted_commitment",
                "errorMessage": "provided identity commitment was deleted",
            }));
        };

        test_invalid_commitment_path_param(&server, query).await;
        test_unreduced_commitment(&server, query).await;
        test_invalid_commitment(&server, query).await;

        add_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await?;

        test_unprocessed("0x0000000000000000000000000000000000000000000000000000000000000001")
            .await;
        test_not_found("0x0000000000000000000000000000000000000000000000000000000000000002").await;

        pending_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await?;

        test_success(
            "0x0000000000000000000000000000000000000000000000000000000000000001",
            json!({
                "root": "0x2f7a6f1799731edf77832aca6ae90272d50719a2f6179ce47a882c10e569857d",
                "proof": [
                    { "Left": "0x0" },
                    { "Left": "0x2098f5fb9e239eab3ceac3f27b81e481dc3124d55ffed523a839ee8446b64864" },
                    { "Left": "0x1069673dcdb12263df301a6ff584a7ec261a44cb9dc68df067a4774460b1f1e1" },
                    { "Left": "0x18f43331537ee2af2e3d758d50f72106467c6eea50371dd528d57eb2b856d238" },
                    { "Left": "0x7f9d837cb17b0d36320ffe93ba52345f1b728571a568265caac97559dbc952a" },
                    { "Left": "0x2b94cf5e8746b3f5c9631f4c5df32907a699c58c94b2ad4d7b5cec1639183f55" },
                    { "Left": "0x2dee93c5a666459646ea7d22cca9e1bcfed71e6951b953611d11dda32ea09d78" },
                    { "Left": "0x78295e5a22b84e982cf601eb639597b8b0515a88cb5ac7fa8a4aabe3c87349d" },
                    { "Left": "0x2fa5e5f18f6027a6501bec864564472a616b2e274a41211a444cbe3a99f3cc61" },
                    { "Left": "0xe884376d0d8fd21ecb780389e941f66e45e7acce3e228ab3e2156a614fcd747" },
                    { "Left": "0x1b7201da72494f1e28717ad1a52eb469f95892f957713533de6175e5da190af2" },
                    { "Left": "0x1f8d8822725e36385200c0b201249819a6e6e1e4650808b5bebc6bface7d7636" },
                    { "Left": "0x2c5d82f66c914bafb9701589ba8cfcfb6162b0a12acf88a8d0879a0471b5f85a" },
                    { "Left": "0x14c54148a0940bb820957f5adf3fa1134ef5c4aaa113f4646458f270e0bfbfd0" },
                    { "Left": "0x190d33b12f986f961e10c0ee44d8b9af11be25588cad89d416118e4bf4ebe80c" },
                    { "Left": "0x22f98aa9ce704152ac17354914ad73ed1167ae6596af510aa5b3649325e06c92" },
                    { "Left": "0x2a7c7c9b6ce5880b9f6f228d72bf6a575a526f29c66ecceef8b753d38bba7323" },
                    { "Left": "0x2e8186e558698ec1c67af9c14d463ffc470043c9c2988b954d75dd643f36b992" },
                    { "Left": "0xf57c5571e9a4eab49e2c8cf050dae948aef6ead647392273546249d1c1ff10f" },
                    { "Left": "0x1830ee67b5fb554ad5f63d4388800e1cfe78e310697d46e43c9ce36134f72cca" },
                    { "Left": "0x2134e76ac5d21aab186c2be1dd8f84ee880a1e46eaf712f9d371b6df22191f3e" },
                    { "Left": "0x19df90ec844ebc4ffeebd866f33859b0c051d8c958ee3aa88f8f8df3db91a5b1" },
                    { "Left": "0x18cca2a66b5c0787981e69aefd84852d74af0e93ef4912b4648c05f722efe52b" },
                    { "Left": "0x2388909415230d1b4d1304d2d54f473a628338f2efad83fadf05644549d2538d" },
                    { "Left": "0x27171fb4a97b6cc0e9e8f543b5294de866a2af2c9c8d0b1d96e673e4529ed540" },
                    { "Left": "0x2ff6650540f629fd5711a0bc74fc0d28dcb230b9392583e5f8d59696dde6ae21" },
                    { "Left": "0x120c58f143d491e95902f7f5277778a2e0ad5168f6add75669932630ce611518" },
                    { "Left": "0x1f21feb70d3f21b07bf853d5e5db03071ec495a0a565a21da2d665d279483795" },
                    { "Left": "0x24be905fa71335e14c638cc0f66a8623a826e768068a9e968bb1a1dde18a72d2" },
                    { "Left": "0xf8666b62ed17491c50ceadead57d4cd597ef3821d65c328744c74e553dac26d" },
                ],
            })
        ).await;

        let (root, _) = delete_identity(
            app.clone(),
            "0x0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await?;
        process_root(app.clone(), &root).await?;
        mine_root(app.clone(), &root).await?;

        test_deleted("0x0000000000000000000000000000000000000000000000000000000000000001").await;

        Ok(())
    }

    #[tokio::test]
    async fn verify_semaphore_proof_test() -> anyhow::Result<()> {
        let docker = Cli::default();
        let (server, app, _db_container, _temp_dir) = setup_server(&docker).await?;

        let query = |server: &TestServer, json_body: &serde_json::Value| {
            server.post("/v2/semaphore-proof/verify").json(json_body)
        };

        let test_success = async |json_body| {
            let res = query(&server, json_body).await;
            res.assert_status(StatusCode::OK);
            res.assert_json(&json!(
                { "valid": true }
            ));
        };

        let test_invalid = async |json_body| {
            let res = query(&server, json_body).await;
            res.assert_status(StatusCode::OK);
            res.assert_json(&json!(
                { "valid": false }
            ));
        };

        let test_invalid_root = async |json_body| {
            let res = query(&server, json_body).await;
            res.assert_status(StatusCode::BAD_REQUEST);
            res.assert_json(&json!({
                "errorId": "invalid_root",
                "errorMessage": "provided root is invalid",
            }));
        };

        let test_root_too_old = async |json_body| {
            let res = query(&server, json_body).await;
            res.assert_status(StatusCode::BAD_REQUEST);
            res.assert_json(&json!({
                "errorId": "root_too_old",
                "errorMessage": "provided root is too old",
            }));
        };

        let test_decompressing_proof_error = async |json_body| {
            let res = query(&server, json_body).await;
            res.assert_status(StatusCode::BAD_REQUEST);
            res.assert_json(&json!({
                "errorId": "decompressing_proof_error",
                "errorMessage": "cannot decompress provided proof",
            }));
        };

        let test_prover_error = async |json_body| {
            let res = query(&server, json_body).await;
            res.assert_status(StatusCode::INTERNAL_SERVER_ERROR);
            res.assert_json(&json!({
                "errorId": "prover_error",
                "errorMessage": "prover error",
            }));
        };

        let mut bytes: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 1];
        let identity = Identity::from_secret(&mut bytes, None);
        let signal_hash = hash_to_field(b"signal_hash_1");
        let external_nullifier_hash = hash_to_field(b"external_hash_1");
        let nullifier_hash = poseidon::hash2(external_nullifier_hash, identity.nullifier);
        let commitment = identity.commitment();

        add_identity(app.clone(), &commitment.to_string()).await?;

        let (root, inclusion_proof) =
            pending_identity(app.clone(), &commitment.to_string()).await?;

        let proof = generate_proof(
            &identity,
            &inclusion_proof,
            external_nullifier_hash,
            signal_hash,
        )?;

        let request_json = json!(
            {
                "root": root.to_string(),
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            proof.1.1[0].to_string(),
                            proof.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof.2.0.to_string(),
                        proof.2.1.to_string(),
                    ]
                ],
            }
        );

        test_success(&request_json).await;

        let request_json = json!(
            {
                "root": root.to_string(),
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            proof.1.1[0].to_string(),
                            proof.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ]
                ],
            }
        );

        test_invalid(&request_json).await;

        let request_json = json!(
            {
                "root": root.to_string(),
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            proof.1.1[0].to_string(),
                            proof.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof.2.0.to_string(),
                        "123456",
                    ]
                ],
            }
        );

        test_prover_error(&request_json).await;

        let request_json = json!(
            {
                "root": "0x1",
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            proof.1.1[0].to_string(),
                            proof.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof.2.0.to_string(),
                        proof.2.1.to_string(),
                    ]
                ],
            }
        );

        test_invalid_root(&request_json).await;

        let request_json = json!(
            {
                "root": root.to_string(),
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            "0",
                            "0",
                        ],
                    ],
                    [
                        "0",
                        "0",
                    ]
                ],
            }
        );

        test_decompressing_proof_error(&request_json).await;

        let request_json = json!(
            {
                "root": root.to_string(),
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            proof.1.1[0].to_string(),
                            proof.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof.2.0.to_string(),
                        proof.2.1.to_string(),
                    ]
                ],
                "maxRootAgeSeconds": 1,
            }
        );

        test_success(&request_json).await;

        let mut bytes: [u8; 8] = [0, 0, 0, 0, 0, 0, 0, 2];
        let identity_2 = Identity::from_secret(&mut bytes, None);
        let signal_hash_2 = hash_to_field(b"signal_hash_2");
        let external_nullifier_hash_2 = hash_to_field(b"external_hash_2");
        let nullifier_hash_2 = poseidon::hash2(external_nullifier_hash_2, identity_2.nullifier);
        let commitment_2 = identity_2.commitment();

        add_identity(app.clone(), &commitment_2.to_string()).await?;

        let (root_2, inclusion_proof_2) =
            pending_identity(app.clone(), &commitment_2.to_string()).await?;

        let proof_2 = generate_proof(
            &identity_2,
            &inclusion_proof_2,
            external_nullifier_hash_2,
            signal_hash_2,
        )?;

        let request_json = json!(
            {
                "root": root_2.to_string(),
                "signalHash": signal_hash_2.to_string(),
                "nullifierHash": nullifier_hash_2.to_string(),
                "externalNullifierHash": external_nullifier_hash_2.to_string(),
                "proof": [
                    [
                        proof_2.0.0.to_string(),
                        proof_2.0.1.to_string(),
                    ],
                    [
                        [
                            proof_2.1.0[0].to_string(),
                            proof_2.1.0[1].to_string(),
                        ],
                        [
                            proof_2.1.1[0].to_string(),
                            proof_2.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof_2.2.0.to_string(),
                        proof_2.2.1.to_string(),
                    ]
                ],
                "maxRootAgeSeconds": 1,
            }
        );

        test_success(&request_json).await;

        sleep(Duration::from_secs(2)).await;

        let request_json = json!(
            {
                "root": root_2.to_string(),
                "signalHash": signal_hash_2.to_string(),
                "nullifierHash": nullifier_hash_2.to_string(),
                "externalNullifierHash": external_nullifier_hash_2.to_string(),
                "proof": [
                    [
                        proof_2.0.0.to_string(),
                        proof_2.0.1.to_string(),
                    ],
                    [
                        [
                            proof_2.1.0[0].to_string(),
                            proof_2.1.0[1].to_string(),
                        ],
                        [
                            proof_2.1.1[0].to_string(),
                            proof_2.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof_2.2.0.to_string(),
                        proof_2.2.1.to_string(),
                    ]
                ],
                "maxRootAgeSeconds": 1,
            }
        );

        test_success(&request_json).await;

        let request_json = json!(
            {
                "root": root.to_string(),
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            proof.1.1[0].to_string(),
                            proof.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof.2.0.to_string(),
                        proof.2.1.to_string(),
                    ]
                ],
                "maxRootAgeSeconds": 1,
            }
        );

        test_root_too_old(&request_json).await;

        process_root(app.clone(), &root_2).await?;

        sleep(Duration::from_secs(2)).await;

        let request_json = json!(
            {
                "root": root_2.to_string(),
                "signalHash": signal_hash_2.to_string(),
                "nullifierHash": nullifier_hash_2.to_string(),
                "externalNullifierHash": external_nullifier_hash_2.to_string(),
                "proof": [
                    [
                        proof_2.0.0.to_string(),
                        proof_2.0.1.to_string(),
                    ],
                    [
                        [
                            proof_2.1.0[0].to_string(),
                            proof_2.1.0[1].to_string(),
                        ],
                        [
                            proof_2.1.1[0].to_string(),
                            proof_2.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof_2.2.0.to_string(),
                        proof_2.2.1.to_string(),
                    ]
                ],
                "maxRootAgeSeconds": 1,
            }
        );

        test_success(&request_json).await;

        let request_json = json!(
            {
                "root": root.to_string(),
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            proof.1.1[0].to_string(),
                            proof.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof.2.0.to_string(),
                        proof.2.1.to_string(),
                    ]
                ],
                "maxRootAgeSeconds": 1,
            }
        );

        test_root_too_old(&request_json).await;

        mine_root(app.clone(), &root_2).await?;

        sleep(Duration::from_secs(2)).await;

        let request_json = json!(
            {
                "root": root_2.to_string(),
                "signalHash": signal_hash_2.to_string(),
                "nullifierHash": nullifier_hash_2.to_string(),
                "externalNullifierHash": external_nullifier_hash_2.to_string(),
                "proof": [
                    [
                        proof_2.0.0.to_string(),
                        proof_2.0.1.to_string(),
                    ],
                    [
                        [
                            proof_2.1.0[0].to_string(),
                            proof_2.1.0[1].to_string(),
                        ],
                        [
                            proof_2.1.1[0].to_string(),
                            proof_2.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof_2.2.0.to_string(),
                        proof_2.2.1.to_string(),
                    ]
                ],
                "maxRootAgeSeconds": 1,
            }
        );

        test_success(&request_json).await;

        let request_json = json!(
            {
                "root": root.to_string(),
                "signalHash": signal_hash.to_string(),
                "nullifierHash": nullifier_hash.to_string(),
                "externalNullifierHash": external_nullifier_hash.to_string(),
                "proof": [
                    [
                        proof.0.0.to_string(),
                        proof.0.1.to_string(),
                    ],
                    [
                        [
                            proof.1.0[0].to_string(),
                            proof.1.0[1].to_string(),
                        ],
                        [
                            proof.1.1[0].to_string(),
                            proof.1.1[1].to_string(),
                        ],
                    ],
                    [
                        proof.2.0.to_string(),
                        proof.2.1.to_string(),
                    ]
                ],
                "maxRootAgeSeconds": 1,
            }
        );

        test_root_too_old(&request_json).await;

        Ok(())
    }
}
