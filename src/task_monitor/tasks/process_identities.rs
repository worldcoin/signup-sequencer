use std::sync::Arc;

use anyhow::Context;
use chrono::{DateTime, Utc};
use ethers::types::U256;
use ruint::Uint;
use semaphore::merkle_tree::Proof;
use semaphore::poseidon_tree::{Branch, PoseidonHash};
use tokio::sync::{mpsc, Notify};
use tokio::{select, time};
use tracing::instrument;

use crate::app::App;
use crate::contracts::IdentityManager;
use crate::database::types::{BatchEntry, BatchType};
use crate::database::DatabaseExt as _;
use crate::ethereum::write::TransactionId;
use crate::identity_tree::{
    AppliedTreeUpdate, Hash, Intermediate, Latest, TreeVersion, TreeVersionReadOps,
    TreeWithNextVersion,
};
use crate::prover::identity::Identity;
use crate::prover::Prover;
use crate::task_monitor::TaskMonitor;
use crate::utils::index_packing::pack_indices;

/// The number of seconds either side of the timer tick to treat as enough to
/// trigger a forced batch insertion.
const DEBOUNCE_THRESHOLD_SECS: i64 = 1;

pub async fn process_identities(
    app: Arc<App>,
    monitored_txs_sender: Arc<mpsc::Sender<TransactionId>>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    tracing::info!("Awaiting for a clean slate");
    app.identity_manager.await_clean_slate().await?;

    tracing::info!("Starting identity processor.");

    // We start a timer and force it to perform one initial tick to avoid an
    // immediate trigger.
    let mut timer = time::interval(app.config.app.batch_insertion_timeout);
    timer.tick().await;

    // When both futures are woken at once, the choice is made
    // non-deterministically. This could, in the worst case, result in users waiting
    // for twice `timeout_secs` for their insertion to be processed.
    //
    // To ensure that this does not happen we track the last time a batch was
    // inserted. If we have an incomplete batch but are within a small delta of the
    // tick happening anyway in the wake branch, we insert the current
    // (possibly-incomplete) batch anyway.
    let mut last_batch_time: DateTime<Utc> = app
        .database
        .get_latest_insertion_timestamp()
        .await?
        .unwrap_or(Utc::now());

    loop {
        // We wait either for a timer tick or a full batch
        select! {
            _ = timer.tick() => {
                tracing::info!("Identity batch insertion woken due to timeout");
            }

            () = wake_up_notify.notified() => {
                tracing::trace!("Identity batch insertion woken due to request");
            },
        }

        let current_root = app.tree_state()?.latest_tree().get_root();
        let next_batch = app.database.get_next_batch(&current_root).await?;
        let Some(next_batch) = next_batch else {
            if !app.database.is_root_in_batch_chain(&current_root).await? {
                // todo(piotrh)
                panic!(
                    "Current root of latest tree cannot be find in database in batches chain. It \
                     should never happen."
                );
            }
            println!("[zzz] skipping batch");
            // todo(piotrh)
            continue;
        };

        let tx = app
            .database
            .get_transaction_for_batch(&next_batch.next_root)
            .await?;
        if tx.is_some() {
            println!("[zzz] transaction already created");
            // todo(piotrh): should be run here?
            update_tree(app.tree_state()?.latest_tree(), &next_batch)?;

            // todo(pioth): check
            continue;
        }

        println!("[zzz] processing batch");

        // let batch_size = match next_batch.batch_type {
        //     BatchType::Insertion =>
        // app.identity_manager.max_insertion_batch_size().await,
        //     BatchType::Deletion =>
        // app.identity_manager.max_deletion_batch_size().await, };
        //
        // let updates = app
        //     .tree_state()?
        //     .batching_tree()
        //     .peek_next_updates(batch_size);

        // let current_time = Utc::now();
        // let batch_insertion_timeout =
        //     chrono::Duration::from_std(app.config.app.batch_insertion_timeout)?;
        //
        // let timeout_batch_time = last_batch_time
        //     + batch_insertion_timeout
        //     + chrono::Duration::seconds(DEBOUNCE_THRESHOLD_SECS);
        //
        // let can_skip_batch = current_time < timeout_batch_time;

        // if next_batch.commitments.0.len() < batch_size && can_skip_batch {
        //     tracing::trace!(
        //         num_updates = next_batch.commitments.0.len(),
        //         batch_size,
        //         ?last_batch_time,
        //         "Pending identities is less than batch size, skipping batch",
        //     );
        //
        //     continue;
        // }

        let should_continue = match next_batch.batch_type {
            BatchType::Insertion => {
                process_insertion_batch(&app, &next_batch, &last_batch_time, &monitored_txs_sender)
                    .await?
            }
            BatchType::Deletion => {
                process_deletion_batch(&app, &next_batch, &last_batch_time, &monitored_txs_sender)
                    .await?
            }
        };

        if should_continue {
            continue;
        }

        timer.reset();
        last_batch_time = Utc::now();
        app.database
            .update_latest_insertion_timestamp(last_batch_time)
            .await?;

        // We want to check if there's a full batch available immediately
        wake_up_notify.notify_one();
    }
}

