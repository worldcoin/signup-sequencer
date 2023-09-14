use std::fmt::{Display, Formatter};
use std::mem::size_of;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use axum_server::Handle;
use ethers::types::U256;
use ethers::utils::keccak256;
use hyper::StatusCode;
use semaphore::poseidon_tree::{Branch, Proof as TreeProof};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

/// A representation of an error from the prover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProverError {
    pub code:    String,
    pub message: String,
}

impl Display for ProverError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PROVER FAILURE: Code = {}, Message = {}",
            self.code, self.message
        )
    }
}

/// The input to the prover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InsertionProofInput {
    input_hash:           U256,
    start_index:          u32,
    pre_root:             U256,
    post_root:            U256,
    identity_commitments: Vec<U256>,
    merkle_proofs:        Vec<Vec<U256>>,
}

// TODO: ideally we just import the InsertionProofInput and DeletionProofInput
// from the signup sequencer so that we can know e2e breaks when any interface
// changes occur

/// The input to the prover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeletionProofInput {
    input_hash:           U256,
    pre_root:             U256,
    post_root:            U256,
    deletion_indices:     Vec<u32>,
    identity_commitments: Vec<U256>,
    merkle_proofs:        Vec<Vec<U256>>,
}

/// The proof response from the prover.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proof {
    pub ar:  [U256; 2],
    pub bs:  [[U256; 2]; 2],
    pub krs: [U256; 2],
}

impl From<[U256; 8]> for Proof {
    fn from(value: [U256; 8]) -> Self {
        Self {
            ar:  [value[0], value[1]],
            bs:  [[value[2], value[3]], [value[4], value[5]]],
            krs: [value[6], value[7]],
        }
    }
}

/// A transparent enum (untagged in serialization) to make it easy to return
/// multiple types in the endpoint.
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
enum ProveResponse {
    ProofSuccess(Proof),
    ProofFailure(ProverError),
}

impl ProveResponse {
    /// Constructs a success response containing the provided `terms`.
    pub fn success(terms: impl Into<Proof>) -> Self {
        let proof: Proof = terms.into();
        Self::ProofSuccess(proof)
    }

    /// Constructs a failure response from the provided `code` and `message`.
    pub fn failure(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::ProofFailure(ProverError {
            code:    code.into(),
            message: message.into(),
        })
    }
}

/// The mock prover service.
pub struct ProverService {
    server:      Handle,
    inner:       Arc<Mutex<Prover>>,
    address:     SocketAddr,
    batch_size:  usize,
    prover_type: ProverType,
}

// TODO: we could just import this from the sequencer
#[derive(Debug, Copy, Clone, sqlx::Type, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[sqlx(type_name = "prover_enum", rename_all = "PascalCase")]
pub enum ProverType {
    #[default]
    Insertion,
    Deletion,
}

impl std::fmt::Display for ProverType {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ProverType::Insertion => write!(f, "insertion"),
            ProverType::Deletion => write!(f, "deletion"),
        }
    }
}

struct Prover {
    is_available: bool,
}

impl ProverService {
    /// Returns a new instance of the mock prover service, serving at the
    /// provided `url`.
    ///
    /// It provides only a single endpoint for now, `/prove` in order to match
    /// the full service (`semaphore-mtb`). This can be extended in the future
    /// if needed.
    pub async fn new(batch_size: usize, prover_type: ProverType) -> anyhow::Result<Self> {
        async fn prove(
            state: State<Arc<Mutex<Prover>>>,
            Json(input): Json<serde_json::Value>,
        ) -> Result<Json<ProveResponse>, StatusCode> {
            let state = state.lock().await;

            // Attempt to deserialize into InsertionProofInput
            if let Ok(deserialized_insertion_input) =
                serde_json::from_value::<InsertionProofInput>(input.clone())
            {
                return state
                    .prove_insertion(deserialized_insertion_input)
                    .map(Json);
            }

            // If the above fails, attempt to deserialize into DeletionProofInput
            if let Ok(deserialized_deletion_input) =
                serde_json::from_value::<DeletionProofInput>(input)
            {
                return state.prove_deletion(deserialized_deletion_input).map(Json);
            }

            // If both fail, return an error
            Err(StatusCode::BAD_REQUEST)
        }

        let inner = Arc::new(Mutex::new(Prover { is_available: true }));
        let state = inner.clone();

        let app = Router::new().route("/prove", post(prove)).with_state(state);

        // We use a random port here so that we can run multiple tests in many
        // threads/tasks
        let addr: SocketAddr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0));

        let server = Handle::new();
        let serverside_handle = server.clone();
        let service = app.into_make_service();

        tokio::spawn(async move {
            axum_server::bind(addr)
                .handle(serverside_handle)
                .serve(service)
                .await
                .expect("Failed to bind server");
        });

        let address = server.listening().await.context("Failed to bind server")?;

        let service = Self {
            server,
            inner,
            address,
            batch_size,
            prover_type,
        };

        Ok(service)
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.address)
    }

    pub async fn set_availability(&self, availability: bool) {
        let mut inner = self.inner.lock().await;
        inner.is_available = availability;
    }

    /// Shuts down the server and frees up the socket that it was using.
    pub fn stop(self) {
        self.server.shutdown();
    }

    pub fn batch_size(&self) -> usize {
        self.batch_size
    }

    pub fn prover_type(&self) -> ProverType {
        self.prover_type
    }

    /// Produces an arg string that's compatible with this prover - can be used
    /// as is in the CLI args
    ///
    /// e.g. `[{"url": "http://localhost:3001","batch_size": 3,"timeout_s": 30}]`
    pub fn arg_string(&self) -> String {
        format!("[{}]", self.arg_string_single())
    }

    /// Produces an arg string that's compatible with this prover - needs to be
    /// wrapped in an array
    ///
    /// e.g. `{"url": "http://localhost:3001","batch_size": 3,"timeout_s": 30,"prover_type": "insertion"}`
    pub fn arg_string_single(&self) -> String {
        format!(
            r#"{{"url": "{}","batch_size": {},"timeout_s": 30, "prover_type": "{}"}}"#,
            self.url(),
            self.batch_size,
            self.prover_type
        )
    }
}

