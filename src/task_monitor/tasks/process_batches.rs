use std::sync::Arc;
use std::time::Duration;

use ethers::types::U256;
use tokio::sync::{mpsc, Notify};
use tokio::{select, time};
use tracing::instrument;

use crate::app::App;
use crate::contracts::IdentityManager;
use crate::database::types::{BatchEntry, BatchType};
use crate::database::DatabaseExt as _;
use crate::ethereum::write::TransactionId;
use crate::prover::Prover;
use crate::utils::index_packing::pack_indices;

pub async fn process_batches(
    app: Arc<App>,
    monitored_txs_sender: Arc<mpsc::Sender<TransactionId>>,
    next_batch_notify: Arc<Notify>,
    wake_up_notify: Arc<Notify>,
) -> anyhow::Result<()> {
    tracing::info!("Awaiting for a clean slate");
    app.identity_manager.await_clean_slate().await?;

    tracing::info!("Starting identity processor.");

    // We start a timer and force it to perform one initial tick to avoid an
    // immediate trigger.
    let mut timer = time::interval(Duration::from_secs(5));

    loop {
        // We wait either for a timer tick or a full batch
        select! {
            _ = timer.tick() => {
                tracing::info!("Identity batch insertion woken due to timeout");
            }

            () = next_batch_notify.notified() => {
                tracing::trace!("Identity batch insertion woken due to next batch creation");
            },

            () = wake_up_notify.notified() => {
                tracing::trace!("Identity batch insertion woken due to request");
            },
        }

        let next_batch = app.database.get_next_batch_without_transaction().await?;
        let Some(next_batch) = next_batch else {
            continue;
        };

        let tx_id =
            commit_identities(&app.identity_manager, &monitored_txs_sender, &next_batch).await?;

        if let Some(tx_id) = tx_id {
            app.database
                .insert_new_transaction(&tx_id.0, &next_batch.next_root)
                .await?;
        }

        // We want to check if there's a full batch available immediately
        wake_up_notify.notify_one();
    }
}

async fn commit_identities(
    identity_manager: &IdentityManager,
    monitored_txs_sender: &mpsc::Sender<TransactionId>,
    batch: &BatchEntry,
) -> anyhow::Result<Option<TransactionId>> {
    // If the update is an insertion
    let tx_id = if batch.batch_type == BatchType::Insertion {
        let prover = identity_manager
            .get_suitable_insertion_prover(batch.data.0.identities.len())
            .await?;

        tracing::info!(
            num_updates = batch.data.0.identities.len(),
            batch_size = prover.batch_size(),
            "Insertion batch",
        );

        insert_identities(identity_manager, &prover, batch).await?
    } else {
        let prover = identity_manager
            .get_suitable_deletion_prover(batch.data.0.identities.len())
            .await?;

        tracing::info!(
            num_updates = batch.data.0.identities.len(),
            batch_size = prover.batch_size(),
            "Deletion batch"
        );

        delete_identities(identity_manager, &prover, batch).await?
    };

    if let Some(tx_id) = tx_id.clone() {
        monitored_txs_sender.send(tx_id).await?;
    }

    Ok(tx_id)
}

#[instrument(level = "info", skip_all)]
pub async fn insert_identities(
    identity_manager: &IdentityManager,
    prover: &Prover,
    batch: &BatchEntry,
) -> anyhow::Result<Option<TransactionId>> {
    identity_manager.validate_merkle_proofs(&batch.data.0.identities)?;
    let start_index = *batch.data.0.indexes.first().expect("Should exist.");
    let pre_root: U256 = batch.prev_root.expect("Should exist.").into();
    let post_root: U256 = batch.next_root.into();

    // We prepare the proof before reserving a slot in the pending identities
    let proof = IdentityManager::prepare_insertion_proof(
        prover,
        start_index,
        pre_root,
        &batch.data.0.identities,
        post_root,
    )
    .await?;

    tracing::info!(
        start_index,
        ?pre_root,
        ?post_root,
        "Submitting insertion batch"
    );

    // With all the data prepared we can submit the identities to the on-chain
    // identity manager and wait for that transaction to be mined.
    let transaction_id = identity_manager
        .register_identities(
            start_index,
            pre_root,
            post_root,
            batch.data.0.identities.clone(),
            proof,
        )
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to insert identity to contract.");
            e
        })?;

    tracing::info!(
        start_index,
        ?pre_root,
        ?post_root,
        ?transaction_id,
        "Insertion batch submitted"
    );

    Ok(Some(transaction_id))
}

pub async fn delete_identities(
    identity_manager: &IdentityManager,
    prover: &Prover,
    batch: &BatchEntry,
) -> anyhow::Result<Option<TransactionId>> {
    identity_manager.validate_merkle_proofs(&batch.data.0.identities)?;
    let pre_root: U256 = batch.prev_root.expect("Should exist.").into();
    let post_root: U256 = batch.next_root.into();
    let deletion_indices: Vec<_> = batch.data.0.indexes.iter().map(|&v| v as u32).collect();

    // We prepare the proof before reserving a slot in the pending identities
    let proof = IdentityManager::prepare_deletion_proof(
        prover,
        pre_root,
        deletion_indices.clone(),
        batch.data.0.identities.clone(),
        post_root,
    )
    .await?;

    let packed_deletion_indices = pack_indices(&deletion_indices);

    tracing::info!(?pre_root, ?post_root, "Submitting deletion batch");

    // With all the data prepared we can submit the identities to the on-chain
    // identity manager and wait for that transaction to be mined.
    let transaction_id = identity_manager
        .delete_identities(proof, packed_deletion_indices, pre_root, post_root)
        .await
        .map_err(|e| {
            tracing::error!(?e, "Failed to insert identity to contract.");
            e
        })?;

    tracing::info!(
        ?pre_root,
        ?post_root,
        ?transaction_id,
        "Deletion batch submitted"
    );

    Ok(Some(transaction_id))
}