// async fn commit_identities(
//     identity_manager: &IdentityManager,
//     batching_tree: &TreeVersion<Intermediate>,
//     monitored_txs_sender: &mpsc::Sender<TransactionId>,
//     updates: &[AppliedTreeUpdate],
// ) -> anyhow::Result<TransactionId> {
//     // If the update is an insertion
//     let tx_id = if updates
//         .first()
//         .context("Updates should be > 1")?
//         .update
//         .element
//         != Hash::ZERO
//     {
//         let prover = identity_manager
//             .get_suitable_insertion_prover(updates.len())
//             .await?;
//
//         tracing::info!(
//             num_updates = updates.len(),
//             batch_size = prover.batch_size(),
//             "Insertion batch",
//         );
//
//         insert_identities(identity_manager, batching_tree, updates,
// &prover).await?     } else {
//         let prover = identity_manager
//             .get_suitable_deletion_prover(updates.len())
//             .await?;
//
//         tracing::info!(
//             num_updates = updates.len(),
//             batch_size = prover.batch_size(),
//             "Deletion batch"
//         );
//
//         delete_identities(identity_manager, batching_tree, updates,
// &prover).await?     };
//
//     // todo(piotrh): remove unwrap
//     let res = tx_id.clone().unwrap();
//
//     if let Some(tx_id) = tx_id {
//         monitored_txs_sender.send(tx_id).await?;
//     }
//
//     // todo(piotrh): remove unwrap
//     Ok(res)
// }

