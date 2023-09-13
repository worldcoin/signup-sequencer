mod common;

use std::str::FromStr;

use common::prelude::*;
use hyper::Uri;

use crate::common::{test_add_batch_size, test_remove_batch_size};

const SUPPORTED_DEPTH: usize = 20;
const IDLE_TIME: u64 = 7;

#[tokio::test]
#[serial_test::serial]
async fn dynamic_batch_sizes() -> anyhow::Result<()> {
    // Initialize logging for the test.
    init_tracing_subscriber();
    info!("Starting integration test");

    let batch_size: usize = 3;
    let second_batch_size: usize = 2;
    #[allow(clippy::cast_possible_truncation)]
    let tree_depth: u8 = SUPPORTED_DEPTH as u8;

    let mut ref_tree = PoseidonTree::new(SUPPORTED_DEPTH + 1, ruint::Uint::ZERO);
    let initial_root: U256 = ref_tree.root().into();

    let (mock_chain, db_container, insertion_prover_map, _, micro_oz) =
        spawn_deps(initial_root, &[batch_size], &vec![], tree_depth).await?;

    if let Some(insertion_prover_map) = insertion_prover_map {
        let prover_mock = &insertion_prover_map[&batch_size];

        let port = db_container.port();
        let db_url = format!("postgres://postgres:postgres@localhost:{port}/database");

        // We initially spawn the service with a single prover for batch size 3.

        let mut options = Options::try_parse_from([
            "signup-sequencer",
            "--identity-manager-address",
            "0x0000000000000000000000000000000000000000", // placeholder, updated below
            "--database",
            &db_url,
            "--database-max-connections",
            "1",
            "--tree-depth",
            &format!("{tree_depth}"),
            "--prover-urls",
            &prover_mock.arg_string(),
            "--batch-timeout-seconds",
            "3",
            "--dense-tree-prefix-depth",
            "10",
            "--tree-gc-threshold",
            "1",
            "--oz-api-key",
            "",
            "--oz-api-secret",
            "",
            "--oz-api-url",
            &micro_oz.endpoint(),
            "--oz-address",
            &format!("{:?}", micro_oz.address()),
        ])
        .context("Failed to create options")?;

        options.server.server = Url::parse("http://127.0.0.1:0/").expect("Failed to parse URL");

        options.app.contracts.identity_manager_address = mock_chain.identity_manager.address();
        options.app.ethereum.ethereum_provider =
            Url::parse(&mock_chain.anvil.endpoint()).expect("Failed to parse Anvil url");

        let (app, local_addr) = spawn_app(options.clone())
            .await
            .expect("Failed to spawn app.");

        let test_identities = generate_test_identities(batch_size * 5);
        let identities_ref: Vec<Field> = test_identities
            .iter()
            .map(|i| Hash::from_str_radix(i, 16).unwrap())
            .collect();

        let uri = "http://".to_owned() + &local_addr.to_string();
        let client = Client::new();

        // Insert enough identities to trigger an batch to be sent to the blockchain
        // based on the current batch size of 3.
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 0).await;
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 1).await;
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 2).await;

        // wait at leat 5 seconds before checking proof
        tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

        // Check that we can get their inclusion proofs back.
        test_inclusion_proof(
            &uri,
            &client,
            0,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[0], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            1,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[1], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            2,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[2], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;

        // Add a new prover for batch sizes of two.
        let second_prover = spawn_mock_insertion_prover(second_batch_size).await?;

        test_add_batch_size(
            &uri,
            second_prover.url(),
            second_batch_size as u64,
            second_prover.prover_type(),
            &client,
        )
        .await
        .expect("Failed to add batch size.");

        // Query for the available provers.
        let batch_sizes_uri: Uri = Uri::from_str(&format!("{}/listBatchSizes", uri.as_str()))
            .expect("Unable to parse URI.");
        let mut batch_sizes = client
            .get(batch_sizes_uri)
            .await
            .expect("Failed to execute get request");
        let batch_sizes_bytes = hyper::body::to_bytes(batch_sizes.body_mut())
            .await
            .expect("Failed to get response bytes");
        let batch_size_str = String::from_utf8(batch_sizes_bytes.into_iter().collect())
            .expect("Failed to decode response");
        let batch_size_json = serde_json::from_str::<serde_json::Value>(&batch_size_str)
            .expect("JSON wasn't decoded");
        assert_eq!(
            batch_size_json,
            json!([
                {
                    "url": second_prover.url() + "/",
                    "timeout_s": 3,
                    "batch_size": second_batch_size,
                    "prover_type": "insertion",
                },
                {
                    "url": prover_mock.url() + "/",
                    "timeout_s": 30,
                    "batch_size": batch_size,
                    "prover_type": "insertion",

                }
            ])
        );

        // Insert enough identities to trigger the lower batch size.
        prover_mock.set_availability(false).await;
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 3).await;
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 4).await;

        // Force a timeout by resetting the tokio runtime's timer.
        tokio::time::pause();
        tokio::time::resume();

        tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;
        // Check that we can also get these inclusion proofs back.
        test_inclusion_proof(
            &uri,
            &client,
            3,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[3], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;
        test_inclusion_proof(
            &uri,
            &client,
            4,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[4], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;

        // Now if we remove the original prover, things should still work.
        test_remove_batch_size(
            &uri,
            batch_size as u64,
            &client,
            prover_mock.prover_type(),
            false,
        )
        .await?;

        // We should be able to insert less than a full batch successfully.
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 5).await;
        tokio::time::pause();
        tokio::time::resume();

        tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

        test_inclusion_proof(
            &uri,
            &client,
            5,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[5], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;

        // We should be unable to remove _all_ of the provers, however.
        test_remove_batch_size(
            &uri,
            second_batch_size as u64,
            &client,
            second_prover.prover_type(),
            true,
        )
        .await?;

        // So we should still be able to run a batch.
        test_insert_identity(&uri, &client, &mut ref_tree, &identities_ref, 6).await;
        tokio::time::pause();
        tokio::time::resume();

        tokio::time::sleep(Duration::from_secs(IDLE_TIME)).await;

        test_inclusion_proof(
            &uri,
            &client,
            6,
            &ref_tree,
            &Hash::from_str_radix(&test_identities[6], 16)
                .expect("Failed to parse Hash from test leaf 0"),
            false,
        )
        .await;

        // Shutdown the app properly for the final time
        shutdown();
        app.await.unwrap();
        for (_, prover) in insertion_prover_map.into_iter() {
            prover.stop();
        }
        reset_shutdown();

        Ok(())
    } else {
        Err(anyhow::anyhow!("No insertion prover map found"))
    }
}
