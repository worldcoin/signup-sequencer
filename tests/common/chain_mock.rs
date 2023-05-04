use std::{fs::File, io::BufReader, sync::Arc, time::Duration};

use ethers::{
    abi::AbiEncode,
    contract::Contract,
    core::k256::ecdsa::SigningKey,
    prelude::{
        artifacts::BytecodeObject, ContractFactory, Http, LocalWallet, NonceManagerMiddleware,
        Provider, Signer, SignerMiddleware, Wallet,
    },
    providers::Middleware,
    types::{Bytes, H256, U256},
    utils::{Anvil, AnvilInstance},
};
use tracing::{info, instrument};

use super::{abi as ContractAbi, CompiledContract};

pub type SpecialisedContract = Contract<SpecialisedClient>;

pub struct MockChain {
    pub anvil:            AnvilInstance,
    pub private_key:      H256,
    pub identity_manager: SpecialisedContract,
}

#[instrument(skip_all)]
pub async fn spawn_mock_chain(
    initial_root: U256,
    batch_sizes: &[usize],
    tree_depth: u8,
) -> anyhow::Result<MockChain> {
    let chain = Anvil::new().block_time(2u64).spawn();
    let private_key = H256::from_slice(&chain.keys()[0].to_be_bytes());

    let provider = Provider::<Http>::try_from(chain.endpoint())
        .expect("Failed to initialize chain endpoint")
        .interval(Duration::from_millis(500u64));

    let chain_id = provider.get_chainid().await?.as_u64();

    let wallet = LocalWallet::from(chain.keys()[0].clone()).with_chain_id(chain_id);

    // connect the wallet to the provider
    let client = SignerMiddleware::new(provider, wallet.clone());
    let client = NonceManagerMiddleware::new(client, wallet.address());
    let client = Arc::new(client);

    info!("Spawning the pairing library");

    // Loading the semaphore verifier contract is special as it requires replacing
    // the address of the Pairing library.
    let pairing_library_factory = load_and_build_contract("./sol/Pairing.json", client.clone())?;
    let pairing_library = pairing_library_factory
        .deploy(())?
        .confirmations(0usize)
        .send()
        .await?;

    info!("Pairing library spawned");

    let verifier_path = "./sol/SemaphoreVerifier.json";
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

    info!("Deploying semaphore verifier");
    let semaphore_verifier = verifier_factory.deploy(())?.confirmations(0usize).send();

    // The rest of the contracts can be deployed to the mock chain normally.
    let mock_state_bridge_factory =
        load_and_build_contract("./sol/SimpleStateBridge.json", client.clone())?;

    info!("Deploying mock state bridge");
    let mock_state_bridge = mock_state_bridge_factory
        .deploy(())?
        .confirmations(0usize)
        .send();

    let mock_verifier_factory =
        load_and_build_contract("./sol/SequencerVerifier.json", client.clone())?;

    info!("Deploying mock verifier");
    let mock_verifier = mock_verifier_factory
        .deploy(())?
        .confirmations(0usize)
        .send();

    let unimplemented_verifier_factory =
        load_and_build_contract("./sol/UnimplementedTreeVerifier.json", client.clone())?;

    info!("Deploying unimplemented verifier");
    let unimplemented_verifier = unimplemented_verifier_factory
        .deploy(())?
        .confirmations(0usize)
        .send();

    let (semaphore_verifier, mock_state_bridge, mock_verifier, unimplemented_verifier) = tokio::join!(
        semaphore_verifier,
        mock_state_bridge,
        mock_verifier,
        unimplemented_verifier
    );

    info!("Deploying stuff done!");

    let semaphore_verifier = semaphore_verifier?;
    info!("semaphore!");
    let mock_state_bridge = mock_state_bridge?;
    info!("mock state bridge!");
    let mock_verifier = mock_verifier?;
    info!("mock verifier!");
    let unimplemented_verifier = unimplemented_verifier?;
    info!("unimplemented verifier!");

    let verifier_lookup_table_factory =
        load_and_build_contract("./sol/VerifierLookupTable.json", client.clone())?;

    let first_batch_size = batch_sizes[0];

    info!("Spawning verifier insert lookup table");
    let insert_verifiers = verifier_lookup_table_factory
        .clone()
        .deploy((first_batch_size as u64, mock_verifier.address()))?
        .confirmations(0usize)
        .send();

    info!("Spawning verifier update lookup table");
    let update_verifiers = verifier_lookup_table_factory
        .deploy((first_batch_size as u64, unimplemented_verifier.address()))?
        .confirmations(0usize)
        .send();

    let identity_manager_impl_factory =
        load_and_build_contract("./sol/WorldIDIdentityManagerImplV1.json", client.clone())?;

    info!("Spawning identity manager");
    let identity_manager_impl = identity_manager_impl_factory
        .deploy(())?
        .confirmations(0usize)
        .send();

    let (insert_verifiers, update_verifiers, identity_manager_impl) =
        tokio::join!(insert_verifiers, update_verifiers, identity_manager_impl);

    info!("Awaited all");

    let insert_verifiers = insert_verifiers?;
    let update_verifiers = update_verifiers?;
    let identity_manager_impl = identity_manager_impl?;

    info!("Spawning verifiers");
    // TODO: This is sequential but could be parallelized.
    // but for now it's only multiple batch sizes for one test so I don't wanna do
    // it now.
    for batch_size in &batch_sizes[1..] {
        let batch_size = *batch_size as u64;

        info!("Adding verifier for batch size {}", batch_size);
        insert_verifiers
            .method::<_, ()>("addVerifier", (batch_size, mock_verifier.address()))?
            .send()
            .await?
            .await?;
    }
    info!("Verifiers spawned");

    let identity_manager_factory =
        load_and_build_contract("./sol/WorldIDIdentityManager.json", client.clone())?;
    let state_bridge_address = mock_state_bridge.address();
    let enable_state_bridge = true;
    let identity_manager_impl_address = identity_manager_impl.address();

    let init_call_data = ContractAbi::InitializeCall {
        tree_depth,
        initial_root,
        batch_insertion_verifiers: insert_verifiers.address(),
        batch_update_verifiers: update_verifiers.address(),
        semaphore_verifier: semaphore_verifier.address(),
        enable_state_bridge,
        state_bridge: state_bridge_address,
    };
    let init_call_encoded: Bytes = Bytes::from(init_call_data.encode());

    info!("Spawning identity manager");
    let identity_manager_contract = identity_manager_factory
        .deploy((identity_manager_impl_address, init_call_encoded))?
        .confirmations(0usize)
        .send()
        .await?;

    info!("Identity manager spawned");

    let identity_manager: SpecialisedContract = Contract::new(
        identity_manager_contract.address(),
        ContractAbi::BATCHINGCONTRACT_ABI.clone(),
        client.clone(),
    );

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