async fn process_insertion_batch(
    app: &Arc<App>,
    next_batch: &BatchEntry,
    last_batch_time: &DateTime<Utc>,
    monitored_txs_sender: &Arc<mpsc::Sender<TransactionId>>,
) -> anyhow::Result<bool> {
    println!("[zzz] process insertion batch - 1");
    let latest_tree = app.tree_state()?.latest_tree();

    let (updates, updated_tree) = latest_tree.append_many_as_derived(&next_batch.commitments.0);
    let next_root: Hash = updated_tree.root();
    if next_root != next_batch.next_root {
        // todo(piotrh): implement
        panic!("[zzz]");
    }

    println!("[zzz] process insertion batch - 1.2");
    let batch_size = app.identity_manager.max_insertion_batch_size().await;
    let current_time = Utc::now();
    let batch_insertion_timeout =
        chrono::Duration::from_std(app.config.app.batch_insertion_timeout)?;

    let timeout_batch_time = *last_batch_time
        + batch_insertion_timeout
        + chrono::Duration::seconds(DEBOUNCE_THRESHOLD_SECS);

    let can_skip_batch = current_time < timeout_batch_time;

    println!("[zzz] process insertion batch - 1.3");
    // if next_batch.commitments.0.len() < batch_size && can_skip_batch {
    //     tracing::trace!(
    //         num_updates = next_batch.commitments.0.len(),
    //         batch_size,
    //         ?last_batch_time,
    //         "Pending identities is less than batch size, skipping batch",
    //     );
    //
    //     println!("[zzz] process insertion batch - 1.4");
    //     return Ok(true);
    // }

    println!("[zzz] process insertion batch - 2");
    let prover = app
        .identity_manager
        .get_suitable_insertion_prover(updates.len())
        .await?;

    tracing::info!(
        num_updates = updates.len(),
        batch_size = prover.batch_size(),
        "Insertion batch",
    );

    assert_updates_are_consecutive(&next_batch);

    // let start_index = updates[0].update.leaf_index;
    let start_index = next_batch
        .leaf_indexes
        .0
        .first()
        .expect("checked earlier")
        .clone(); // todo(piotrh): error message
                  // let pre_root: U256 = batching_tree.get_root().into();
    let mut commitments: Vec<U256> = next_batch
        .commitments
        .0
        .iter()
        .map(|v| (*v).into())
        .collect();
    println!("[zzz] process insertion batch - 3");
    // let mut commitments: Vec<U256> = updates
    //     .iter()
    //     .map(|update| update.update.element.into())
    //     .collect();
    //
    // let latest_tree_from_updates = updates
    //     .last()
    //     .expect("Updates is non empty.")
    //     .result
    //     .clone();
    //
    // // Next get merkle proofs for each update - note the proofs are acquired from
    // // intermediate versions of the tree
    let mut merkle_proofs: Vec<_> = updates.iter().map(|(_, proof, _)| proof.clone()).collect();
    // let mut merkle_proofs: Vec<_> = updates
    //     .iter()
    //     .map(|update| update.result.proof(update.update.leaf_index))
    //     .collect();

    // Grab some variables for sizes to make querying easier.
    let commitment_count = commitments.len();

    // If these aren't equal then something has gone terribly wrong and is a
    // programmer bug, so we abort.
    assert_eq!(
        commitment_count,
        updates.len(),
        "Number of identities does not match the number of merkle proofs."
    );

    let batch_size = prover.batch_size();

    // The verifier and prover can only work with a given batch size, so we need to
    // ensure that our batches match that size. We do this by padding with
    // subsequent zero identities and their associated merkle proofs if the batch is
    // too small.
    if commitment_count != batch_size {
        let start_index = next_batch.leaf_indexes.0.last()
            .expect("Already confirmed to exist.") // todo(piotrh), check message
            + 1;
        let padding = batch_size - commitment_count;
        commitments.append(&mut vec![U256::zero(); padding]);

        for i in start_index..(start_index + padding) {
            let proof = updated_tree.proof(i);
            merkle_proofs.push(proof);
        }
    }

    println!("[zzz] process insertion batch - 4");
    assert_eq!(
        commitments.len(),
        batch_size,
        "Mismatch between commitments and batch size."
    );
    assert_eq!(
        merkle_proofs.len(),
        batch_size,
        "Mismatch between merkle proofs and batch size."
    );

    // With the updates applied we can grab the value of the tree's new root and
    // build our identities for sending to the identity manager.
    let post_root: U256 = next_root.into(); // todo(piotrh) check if batch was properly executed
    let prev_root: U256 = next_batch.prev_root.unwrap().into(); // todo(piotrh) unwrap remove
    let next_root: U256 = next_batch.next_root.into(); // todo(piotrh) unwrap remove
                                                       // let post_root: U256 = latest_tree.get_root().into(); // todo(piotrh) check if
                                                       // batch was properly executed
    let identity_commitments = zip_commitments_and_proofs(commitments, merkle_proofs);

    app.identity_manager
        .validate_merkle_proofs(&identity_commitments)?;

    // We prepare the proof before reserving a slot in the pending identities
    let proof = IdentityManager::prepare_insertion_proof(
        &prover,
        start_index,
        prev_root,
        &identity_commitments,
        next_root,
    )
    .await?;

    println!("[zzz] process insertion batch - 5");
    tracing::info!(
        start_index,
        ?prev_root,
        ?next_root,
        "Submitting insertion batch"
    );

    // With all the data prepared we can submit the identities to the on-chain
    // identity manager and wait for that transaction to be mined.
    let transaction_id = app
        .identity_manager
        .register_identities(
            start_index,
            prev_root,
            next_root,
            identity_commitments,
            proof,
        )
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to insert identity to contract.");
            e
        })?;

    tracing::info!(
        start_index,
        ?prev_root,
        ?next_root,
        ?transaction_id,
        "Insertion batch submitted"
    );

    tracing::info!(start_index, ?prev_root, ?next_root, "Tree updated");

    app.database
        .insert_new_transaction(&transaction_id.0, &next_batch.next_root)
        .await?;

    // todo(piotrh): check
    // do the real update finally when transaction is created
    // after transaction being saved we are save even to not do it, but let's do for
    // optimization
    latest_tree.append_many(&next_batch.commitments.0);

    monitored_txs_sender.send(transaction_id).await?;

    println!("[zzz] process insertion batch - 6");

    TaskMonitor::log_batch_size(updates.len());

    Ok(false)
}

