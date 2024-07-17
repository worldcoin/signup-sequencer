mod common;

use common::prelude::*;

use crate::common::{chain, docker_compose};

const OFFCHAIN_MODE: bool = true;

#[tokio::test]
async fn offchain_insert_3k() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting e2e test");

    let docker_compose =
        docker_compose::setup("./../docker-compose-offchain", OFFCHAIN_MODE).await?;
    let chain = chain::create_chain(docker_compose.get_chain_addr()).await?;

    let uri = format!("http://{}", docker_compose.get_local_addr());
    let client = Client::new();

    let identities = generate_test_commitments(3000);

    for commitment in identities.iter() {
        insert_identity_with_retries(&client, &uri, commitment, 10, 3.0).await?;
    }

    for commitment in identities.iter() {
        mined_inclusion_proof_with_retries(
            &client,
            &uri,
            &chain,
            commitment,
            120,
            10.0,
            OFFCHAIN_MODE,
        )
        .await?;
    }

    Ok(())
}
