use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;
use std::time::Duration;

use ethers::abi::AbiEncode;
use ethers::contract::Contract;
use ethers::core::k256::ecdsa::SigningKey;
use ethers::prelude::{
    ContractFactory, Http, LocalWallet, NonceManagerMiddleware, Provider, Signer, SignerMiddleware,
    Wallet,
};
use ethers::providers::Middleware;
use ethers::types::{Bytes, U256};
use ethers::utils::{Anvil, AnvilInstance};
use ethers_solc::artifacts::BytecodeObject;
use tracing::{info, instrument};

use super::{abi as ContractAbi, CompiledContract};

pub type SpecialisedContract = Contract<SpecialisedClient>;

pub struct MockChain {
    pub anvil:            AnvilInstance,
    pub private_key:      SigningKey,
    pub identity_manager: SpecialisedContract,
}

#[instrument(skip_all)]
pub async fn spawn_mock_chain(
    initial_root: U256,
    insertion_batch_sizes: &[usize],
    deletion_batch_sizes: &[usize],
    tree_depth: u8,
) -> anyhow::Result<MockChain> {
    let chain = Anvil::new().block_time(2u64).spawn();
    let private_key = chain.keys()[0].clone().into();

    let provider = Provider::<Http>::try_from(chain.endpoint())
        .expect("Failed to initialize chain endpoint")
        .interval(Duration::from_millis(500u64));

    let chain_id = provider.get_chainid().await?.as_u64();

    let wallet = LocalWallet::from(chain.keys()[0].clone()).with_chain_id(chain_id);

    // connect the wallet to the provider
    let client = SignerMiddleware::new(provider, wallet.clone());
    let client = NonceManagerMiddleware::new(client, wallet.address());
    let client = Arc::new(client);

    // Loading the semaphore verifier contract is special as it requires replacing
    // the address of the Pairing library.
    let pairing_library_factory = load_and_build_contract("./sol/Pairing.json", client.clone())?;
    let pairing_library = pairing_library_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let verifier_path = "./sol/SemaphoreVerifier20.json";
    let verifier_file =
        File::open(verifier_path).unwrap_or_else(|_| panic!("Failed to open `{verifier_path}`"));

    let verifier_contract_json: CompiledContract =
        serde_json::from_reader(BufReader::new(verifier_file))
            .unwrap_or_else(|_| panic!("Could not parse the compiled contract at {verifier_path}"));

    let mut verifier_bytecode_object: BytecodeObject = verifier_contract_json.bytecode.object;

    verifier_bytecode_object
        .link_fully_qualified(
            "lib/semaphore/packages/contracts/contracts/base/Pairing.sol:Pairing",
            pairing_library.address(),
        )
        .resolve()
        .unwrap();

    if verifier_bytecode_object.is_unlinked() {
        panic!("Could not link the Pairing library into the Verifier.");
    }

    let bytecode_bytes = verifier_bytecode_object.as_bytes().unwrap_or_else(|| {
        panic!("Could not parse the bytecode for the contract at {verifier_path}")
    });

    let verifier_factory = ContractFactory::new(
        verifier_contract_json.abi,
        bytecode_bytes.clone(),
        client.clone(),
    );

    let semaphore_verifier = verifier_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let mock_verifier_factory =
        load_and_build_contract("./sol/SequencerVerifier.json", client.clone())?;

    let mock_verifier = mock_verifier_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let unimplemented_verifier_factory =
        load_and_build_contract("./sol/UnimplementedTreeVerifier.json", client.clone())?;

    let unimplemented_verifier = unimplemented_verifier_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let verifier_lookup_table_factory =
        load_and_build_contract("./sol/VerifierLookupTable.json", client.clone())?;

    let first_insertion_batch_size = insertion_batch_sizes.first().copied().unwrap_or(1);
    let first_deletion_batch_size = deletion_batch_sizes.first().copied().unwrap_or(1);

    let insert_verifiers = verifier_lookup_table_factory
        .clone()
        .deploy((first_insertion_batch_size as u64, mock_verifier.address()))?
        .confirmations(0usize)
        .send()
        .await?;

    let update_verifiers = verifier_lookup_table_factory
        .clone()
        .deploy((
            first_insertion_batch_size as u64,
            unimplemented_verifier.address(),
        ))?
        .confirmations(0usize)
        .send()
        .await?;

    let delete_verifiers = verifier_lookup_table_factory
        .deploy((first_deletion_batch_size as u64, mock_verifier.address()))?
        .confirmations(0usize)
        .send()
        .await?;

    for batch_size in insertion_batch_sizes.iter().skip(1).copied() {
        let batch_size = batch_size as u64;

        info!("Adding verifier for batch size {}", batch_size);
        insert_verifiers
            .method::<_, ()>("addVerifier", (batch_size, mock_verifier.address()))?
            .send()
            .await?
            .await?;
    }

    for batch_size in deletion_batch_sizes.iter().skip(1).copied() {
        let batch_size = batch_size as u64;

        info!("Adding verifier for batch size {}", batch_size);
        delete_verifiers
            .method::<_, ()>("addVerifier", (batch_size, mock_verifier.address()))?
            .send()
            .await?
            .await?;
    }

    let identity_manager_impl_factory =
        load_and_build_contract("./sol/WorldIDIdentityManagerImplV2.json", client.clone())?;

    let identity_manager_impl = identity_manager_impl_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    let identity_manager_factory =
        load_and_build_contract("./sol/WorldIDIdentityManager.json", client.clone())?;

    let identity_manager_impl_address = identity_manager_impl.address();

    let init_call_data = ContractAbi::InitializeCall {
        tree_depth,
        initial_root,
        batch_insertion_verifiers: insert_verifiers.address(),
        batch_update_verifiers: update_verifiers.address(),
        semaphore_verifier: semaphore_verifier.address(),
    };
    let init_call_encoded: Bytes = Bytes::from(init_call_data.encode());

    let identity_manager_contract = identity_manager_factory
        .deploy((identity_manager_impl_address, init_call_encoded))?
        .confirmations(0usize)
        .send()
        .await?;

    let identity_manager: SpecialisedContract = Contract::new(
        identity_manager_contract.address(),
        ContractAbi::BATCHINGCONTRACT_ABI.clone(),
        client.clone(),
    );

    identity_manager
        .method::<_, ()>("initializeV2", delete_verifiers.address())?
        .send()
        .await?
        .await?;

    Ok(MockChain {
        anvil: chain,
        private_key,
        identity_manager,
    })
}

type SpecialisedClient =
    NonceManagerMiddleware<SignerMiddleware<Provider<Http>, Wallet<SigningKey>>>;
type SharableClient = Arc<SpecialisedClient>;
type SpecialisedFactory = ContractFactory<SpecialisedClient>;

fn load_and_build_contract(
    path: impl Into<String>,
    client: SharableClient,
) -> anyhow::Result<SpecialisedFactory> {
    let path_string = path.into();
    let contract_file = File::open(&path_string)
        .unwrap_or_else(|_| panic!("Failed to open `{pth}`", pth = &path_string));

    let contract_json: CompiledContract = serde_json::from_reader(BufReader::new(contract_file))
        .unwrap_or_else(|_| {
            panic!(
                "Could not parse the compiled contract at {pth}",
                pth = &path_string
            )
        });
    let contract_bytecode = contract_json.bytecode.object.as_bytes().unwrap_or_else(|| {
        panic!(
            "Could not parse the bytecode for the contract at {pth}",
            pth = &path_string
        )
    });
    let contract_factory =
        ContractFactory::new(contract_json.abi, contract_bytecode.clone(), client);
    Ok(contract_factory)
}