async fn process_deletion_batch(
    app: &Arc<App>,
    next_batch: &BatchEntry,
    last_batch_time: &DateTime<Utc>,
    monitored_txs_sender: &Arc<mpsc::Sender<TransactionId>>,
) -> anyhow::Result<bool> {
    let latest_tree = app.tree_state()?.latest_tree();

    let updates = latest_tree.delete_many(&next_batch.leaf_indexes.0);
    if latest_tree.get_root() != next_batch.next_root {
        // todo(piotrh): implement
        panic!("[zzz]");
    }

    let batch_size = app.identity_manager.max_deletion_batch_size().await;
    let current_time = Utc::now();
    let batch_insertion_timeout =
        chrono::Duration::from_std(app.config.app.batch_insertion_timeout)?;

    let timeout_batch_time = *last_batch_time
        + batch_insertion_timeout
        + chrono::Duration::seconds(DEBOUNCE_THRESHOLD_SECS);

    // todo(piotrh): check
    // let can_skip_batch = current_time < timeout_batch_time;
    //
    // if next_batch.commitments.0.len() < batch_size && can_skip_batch {
    //     tracing::trace!(
    //         num_updates = next_batch.commitments.0.len(),
    //         batch_size,
    //         ?last_batch_time,
    //         "Pending identities is less than batch size, skipping batch",
    //     );
    //
    //     return Ok(true);
    // }

    let prover = app
        .identity_manager
        .get_suitable_deletion_prover(updates.len())
        .await?;

    tracing::info!(
        num_updates = updates.len(),
        batch_size = prover.batch_size(),
        "Deletion batch",
    );

    // Grab the initial conditions before the updates are applied to the tree.
    // let pre_root: U256 = batching_tree.get_root().into();

    let mut deletion_indices: Vec<_> = next_batch
        .leaf_indexes
        .0
        .iter()
        .map(|&v| v as u32)
        .collect();
    // let mut deletion_indices = updates
    //     .iter()
    //     .map(|f| f.update.leaf_index as u32)
    //     .collect::<Vec<u32>>();

    // let commitments =
    //     batching_tree.commitments_by_indices(deletion_indices.iter().map(|x| *x
    // as usize)); let mut commitments: Vec<U256> =
    // commitments.into_iter().map(U256::from).collect();
    let mut commitments: Vec<U256> = next_batch
        .commitments
        .0
        .iter()
        .map(|v| (*v).into())
        .collect();

    // let latest_tree_from_updates = updates
    //     .last()
    //     .expect("Updates is non empty.")
    //     .result
    //     .clone();

    // Next get merkle proofs for each update - note the proofs are acquired from
    // intermediate versions of the tree
    let mut merkle_proofs: Vec<_> = updates.iter().map(|(_, proof)| proof.clone()).collect();
    // let mut merkle_proofs: Vec<_> = updates
    //     .iter()
    //     .map(|update_with_tree| {
    //         update_with_tree
    //             .result
    //             .proof(update_with_tree.update.leaf_index)
    //     })
    //     .collect();

    // Grab some variables for sizes to make querying easier.
    let commitment_count = commitments.len();
    // let commitment_count = updates.len();

    // If these aren't equal then something has gone terribly wrong and is a
    // programmer bug, so we abort.
    assert_eq!(
        commitment_count,
        merkle_proofs.len(),
        "Number of identities does not match the number of merkle proofs."
    );

    let batch_size = prover.batch_size();

    // The verifier and prover can only work with a given batch size, so we need to
    // ensure that our batches match that size. We do this by padding deletion
    // indices with tree.depth() ^ 2. The deletion prover will skip the proof for
    // any deletion with an index greater than the max tree depth
    let pad_index = 2_u32.pow(latest_tree.get_depth() as u32);

    if commitment_count != batch_size {
        let padding = batch_size - commitment_count;
        commitments.extend(vec![U256::zero(); padding]);
        deletion_indices.extend(vec![pad_index; padding]);

        let zeroed_proof = Proof(vec![Branch::Left(Uint::ZERO); latest_tree.get_depth()]);

        merkle_proofs.extend(vec![zeroed_proof; padding]);
    }

    assert_eq!(
        deletion_indices.len(),
        batch_size,
        "Mismatch between deletion indices length and batch size."
    );

    // With the updates applied we can grab the value of the tree's new root and
    // build our identities for sending to the identity manager.
    let prev_root: U256 = next_batch.prev_root.unwrap().into(); // todo(piotrh) unwrap remove
    let next_root: U256 = next_batch.next_root.into(); // todo(piotrh) unwrap remove
    let post_root: U256 = latest_tree.get_root().into();
    let identity_commitments = zip_commitments_and_proofs(commitments, merkle_proofs);

    app.identity_manager
        .validate_merkle_proofs(&identity_commitments)?;

    // We prepare the proof before reserving a slot in the pending identities
    let proof = IdentityManager::prepare_deletion_proof(
        &prover,
        prev_root,
        deletion_indices.clone(),
        identity_commitments,
        next_root,
    )
    .await?;

    let packed_deletion_indices = pack_indices(&deletion_indices);

    tracing::info!(?prev_root, ?next_root, "Submitting deletion batch");

    // With all the data prepared we can submit the identities to the on-chain
    // identity manager and wait for that transaction to be mined.
    let transaction_id = app
        .identity_manager
        .delete_identities(proof, packed_deletion_indices, prev_root, next_root)
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to insert identity to contract.");
            e
        })?;

    tracing::info!(
        ?prev_root,
        ?next_root,
        ?transaction_id,
        "Deletion batch submitted"
    );

    app.database
        .insert_new_transaction(&transaction_id.0, &next_batch.next_root)
        .await?;

    monitored_txs_sender.send(transaction_id).await?;

    tracing::info!(?prev_root, ?next_root, "Tree updated");

    TaskMonitor::log_batch_size(updates.len());

    Ok(false)
}

