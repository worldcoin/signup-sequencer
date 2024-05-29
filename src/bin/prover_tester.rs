use std::ops::Rem;
use std::sync::Arc;

use clap::Parser;
use ethers::contract::abigen;
use ethers::providers::{Http, Provider};
use ethers::types::{Address, U256};
use semaphore::lazy_merkle_tree::LazyMerkleTree;
use semaphore::merkle_tree::Branch;
use semaphore::poseidon_tree::PoseidonHash;
use signup_sequencer::prover::{
    compute_insertion_proof_input_hash, Prover, ProverConfig, ProverType,
};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use url::Url;

type Field = ruint::aliases::U256;

abigen!(
    ITreeVerifier,
    r#"[
        function verifyProof(uint256[8] calldata proof, uint256[1] calldata input) external;
    ]"#
);

/// A simple CLI for testing the prover & verifier
#[derive(Debug, Clone, Parser)]
#[clap(rename_all = "kebab-case")]
struct Args {
    /// The address of the verifier contract
    #[clap(short, long)]
    verifier_address: Address,
    /// The prover HTTP url
    #[clap(short, long)]
    prover_url:       Url,
    #[clap(short, long)]
    batch_size:       usize,
    /// The prover type
    #[clap(short = 't', long)]
    prover_type:      ProverType,
    /// RPC Url
    #[clap(short, long)]
    rpc_url:          Url,

    #[clap(short, long, default_value = "30")]
    depth:            usize,
    #[clap(short, long, default_value = "0x000")]
    empty_leaf_value: Field,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                .from_env_lossy()
                .add_directive(LevelFilter::INFO.into()),
        )
        .init();

    let args = Args::parse();

    tracing::info!(?args, "Start");

    let prover_config = ProverConfig {
        url:         args.prover_url.to_string(),
        timeout_s:   30,
        batch_size:  args.batch_size,
        prover_type: args.prover_type,
    };

    let prover = Prover::new(&prover_config)?;

    let mut tree = LazyMerkleTree::<PoseidonHash>::new(args.depth, args.empty_leaf_value);

    match args.prover_type {
        ProverType::Insertion => {
            let mut identities = Vec::with_capacity(args.batch_size);

            let pre_root: U256 = tree.root().into();

            tracing::info!("Building proof input");
            for i in 0..args.batch_size {
                let commitment = Field::from(i);
                tree = tree.update_with_mutation(i, &commitment);

                let merkle_proof = tree.proof(i);
                let merkle_proof: Vec<_> = merkle_proof
                    .0
                    .into_iter()
                    .map(unbranch)
                    .map(U256::from)
                    .collect();

                identities.push(signup_sequencer::prover::identity::Identity {
                    commitment: commitment.into(),
                    merkle_proof,
                });
            }

            let post_root: U256 = tree.root().into();

            tracing::info!("Generating proof");
            let insertion_proof = prover
                .generate_insertion_proof(0, pre_root, post_root, &identities)
                .await?;

            let identities: Vec<_> = identities.into_iter().map(|i| i.commitment).collect();

            let snark_scalar_field = Field::from_str_radix(
                "21888242871839275222246405745257275088548364400416034343698204186575808495617",
                10,
            )
            .expect("This should just parse.");

            let input_hash =
                compute_insertion_proof_input_hash(0, pre_root, post_root, &identities);

            let snark_scalar_field: U256 = snark_scalar_field.into();
            let input_hash = input_hash.rem(snark_scalar_field);

            let proof_points_array: [U256; 8] = insertion_proof.into();

            let provider = Arc::new(Provider::new(Http::new(args.rpc_url)));
            let contract = ITreeVerifier::new(args.verifier_address, provider);

            tracing::info!("Verifying proof");
            let resp = contract
                .verify_proof(proof_points_array, [input_hash])
                .await;

            match resp {
                Ok(_) => tracing::info!("Proof verified"),
                Err(error) => tracing::error!(?error, "Proof verification failed"),
            }
        }
        ProverType::Deletion => {
            // TODO:
            // prover.generate_deletion_proof(
            //     pre_root,
            //     post_root,
            //     deletion_indices,
            //     identities
            // )
        }
    }

    Ok(())
}

fn unbranch(c: Branch<PoseidonHash>) -> Field {
    match c {
        Branch::Left(c) | Branch::Right(c) => c,
    }
}
