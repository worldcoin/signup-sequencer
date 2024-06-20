mod common;

use common::prelude::*;

use crate::common::docker_compose;

#[tokio::test]
async fn insert_restart_insert() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting e2e test");

    let docker_compose = docker_compose::setup("./../docker-compose").await?;

    let uri = format!("http://{}", docker_compose.get_local_addr());
    let client = Client::new();

    let identities = generate_test_commitments(10);

    for commitment in identities.iter() {
        _ = insert_identity_with_retries(&client, &uri, commitment, 10, 3.0).await?;
    }

    for commitment in identities.iter() {
        _ = mined_inclusion_proof_with_retries(&client, &uri, commitment, 60, 10.0).await?;
    }

    _ = docker_compose.restart_sequencer().await?;

    let identities = generate_test_commitments(10);

    for commitment in identities.iter() {
        _ = insert_identity_with_retries(&client, &uri, commitment, 10, 3.0).await?;
    }

    for commitment in identities.iter() {
        _ = mined_inclusion_proof_with_retries(&client, &uri, commitment, 60, 10.0).await?;
    }

    Ok(())
}