fn update_tree(latest_tree: &TreeVersion<Latest>, next_batch: &BatchEntry) -> anyhow::Result<()> {
    if next_batch.batch_type == BatchType::Insertion {
        latest_tree.append_many(&next_batch.commitments.0);
    } else if next_batch.batch_type == BatchType::Deletion {
        latest_tree.delete_many(&next_batch.leaf_indexes.0);
    }

    if latest_tree.get_root() != next_batch.next_root {
        // todo(piotrh): implement
        panic!("[zzz]");
    }

    Ok(())
}

// #[instrument(level = "info", skip_all)]
// pub async fn insert_identities(
//     identity_manager: &IdentityManager,
//     batching_tree: &TreeVersion<Intermediate>,
//     updates: &[AppliedTreeUpdate],
//     prover: &Prover,
// ) -> anyhow::Result<Option<TransactionId>> {
//     assert_updates_are_consecutive(updates);
//
//     let start_index = updates[0].update.leaf_index;
//     let pre_root: U256 = batching_tree.get_root().into();
//     let mut commitments: Vec<U256> = updates
//         .iter()
//         .map(|update| update.update.element.into())
//         .collect();
//
//     let latest_tree_from_updates = updates
//         .last()
//         .expect("Updates is non empty.")
//         .result
//         .clone();
//
//     // Next get merkle proofs for each update - note the proofs are acquired
// from     // intermediate versions of the tree
//     let mut merkle_proofs: Vec<_> = updates
//         .iter()
//         .map(|update| update.result.proof(update.update.leaf_index))
//         .collect();
//
//     // Grab some variables for sizes to make querying easier.
//     let commitment_count = updates.len();
//
//     // If these aren't equal then something has gone terribly wrong and is a
//     // programmer bug, so we abort.
//     assert_eq!(
//         commitment_count,
//         merkle_proofs.len(),
//         "Number of identities does not match the number of merkle proofs."
//     );
//
//     let batch_size = prover.batch_size();
//
//     // The verifier and prover can only work with a given batch size, so we
// need to     // ensure that our batches match that size. We do this by padding
// with     // subsequent zero identities and their associated merkle proofs if
// the batch is     // too small.
//     if commitment_count != batch_size {
//         let start_index = updates
//             .last()
//             .expect("Already confirmed to exist.")
//             .update
//             .leaf_index
//             + 1;
//         let padding = batch_size - commitment_count;
//         commitments.append(&mut vec![U256::zero(); padding]);
//
//         for i in start_index..(start_index + padding) {
//             let proof = latest_tree_from_updates.proof(i);
//             merkle_proofs.push(proof);
//         }
//     }
//
//     assert_eq!(
//         commitments.len(),
//         batch_size,
//         "Mismatch between commitments and batch size."
//     );
//     assert_eq!(
//         merkle_proofs.len(),
//         batch_size,
//         "Mismatch between merkle proofs and batch size."
//     );
//
//     // With the updates applied we can grab the value of the tree's new root
// and     // build our identities for sending to the identity manager.
//     let post_root: U256 = latest_tree_from_updates.root().into();
//     let identity_commitments = zip_commitments_and_proofs(commitments,
// merkle_proofs);
//
//     identity_manager.validate_merkle_proofs(&identity_commitments)?;
//
//     // We prepare the proof before reserving a slot in the pending identities
//     let proof = IdentityManager::prepare_insertion_proof(
//         prover,
//         start_index,
//         pre_root,
//         &identity_commitments,
//         post_root,
//     )
//     .await?;
//
//     tracing::info!(
//         start_index,
//         ?pre_root,
//         ?post_root,
//         "Submitting insertion batch"
//     );
//
//     // With all the data prepared we can submit the identities to the
// on-chain     // identity manager and wait for that transaction to be mined.
//     let transaction_id = identity_manager
//         .register_identities(
//             start_index,
//             pre_root,
//             post_root,
//             identity_commitments,
//             proof,
//         )
//         .await
//         .map_err(|e| {
//             tracing::error!(?e, "Failed to insert identity to contract.");
//             e
//         })?;
//
//     tracing::info!(
//         start_index,
//         ?pre_root,
//         ?post_root,
//         ?transaction_id,
//         "Insertion batch submitted"
//     );
//
//     // Update the batching tree only after submitting the identities to the
// chain     batching_tree.apply_updates_up_to(post_root.into());
//
//     tracing::info!(start_index, ?pre_root, ?post_root, "Tree updated");
//
//     TaskMonitor::log_batch_size(updates.len());
//
//     Ok(Some(transaction_id))
// }

