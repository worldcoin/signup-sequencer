use alloy::network::TransactionBuilder;
use std::fs::File;
use std::io::BufReader;
use std::time::Duration;

use alloy::node_bindings::{Anvil, AnvilInstance};
use alloy::primitives::{Address, Bytes, U256};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::sol_types::{SolCall, SolValue};
use k256::ecdsa::SigningKey;
use tracing::instrument;

use super::abi::IWorldIDIdentityManager;
use super::CompiledContract;

pub type SpecialisedClient = DynProvider;
pub type SpecialisedContract =
    IWorldIDIdentityManager::IWorldIDIdentityManagerInstance<DynProvider>;

pub struct MockChain {
    pub anvil: AnvilInstance,
    pub private_key: SigningKey,
    pub identity_manager: SpecialisedContract,
}

alloy::sol! {
    #[sol(rpc)]
    interface VerifierLookupTable {
        function addVerifier(uint256 batchSize, address verifier) external;
    }
}

fn load_contract(path: &str) -> anyhow::Result<CompiledContract> {
    Ok(serde_json::from_reader(BufReader::new(File::open(path)?))?)
}

async fn deploy(
    client: &DynProvider,
    contract: CompiledContract,
    constructor_args: Vec<u8>,
) -> anyhow::Result<Address> {
    let bytecode = contract
        .bytecode
        .object
        .as_bytes()
        .ok_or_else(|| anyhow::anyhow!("Unlinked contract bytecode"))?;
    let mut input = bytecode.to_vec();
    input.extend(constructor_args);
    let receipt = client
        .send_transaction(TransactionRequest::default().with_deploy_code(Bytes::from(input)))
        .await?
        .get_receipt()
        .await?;
    anyhow::ensure!(receipt.status(), "Contract deployment reverted");
    receipt
        .contract_address
        .ok_or_else(|| anyhow::anyhow!("Deployment has no contract address"))
}

#[instrument(skip_all)]
pub async fn spawn_mock_chain(
    initial_root: U256,
    insertion_batch_sizes: &[usize],
    deletion_batch_sizes: &[usize],
    tree_depth: u8,
) -> anyhow::Result<MockChain> {
    let chain = Anvil::new().block_time(2u64).spawn();
    let private_key: SigningKey = chain.keys()[0].clone().into();
    let wallet = PrivateKeySigner::from(private_key.clone());
    let client = ProviderBuilder::new()
        .wallet(wallet)
        .connect_http(chain.endpoint().parse()?)
        .erased();
    client
        .client()
        .set_poll_interval(Duration::from_millis(500));

    let pairing = deploy(&client, load_contract("./sol/Pairing.json")?, vec![]).await?;
    let mut verifier = load_contract("./sol/SemaphoreVerifier20.json")?;
    verifier
        .bytecode
        .object
        .link_fully_qualified(
            "lib/semaphore/packages/contracts/contracts/base/Pairing.sol:Pairing",
            pairing,
        )
        .resolve();
    anyhow::ensure!(
        !verifier.bytecode.object.is_unlinked(),
        "Could not link Pairing"
    );
    let semaphore_verifier = deploy(&client, verifier, vec![]).await?;
    let mock_verifier = deploy(
        &client,
        load_contract("./sol/SequencerVerifier.json")?,
        vec![],
    )
    .await?;
    let unimplemented = deploy(
        &client,
        load_contract("./sol/UnimplementedTreeVerifier.json")?,
        vec![],
    )
    .await?;

    let first_insert = U256::from(insertion_batch_sizes.first().copied().unwrap_or(1));
    let first_delete = U256::from(deletion_batch_sizes.first().copied().unwrap_or(1));
    let insert_verifiers = deploy(
        &client,
        load_contract("./sol/VerifierLookupTable.json")?,
        (first_insert, mock_verifier).abi_encode_params(),
    )
    .await?;
    let update_verifiers = deploy(
        &client,
        load_contract("./sol/VerifierLookupTable.json")?,
        (first_insert, unimplemented).abi_encode_params(),
    )
    .await?;
    let delete_verifiers = deploy(
        &client,
        load_contract("./sol/VerifierLookupTable.json")?,
        (first_delete, mock_verifier).abi_encode_params(),
    )
    .await?;

    for (address, batch_sizes) in [
        (insert_verifiers, insertion_batch_sizes),
        (delete_verifiers, deletion_batch_sizes),
    ] {
        let table = VerifierLookupTable::new(address, client.clone());
        for &batch_size in batch_sizes.iter().skip(1) {
            let receipt = table
                .addVerifier(U256::from(batch_size), mock_verifier)
                .send()
                .await?
                .get_receipt()
                .await?;
            anyhow::ensure!(receipt.status(), "Adding verifier reverted");
        }
    }
    let implementation = deploy(
        &client,
        load_contract("./sol/WorldIDIdentityManagerImplV2.json")?,
        vec![],
    )
    .await?;
    let init = IWorldIDIdentityManager::initializeCall {
        treeDepth: tree_depth,
        initialRoot: initial_root,
        _batchInsertionVerifiers: insert_verifiers,
        _batchUpdateVerifiers: update_verifiers,
        _semaphoreVerifier: semaphore_verifier,
    }
    .abi_encode();
    let address = deploy(
        &client,
        load_contract("./sol/WorldIDIdentityManager.json")?,
        (implementation, Bytes::from(init)).abi_encode_params(),
    )
    .await?;
    let identity_manager = IWorldIDIdentityManager::new(address, client);
    let receipt = identity_manager
        .initializeV2(delete_verifiers)
        .send()
        .await?
        .get_receipt()
        .await?;
    anyhow::ensure!(receipt.status(), "initializeV2 reverted");
    Ok(MockChain {
        anvil: chain,
        private_key,
        identity_manager,
    })
}
