use std::path::{Path, PathBuf};

use anyhow::Context;
use clap::Parser;
use ethers::core::rand::{thread_rng, RngCore};
use semaphore_rs::identity::Identity;
use semaphore_rs::poseidon_tree::Proof;
use semaphore_rs::Field;
use serde::{Deserialize, Serialize};
use signup_sequencer::server::api_v1::data::{
    InclusionProofRequest, InclusionProofResponse, InsertCommitmentRequest,
    VerifySemaphoreProofRequest, VerifySemaphoreProofResponse,
};

#[derive(Debug, Parser)]
struct Args {
    #[clap(subcommand)]
    subcommand: Command,

    #[clap(
        short,
        long,
        env,
        default_value = "https://signup-orb-ethereum.crypto.worldcoin.dev"
    )]
    sequencer_url: String,

    #[clap(long, env)]
    basic_auth_username: Option<String>,

    #[clap(long, env)]
    basic_auth_password: Option<String>,

    /// The path to the file that will be used to store the identity
    #[clap(long, env)]
    identity_file: Option<PathBuf>,

    /// The path to the file that will be used to store the inclusion proof
    #[clap(long, env)]
    inclusion_proof_file: Option<PathBuf>,

    /// The path to the file that will be used to store the semaphore proof
    #[clap(long, env)]
    semaphore_proof_file: Option<PathBuf>,
}

#[derive(Debug, Parser)]
enum Command {
    /// Generated an identity
    #[clap(visible_alias = "g")]
    Generate,
    /// Prove inclusion
    #[clap(visible_alias = "i")]
    #[clap(visible_alias = "ip")]
    InclusionProof(InclusionProofCmd),
    /// Verify a semaphore proof
    #[clap(visible_alias = "v")]
    #[clap(visible_alias = "vp")]
    #[clap(visible_alias = "pv")]
    VerifyProof(VerifyProofCmd),
    /// Insert an identity
    #[clap(visible_alias = "ii")]
    InsertIdentity(InsertIdentityCmd),

    /// Generate an arbitrary proof
    #[clap(visible_alias = "pg")]
    #[clap(visible_alias = "gp")]
    GenerateProof(GenerateProofCmd),
}

#[derive(Debug, Parser)]
struct InclusionProofCmd {
    #[clap(short, long)]
    commitment: Option<Field>,
}

#[derive(Debug, Parser)]
struct InsertIdentityCmd {
    #[clap(short, long)]
    commitment: Option<Field>,
}

#[derive(Debug, Parser)]
struct GenerateProofCmd {
    #[clap(short, long)]
    external_nullifier_hash: Field,

    #[clap(short, long)]
    signal_hash: Field,
}

#[derive(Debug, Parser)]
struct VerifyProofCmd {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    let args = Args::parse();

