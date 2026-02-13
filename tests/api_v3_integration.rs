mod common;

use common::prelude::*;

const IDLE_TIME: u64 = 15;

#[tokio::test]
async fn api_v3_integration_onchain() -> anyhow::Result<()> {
    api_v3_integration(false).await
}

#[tokio::test]
async fn api_v3_integration_offchain() -> anyhow::Result<()> {
    api_v3_integration(true).await
}

/// Comprehensive test for API v3 endpoints, especially inclusion proof types
async fn api_v3_integration(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting API v3 integration test");

    let insertion_batch_size: usize = 3;
    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Cli::default();
    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) = spawn_deps(
        initial_root,
        &[insertion_batch_size],
        &[],
        DEFAULT_TREE_DEPTH as u8,
        &docker,
    )
    .await?;

    let prover_mock = &insertion_prover_map[&insertion_batch_size];
    let db_socket_addr = db_container.address();
    let db_url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    let temp_dir = tempfile::tempdir()?;
    info!(
        "temp dir created at: {:?}",
        temp_dir.path().join("testfile")
    );

    let config = TestConfigBuilder::new()
        .db_url(&db_url)
        .oz_api_url(&micro_oz.endpoint())
        .oz_address(micro_oz.address())
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(prover_mock)
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");

    let uri = format!("http://{}", local_addr);
    let client = Client::new();

    // Generate test identities
    let test_identities = generate_test_identities(insertion_batch_size * 2);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    info!("Testing API v3 identity insertion");

    // Test 1: Insert identities via v3 API
    for i in 0..insertion_batch_size {
        let commitment = format!("0x{}", test_identities[i]);
        let response = client
            .post(format!("{}/v3/identities/{}", uri, commitment))
            .send()
            .await?;

        assert_eq!(
            response.status(),
            StatusCode::ACCEPTED,
            "Failed to insert identity {}",
            i
        );

        // Update reference tree
        ref_tree.set(i, identities_ref[i]);
    }

    info!("Testing duplicate insertion detection");

    // Test 2: Try to insert duplicate - should get conflict
    let duplicate_commitment = format!("0x{}", test_identities[0]);
    let response = client
        .post(format!("{}/v3/identities/{}", uri, duplicate_commitment))
        .send()
        .await?;

    assert!(
        response.status() == StatusCode::CONFLICT,
        "Expected CONFLICT for duplicate identity, got {}",
        response.status()
    );

    info!("Waiting for batch to be processed");
    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    info!("Testing inclusion proof endpoints with different types");

    // Test 3: Get inclusion proof - processed type
    for i in 0..insertion_batch_size {
        let commitment = format!("0x{}", test_identities[i]);
        let response = client
            .get(format!(
                "{}/v3/identities/{}/inclusion-proof/processed",
                uri, commitment
            ))
            .send()
            .await?;

        assert!(
            response.status().is_success(),
            "Failed to get processed inclusion proof for identity {}: {}",
            i,
            response.status()
        );

        let body: serde_json::Value = response.json().await?;
        assert!(body.get("root").is_some(), "Missing root in response");
        assert!(body.get("proof").is_some(), "Missing proof in response");
    }

    // Test 4: Get inclusion proof - mined type
    for i in 0..insertion_batch_size {
        let commitment = format!("0x{}", test_identities[i]);
        let response = client
            .get(format!(
                "{}/v3/identities/{}/inclusion-proof/mined",
                uri, commitment
            ))
            .send()
            .await?;

        assert!(
            response.status().is_success(),
            "Failed to get mined inclusion proof for identity {}: {}",
            i,
            response.status()
        );

        let body: serde_json::Value = response.json().await?;
        assert!(body.get("root").is_some(), "Missing root in response");
        assert!(body.get("proof").is_some(), "Missing proof in response");
    }

    // Test 5: Get inclusion proof - bridged type (should work for mined identities)
    for i in 0..insertion_batch_size {
        let commitment = format!("0x{}", test_identities[i]);
        let response = client
            .get(format!(
                "{}/v3/identities/{}/inclusion-proof/bridged",
                uri, commitment
            ))
            .send()
            .await?;

        assert!(
            response.status().is_success(),
            "Failed to get bridged inclusion proof for identity {}: {}",
            i,
            response.status()
        );

        let body: serde_json::Value = response.json().await?;
        assert!(body.get("root").is_some(), "Missing root in response");
        assert!(body.get("proof").is_some(), "Missing proof in response");
    }

    info!("Testing inclusion proof for non-existent identity");

    // Test 6: Try to get inclusion proof for non-existent identity
    let non_existent = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
    let response = client
        .get(format!(
            "{}/v3/identities/{}/inclusion-proof/processed",
            uri, non_existent
        ))
        .send()
        .await?;

    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "Expected NOT_FOUND for non-existent identity"
    );

    info!("Testing invalid inclusion proof type");

    // Test 7: Try invalid inclusion proof type
    let commitment = format!("0x{}", test_identities[0]);
    let response = client
        .get(format!(
            "{}/v3/identities/{}/inclusion-proof/invalid_type",
            uri, commitment
        ))
        .send()
        .await?;

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Expected BAD_REQUEST for invalid inclusion proof type"
    );

    info!("Testing identity deletion via v3 API");

    // Test 8: Delete an identity via v3 API
    let commitment_to_delete = format!("0x{}", test_identities[0]);
    let response = client
        .delete(format!("{}/v3/identities/{}", uri, commitment_to_delete))
        .send()
        .await?;

    assert_eq!(
        response.status(),
        StatusCode::ACCEPTED,
        "Failed to delete identity"
    );

    info!("Testing invalid commitment format");

    // Test 9: Invalid commitment format
    let invalid_commitment = "not_a_hex_value";
    let response = client
        .post(format!("{}/v3/identities/{}", uri, invalid_commitment))
        .send()
        .await?;

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Expected BAD_REQUEST for invalid commitment format"
    );

    info!("Testing unreduced commitment");

    // Test 12: Unreduced commitment (too large value)
    let unreduced_commitment = "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";
    let response = client
        .post(format!("{}/v3/identities/{}", uri, unreduced_commitment))
        .send()
        .await?;

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "Expected BAD_REQUEST for unreduced commitment"
    );

    info!("API v3 integration test completed successfully");

    // Cleanup
    shutdown.shutdown();
    app_handle.await?;
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}