impl Prover {
    fn prove_insertion(&self, input: InsertionProofInput) -> Result<ProveResponse, StatusCode> {
        if !self.is_available {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }

        // Calculate the input hash based on the prover parameters.
        let input_hash = Self::calculate_identity_registration_input_hash(&input);

        // If the hashes aren't the same something's wrong so we return an error.
        if input_hash != input.input_hash {
            return Ok(ProveResponse::failure("42", "Input hash mismatch."));
        }

        // Next we verify the merkle proofs.
        let empty_leaf = U256::zero();
        let mut last_root = input.pre_root;

        for (index, (identity, merkle_proof)) in input
            .identity_commitments
            .iter()
            .zip(input.merkle_proofs)
            .enumerate()
        {
            let leaf_index = input.start_index as usize + index;
            let proof = Self::reconstruct_proof_with_directions(leaf_index, &merkle_proof);
            let root: U256 = proof.root(empty_leaf.into()).into();
            if root != last_root {
                break;
            }
            last_root = proof.root((*identity).into()).into();
        }

        // If the final root doesn't match the post root something's broken so we error.
        if last_root != input.post_root {
            return Ok(ProveResponse::failure(
                "43",
                "Merkle proof verification failure.",
            ));
        }

        // If we succeed in verifying, the output should be correlated with the input,
        // so we use the input_hash as part of it.
        Ok(ProveResponse::success([
            "0x2".into(),
            input_hash,
            "0x2413396a2af3add6fbe8137cfe7657917e31a5cdab0b7d1d645bd5eeb47ba601".into(),
            "0x1ad029539528b32ba70964ce43dbf9bba2501cdb3aaa04e4d58982e2f6c34752".into(),
            "0x5bb975296032b135458bd49f92d5e9d363367804440d4692708de92e887cf17".into(),
            "0x14932600f53a1ceb11d79a7bdd9688a2f8d1919176f257f132587b2b3274c41e".into(),
            "0x13d7b19c7b67bf5d3adf2ac2d3885fd5d49435b6069c0656939cd1fb7bef9dc9".into(),
            "0x142e14f90c49c79b4edf5f6b7acbcdb0b0f376a4311fc036f1006679bd53ca9e".into(),
        ]))
    }