    match args.subcommand {
        Command::Generate => {
            let mut rng = thread_rng();

            let mut secret = [0_u8; 64];
            rng.fill_bytes(&mut secret);

            let identity = Identity::from_secret(&mut secret, None);

            let commitment = identity.commitment();

            if let Some(path) = args.identity_file {
                save_identity(path, &identity).await?;
            } else {
                println!("Nullifier: {}", identity.nullifier);
                println!("Identity: {}", identity.trapdoor);
            }

            println!("{}", commitment);
        }
        Command::InsertIdentity(x) => {
            let basic_auth_username = args
                .basic_auth_username
                .context("Missing basic auth username")?;
            let basic_auth_password = args
                .basic_auth_password
                .context("Missing basic auth password")?;

            let identity_commitment = if let Some(commitment) = x.commitment {
                commitment
            } else if let Some(identity_path) = args.identity_file.as_ref() {
                let identity = load_identity(identity_path).await?;

                identity.commitment()
            } else {
                return Err(anyhow::anyhow!(
                    "Missing commitment - must set either --commitment or --identity"
                ));
            };

            let client = reqwest::Client::new();

            let response = client
                .post(format!("{}/insertIdentity", args.sequencer_url))
                .basic_auth(basic_auth_username, Some(basic_auth_password))
                .json(&InsertCommitmentRequest {
                    identity_commitment,
                })
                .send()
                .await?;

            let _response = response.error_for_status()?;
        }
        Command::InclusionProof(x) => {
            let identity_commitment = if let Some(commitment) = x.commitment {
                commitment
            } else if let Some(identity_path) = args.identity_file.as_ref() {
                let identity = load_identity(identity_path).await?;

                identity.commitment()
            } else {
                return Err(anyhow::anyhow!(
                    "Missing commitment - must set either --commitment or --identity"
                ));
            };

            let client = reqwest::Client::new();

            let response = client
                .post(format!("{}/inclusionProof", args.sequencer_url))
                .json(&InclusionProofRequest {
                    identity_commitment,
                })
                .send()
                .await?;

            let response = response.error_for_status()?;

            let response: InclusionProofResponse = response.json().await?;

            let proof: Proof = response.proof.context("Missing proof")?;
            let proof_serialized = serde_json::to_string_pretty(&proof)?;

            if let Some(inclusion_proof_file) = args.inclusion_proof_file.as_ref() {
                tokio::fs::write(inclusion_proof_file, &proof_serialized).await?;
            }

            println!("{}", proof_serialized);
        }
        Command::VerifyProof(_x) => {
            let proof_request = tokio::fs::read_to_string(
                args.semaphore_proof_file
                    .as_ref()
                    .context("Missing semaphore proof")?,
            )
            .await?;
            let proof_request: VerifySemaphoreProofRequest = serde_json::from_str(&proof_request)?;

            let client = reqwest::Client::new();

            let response = client
                .post(format!("{}/verifySemaphoreProof", args.sequencer_url))
                .json(&proof_request)
                .send()
                .await?;

            let response = response.error_for_status()?;

            let response: VerifySemaphoreProofResponse = response.json().await?;
            let response_serialized = serde_json::to_string_pretty(&response)?;

            println!("{}", response_serialized);
        }
        Command::GenerateProof(x) => {
            let identity =
                load_identity(args.identity_file.as_ref().context("Missing --identity")?).await?;

            let client = reqwest::Client::new();

            let response = client
                .post(format!("{}/inclusionProof", args.sequencer_url))
                .json(&InclusionProofRequest {
                    identity_commitment: identity.commitment(),
                })
                .send()
                .await?;

            let response = response.error_for_status()?;

            let response: InclusionProofResponse = response.json().await?;

            let root = response.root.context("Missing root")?;

            let nullifier_hash = semaphore_rs::protocol::generate_nullifier_hash(
                &identity,
                x.external_nullifier_hash,
            );

            let proof = semaphore_rs::protocol::generate_proof(
                &identity,
                &response.proof.context("Missing proof")?,
                x.external_nullifier_hash,
                x.signal_hash,
            )?;

            let semaphore_request = VerifySemaphoreProofRequest {
                root,
                signal_hash: x.signal_hash,
                nullifier_hash,
                external_nullifier_hash: x.external_nullifier_hash,
                proof,
            };

            let semaphore_request_serialized = serde_json::to_string_pretty(&semaphore_request)?;

            if let Some(semaphore_proof_file) = args.semaphore_proof_file.as_ref() {
                tokio::fs::write(semaphore_proof_file, &semaphore_request_serialized).await?;
            }

            println!("{}", semaphore_request_serialized);
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SerializedIdentity {
    nullifier: Field,
    trapdoor: Field,
}

async fn load_identity(path: impl AsRef<Path>) -> anyhow::Result<Identity> {
    let identity = tokio::fs::read_to_string(path.as_ref()).await?;

    let identity: SerializedIdentity = serde_json::from_str(&identity)?;

    Ok(Identity {
        nullifier: identity.nullifier,
        trapdoor: identity.trapdoor,
    })
}

async fn save_identity(path: impl AsRef<Path>, identity: &Identity) -> anyhow::Result<()> {
    let identity = SerializedIdentity {
        nullifier: identity.nullifier,
        trapdoor: identity.trapdoor,
    };

    let identity = serde_json::to_string_pretty(&identity)?;

    tokio::fs::write(path, identity).await?;

    Ok(())
}
