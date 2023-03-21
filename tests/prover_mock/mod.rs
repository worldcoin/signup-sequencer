use std::{
    fmt::{Display, Formatter},
    mem::size_of,
    net::SocketAddr,
};

use axum::{routing::post, Json, Router};
use axum_server::Handle;
use ethers::{types::U256, utils::keccak256};
use semaphore::poseidon_tree::{Branch, Proof as TreeProof};
use serde::{Deserialize, Serialize};

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
struct ProofInput {
    input_hash:           U256,
    start_index:          u32,
    pre_root:             U256,
    post_root:            U256,
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
pub struct Service {
    server: Handle,
}

impl Service {
    /// Returns a new instance of the mock prover service, serving at the
    /// provided `url`.
    ///
    /// It provides only a single endpoint for now, `/prove` in order to match
    /// the full service (`semaphore-mtb`). This can be extended in the future
    /// if needed.
    pub async fn new(url: String) -> anyhow::Result<Self> {
        let prove = |Json(input): Json<ProofInput>| async move { Json(Self::prove(input)) };
        let app = Router::new().route("/prove", post(prove));

        let addr: SocketAddr = url.parse()?;
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

        let service = Self { server };
        Ok(service)
    }

    /// Shuts down the server and frees up the socket that it was using.
    pub fn stop(self) {
        self.server.shutdown();
    }

    /// Performs the 'proof' operation on the provided `input`.
    ///
    /// Note that this does _not_ implement the full ZK proof system as done by
    /// `semaphore-mtb`. Instead, it just verifies that the provided merkle
    /// proofs are correct, which is sufficient when combined with the mock
    /// verifier used by the mock chain.
    ///
    /// In order to save effort and reduce the surface for bugs, the proof
    /// verification logic from `semaphore-rs` is reused.
    fn prove(input: ProofInput) -> ProveResponse {
        // Calculate the input hash based on the prover parameters.
        let input_hash = Self::calculate_identity_registration_input_hash(&input);

        // If the hashes aren't the same something's wrong so we return an error.
        if input_hash != input.input_hash {
            return ProveResponse::failure("42", "Input hash mismatch.");
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
            return ProveResponse::failure("43", "Merkle proof verification failure.");
        }

        // If we succeed in verifying, the output should be correlated with the input,
        // so we use the input_hash as part of it.
        ProveResponse::success([
            "0x2".into(),
            input_hash,
            "0x2413396a2af3add6fbe8137cfe7657917e31a5cdab0b7d1d645bd5eeb47ba601".into(),
            "0x1ad029539528b32ba70964ce43dbf9bba2501cdb3aaa04e4d58982e2f6c34752".into(),
            "0x5bb975296032b135458bd49f92d5e9d363367804440d4692708de92e887cf17".into(),
            "0x14932600f53a1ceb11d79a7bdd9688a2f8d1919176f257f132587b2b3274c41e".into(),
            "0x13d7b19c7b67bf5d3adf2ac2d3885fd5d49435b6069c0656939cd1fb7bef9dc9".into(),
            "0x142e14f90c49c79b4edf5f6b7acbcdb0b0f376a4311fc036f1006679bd53ca9e".into(),
        ])
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
    fn calculate_identity_registration_input_hash(input: &ProofInput) -> U256 {
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
}