    fn prove_deletion(&self, input: DeletionProofInput) -> Result<ProveResponse, StatusCode> {
        if !self.is_available {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        }

        // Calculate the input hash based on the prover parameters.
        let input_hash = Self::compute_deletion_proof_input_hash(
            input.pre_root,
            &input.identity_commitments,
            input.post_root,
        );

        // If the hashes aren't the same something's wrong so we return an error.
        if input_hash != input.input_hash {
            return Ok(ProveResponse::failure("42", "Input hash mismatch."));
        }

        // Next we verify the merkle proofs.
        let empty_leaf = U256::zero();
        let mut last_root = input.pre_root;

        for (leaf_index, merkle_proof) in input.deletion_indices.iter().zip(input.merkle_proofs) {
            let proof =
                Self::reconstruct_proof_with_directions(*leaf_index as usize, &merkle_proof);
            last_root = proof.root(empty_leaf.into()).into();
        }

        // If the final root doesn't match the post root something's broken so we error.
        if last_root != input.post_root {
            return Ok(ProveResponse::failure(
                "43",
                "Merkle proof verification failure.",
            ));
        }

        // TODO: need to generate deletion proof from semaphore-mock
        Ok(ProveResponse::success([
            "0x2".into(),
            input_hash,
            "0x2413396a2af3add6fbe8137cfe7657917e31a5cdab0b7d1d645bd5eeb47ba601".into(),
            "0x1ad029539528b32ba70964ce43dbf9bba2501cdb3aaa04e4d58982e2f6c34752".into(),
            "0x5bb975296032b135458bd49f92d5e9d363367804440d4692708de92e887cf17".into(),
            "0x14932600f53a1ceb11d79a7bdd9688a2f8d1919176f257f132587b2b3274c41e".into(),
            "0x13d7b19c7b67bf5d3adf2ac2d3885fd5d49435b6069c0656939cd1fb7bef9dc9".into(),
            "0x142e14f90c49c79b4edf5f6b7acbcdb0b0f376a4311fc036f1006679bd53ca9e".into(),
        ]))
    }

    /// Reconstructs the proof with directions as required by `semaphore-rs`.
    ///
    /// This allows us to utilise the proof verification procedure from that
    /// library instead of implementing our own.
    fn reconstruct_proof_with_directions(index: usize, proof: &[U256]) -> TreeProof {
        let proof_vec: Vec<Branch> = proof
            .iter()
            .enumerate()
            .map(|(i, node)| {
                if Self::is_left_node_at_depth(index, i) {
                    Branch::Left((*node).into())
                } else {
                    Branch::Right((*node).into())
                }
            })
            .collect();
        TreeProof { 0: proof_vec }
    }

    /// Computes whether the node at a given index is a left child or right
    /// child.
    ///
    /// As the underlying tree is a binary tree, the corresponding bit to the
    /// depth will tell us the direction. A 0 bit signifies a left child, while
    /// a 1 bit signifies the right.
    fn is_left_node_at_depth(index: usize, depth: usize) -> bool {
        index & (1 << depth) == 0
    }

    /// Calculates the input hash based on the `input` parameters to the prover.
    ///
    /// We keccak hash all input to save verification gas. Inputs are arranged
    /// as follows:
    /// ```
    /// StartIndex || PreRoot || PostRoot || IdComms[0] || IdComms[1] || ... || IdComms[batchSize-1]
    ///     32     ||   256   ||   256    ||    256     ||    256     || ... ||     256 bits
    /// ```
    fn calculate_identity_registration_input_hash(input: &InsertionProofInput) -> U256 {
        // Calculate the input hash as described by the prover.
        let mut hashable_bytes: Vec<u8> = vec![];
        let mut buffer: [u8; size_of::<U256>()] = Default::default();
        hashable_bytes.extend(input.start_index.to_be_bytes());
        input.pre_root.to_big_endian(&mut buffer);
        hashable_bytes.extend(buffer);
        input.post_root.to_big_endian(&mut buffer);
        hashable_bytes.extend(buffer);

        input.identity_commitments.iter().for_each(|id| {
            id.to_big_endian(&mut buffer);
            hashable_bytes.extend(buffer);
        });

        keccak256(hashable_bytes).into()
    }

    /// Calculates the input hash based on the `input` parameters to the prover.
    ///
    /// We keccak hash all input to save verification gas. Inputs are arranged
    /// as follows:
    /// ```
    /// PackedDeletionIndices || PreRoot || PostRoot
    ///   32 bits * batchSize ||   256   ||    256
    /// ```
    pub fn compute_deletion_proof_input_hash(
        pre_root: U256,
        identity_commitments: &[U256],
        post_root: U256,
    ) -> U256 {
        let mut pre_root_bytes: [u8; size_of::<U256>()] = Default::default();
        pre_root.to_big_endian(pre_root_bytes.as_mut_slice());
        let mut post_root_bytes: [u8; size_of::<U256>()] = Default::default();
        post_root.to_big_endian(post_root_bytes.as_mut_slice());

        let mut bytes: Vec<u8> = vec![];
        bytes.extend(pre_root_bytes.iter());
        bytes.extend(post_root_bytes.iter());

        for commitment in identity_commitments.iter() {
            let mut commitment_bytes: [u8; size_of::<U256>()] = Default::default();
            commitment.to_big_endian(commitment_bytes.as_mut_slice());
            bytes.extend(commitment_bytes.iter());
        }

        keccak256(bytes).into()
    }
}