#[tokio::test]
async fn api_v3_inclusion_proof_transitions_onchain() -> anyhow::Result<()> {
    api_v3_inclusion_proof_transitions(false).await
}

#[tokio::test]
async fn api_v3_inclusion_proof_transitions_offchain() -> anyhow::Result<()> {
    api_v3_inclusion_proof_transitions(true).await
}

/// Test that inclusion proofs work correctly as identities transition through states
async fn api_v3_inclusion_proof_transitions(offchain_mode_enabled: bool) -> anyhow::Result<()> {
    init_tracing_subscriber();
    info!("Starting API v3 inclusion proof transitions test");

    let insertion_batch_size: usize = 3;
    let mut ref_tree = PoseidonTree::new(DEFAULT_TREE_DEPTH, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let docker = Cli::default();
    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) = spawn_deps(
        initial_root,
        &[insertion_batch_size],
        &[],
        DEFAULT_TREE_DEPTH as u8,
        &docker,
    )
    .await?;

    let prover_mock = &insertion_prover_map[&insertion_batch_size];
    let db_socket_addr = db_container.address();
    let db_url = format!("postgres://postgres:postgres@{db_socket_addr}/database");

    let temp_dir = tempfile::tempdir()?;

    let config = TestConfigBuilder::new()
        .db_url(&db_url)
        .oz_api_url(&micro_oz.endpoint())
        .oz_address(micro_oz.address())
        .identity_manager_address(mock_chain.identity_manager.address())
        .primary_network_provider(mock_chain.anvil.endpoint())
        .cache_file(temp_dir.path().join("testfile").to_str().unwrap())
        .add_prover(prover_mock)
        .offchain_mode(offchain_mode_enabled)
        .build()?;

    let (_, app_handle, local_addr, shutdown) =
        spawn_app(config).await.expect("Failed to spawn app.");

    let uri = format!("http://{}", local_addr);
    let client = Client::new();

    let test_identities = generate_test_identities(insertion_batch_size);
    let identities_ref: Vec<Field> = test_identities
        .iter()
        .map(|i| Hash::from_str_radix(i, 16).unwrap())
        .collect();

    info!("Inserting identity");

    // Insert single identity
    let commitment = format!("0x{}", test_identities[0]);
    let response = client
        .post(format!("{}/v3/identities/{}", uri, commitment))
        .send()
        .await?;

    assert_eq!(response.status(), StatusCode::ACCEPTED);
    ref_tree.set(0, identities_ref[0]);

    info!("Testing processed proof shortly after insertion");

    // Give a moment for the identity to be recorded
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Should be able to get processed proof after insertion
    let response = client
        .get(format!(
            "{}/v3/identities/{}/inclusion-proof/processed",
            uri, commitment
        ))
        .send()
        .await?;

    assert!(
        response.status().is_success(),
        "Should be able to get processed proof after insertion"
    );

    info!("Waiting for batch to be mined");
    tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

    info!("Testing all proof types after mining");

    // After mining, all proof types should work
    for proof_type in ["processed", "mined", "bridged"] {
        let response = client
            .get(format!(
                "{}/v3/identities/{}/inclusion-proof/{}",
                uri, commitment, proof_type
            ))
            .send()
            .await?;

        assert!(
            response.status().is_success(),
            "Failed to get {} proof after mining",
            proof_type
        );

        let body: serde_json::Value = response.json().await?;

        // Verify response structure
        assert!(
            body.get("root").is_some(),
            "Missing root in {} proof",
            proof_type
        );
        assert!(
            body.get("proof").is_some(),
            "Missing proof in {} proof",
            proof_type
        );

        // Verify proof is an array
        let proof = body.get("proof").unwrap();
        assert!(
            proof.is_array(),
            "Proof should be an array for {}",
            proof_type
        );
    }

    info!("API v3 inclusion proof transitions test completed successfully");

    // Cleanup
    shutdown.shutdown();
    app_handle.await?;
    for (_, prover) in insertion_prover_map.into_iter() {
        prover.stop();
    }

    Ok(())
}
