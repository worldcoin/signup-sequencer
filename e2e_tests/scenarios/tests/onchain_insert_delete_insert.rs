mod common;

use common::prelude::*;

use crate::common::{chain, docker_compose};

const OFFCHAIN_MODE: bool = false;

#[tokio::test]
async fn onchain_insert_delete_insert() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting e2e test");

    let docker_compose = docker_compose::setup("./../docker-compose", OFFCHAIN_MODE).await?;
    let chain = chain::create_chain(docker_compose.get_chain_addr()).await?;

    let uri = format!("http://{}", docker_compose.get_local_addr());
    let client = Client::new();

    let identities = generate_test_commitments(10);

    for commitment in identities.iter() {
        insert_identity_with_retries(&client, &uri, commitment, 10, 3.0).await?;
    }

    for commitment in identities.iter() {
        mined_inclusion_proof_with_retries(
            &client,
            &uri,
            &chain,
            commitment,
            60,
            10.0,
            OFFCHAIN_MODE,
        )
        .await?;
    }

    let first_commitment = identities.first().unwrap();

    delete_identity_with_retries(&client, &uri, first_commitment, 10, 3.0).await?;
    bad_request_inclusion_proof_with_retries(&client, &uri, first_commitment, 60, 10.0).await?;

    let new_identities = generate_test_commitments(10);

    for commitment in new_identities.iter() {
        insert_identity_with_retries(&client, &uri, commitment, 10, 3.0).await?;
    }

    for commitment in new_identities.iter() {
        mined_inclusion_proof_with_retries(
            &client,
            &uri,
            &chain,
            commitment,
            60,
            10.0,
            OFFCHAIN_MODE,
        )
        .await?;
    }

    Ok(())
}