fn assert_updates_are_consecutive(next_batch: &BatchEntry) {
    for window in next_batch.leaf_indexes.0.windows(2) {
        if window[0] + 1 != window[1] {
            panic!(
                "Identities are not consecutive leaves in the tree (leaf_indexes = {:?}, \
                 commitments = {:?})",
                next_batch.leaf_indexes.0, next_batch.commitments.0
            );
        }
    }
}

// pub async fn delete_identities(
//     identity_manager: &IdentityManager,
//     batching_tree: &TreeVersion<Intermediate>,
//     updates: &[AppliedTreeUpdate],
//     prover: &Prover,
// ) -> anyhow::Result<Option<TransactionId>> {
//     // Grab the initial conditions before the updates are applied to the
// tree.     let pre_root: U256 = batching_tree.get_root().into();
//
//     let mut deletion_indices = updates
//         .iter()
//         .map(|f| f.update.leaf_index as u32)
//         .collect::<Vec<u32>>();
//
//     let commitments =
//         batching_tree.commitments_by_indices(deletion_indices.iter().map(|x|
// *x as usize));     let mut commitments: Vec<U256> =
// commitments.into_iter().map(U256::from).collect();
//
//     let latest_tree_from_updates = updates
//         .last()
//         .expect("Updates is non empty.")
//         .result
//         .clone();
//
//     // Next get merkle proofs for each update - note the proofs are acquired
// from     // intermediate versions of the tree
//     let mut merkle_proofs: Vec<_> = updates
//         .iter()
//         .map(|update_with_tree| {
//             update_with_tree
//                 .result
//                 .proof(update_with_tree.update.leaf_index)
//         })
//         .collect();
//
//     // Grab some variables for sizes to make querying easier.
//     let commitment_count = updates.len();
//
//     // If these aren't equal then something has gone terribly wrong and is a
//     // programmer bug, so we abort.
//     assert_eq!(
//         commitment_count,
//         merkle_proofs.len(),
//         "Number of identities does not match the number of merkle proofs."
//     );
//
//     let batch_size = prover.batch_size();
//
//     // The verifier and prover can only work with a given batch size, so we
// need to     // ensure that our batches match that size. We do this by padding
// deletion     // indices with tree.depth() ^ 2. The deletion prover will skip
// the proof for     // any deletion with an index greater than the max tree
// depth     let pad_index = 2_u32.pow(latest_tree_from_updates.depth() as u32);
//
//     if commitment_count != batch_size {
//         let padding = batch_size - commitment_count;
//         commitments.extend(vec![U256::zero(); padding]);
//         deletion_indices.extend(vec![pad_index; padding]);
//
//         let zeroed_proof = Proof(vec![
//             Branch::Left(Uint::ZERO);
//             latest_tree_from_updates.depth()
//         ]);
//
//         merkle_proofs.extend(vec![zeroed_proof; padding]);
//     }
//
//     assert_eq!(
//         deletion_indices.len(),
//         batch_size,
//         "Mismatch between deletion indices length and batch size."
//     );
//
//     // With the updates applied we can grab the value of the tree's new root
// and     // build our identities for sending to the identity manager.
//     let post_root: U256 = latest_tree_from_updates.root().into();
//     let identity_commitments = zip_commitments_and_proofs(commitments,
// merkle_proofs);
//
//     identity_manager.validate_merkle_proofs(&identity_commitments)?;
//
//     // We prepare the proof before reserving a slot in the pending identities
//     let proof = IdentityManager::prepare_deletion_proof(
//         prover,
//         pre_root,
//         deletion_indices.clone(),
//         identity_commitments,
//         post_root,
//     )
//     .await?;
//
//     let packed_deletion_indices = pack_indices(&deletion_indices);
//
//     tracing::info!(?pre_root, ?post_root, "Submitting deletion batch");
//
//     // With all the data prepared we can submit the identities to the
// on-chain     // identity manager and wait for that transaction to be mined.
//     let transaction_id = identity_manager
//         .delete_identities(proof, packed_deletion_indices, pre_root,
// post_root)         .await
//         .map_err(|e| {
//             tracing::error!(?e, "Failed to insert identity to contract.");
//             e
//         })?;
//
//     tracing::info!(
//         ?pre_root,
//         ?post_root,
//         ?transaction_id,
//         "Deletion batch submitted"
//     );
//
//     // Update the batching tree only after submitting the identities to the
// chain     batching_tree.apply_updates_up_to(post_root.into());
//
//     tracing::info!(?pre_root, ?post_root, "Tree updated");
//
//     TaskMonitor::log_batch_size(updates.len());
//
//     Ok(Some(transaction_id))
// }

fn zip_commitments_and_proofs(
    commitments: Vec<U256>,
    merkle_proofs: Vec<Proof<PoseidonHash>>,
) -> Vec<Identity> {
    commitments
        .iter()
        .zip(merkle_proofs)
        .map(|(id, prf)| {
            let commitment: U256 = id.into();
            let proof: Vec<U256> = prf
                .0
                .iter()
                .map(|branch| match branch {
                    Branch::Left(v) | Branch::Right(v) => U256::from(*v),
                })
                .collect();
            Identity::new(commitment, proof)
        })
        .collect()
}
