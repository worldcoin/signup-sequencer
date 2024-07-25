mod common;

use common::prelude::*;

use crate::common::{chain, docker_compose};

#[tokio::test]
async fn insert_restart_insert() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting e2e test");

    let docker_compose = docker_compose::setup("./../docker-compose").await?;
    let chain = chain::create_chain(docker_compose.get_chain_addr()).await?;

    let uri = format!("http://{}", docker_compose.get_local_addr());
    let client = Client::new();

    let identities = generate_test_commitments(10);

    for commitment in identities.iter() {
        insert_identity_with_retries(&client, &uri, commitment, 10, 3.0).await?;
    }

    for commitment in identities.iter() {
        mined_inclusion_proof_with_retries(&client, &uri, &chain, commitment, 60, 10.0).await?;
    }

    docker_compose.restart_sequencer().await?;

    let identities = generate_test_commitments(10);

    for commitment in identities.iter() {
        insert_identity_with_retries(&client, &uri, commitment, 10, 3.0).await?;
    }

    for commitment in identities.iter() {
        mined_inclusion_proof_with_retries(&client, &uri, &chain, commitment, 60, 10.0).await?;
    }

    Ok(())
}
