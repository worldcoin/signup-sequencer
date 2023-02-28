use axum::{routing::post, Json, Router};
use axum_server::Handle;
use ethers::types::U256;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter},
    net::SocketAddr,
};

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

pub struct Service {
    server: Handle,
}

#[derive(Serialize, Deserialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
enum ProveResponse {
    ProofSuccess(Proof),
    ProofFailure(ProverError),
}

impl Service {
    pub async fn new(url: String) -> anyhow::Result<Self> {
        // Hitting this endpoint will always succeed, which is fine for what we want to
        // do in tests. For now at least.
        let prove = |Json(_): Json<ProofInput>| async move {
            let proof_output = Proof::from([
                "0x12bba8b5a46139c819d83544f024828ece34f4f46be933a377a07c1904e96ec4".into(),
                "0x112c8d7c63b6c431cef23e9c0d9ffff39d1d660f514030d4f2787960b437a1d5".into(),
                "0x2413396a2af3add6fbe8137cfe7657917e31a5cdab0b7d1d645bd5eeb47ba601".into(),
                "0x1ad029539528b32ba70964ce43dbf9bba2501cdb3aaa04e4d58982e2f6c34752".into(),
                "0x5bb975296032b135458bd49f92d5e9d363367804440d4692708de92e887cf17".into(),
                "0x14932600f53a1ceb11d79a7bdd9688a2f8d1919176f257f132587b2b3274c41e".into(),
                "0x13d7b19c7b67bf5d3adf2ac2d3885fd5d49435b6069c0656939cd1fb7bef9dc9".into(),
                "0x142e14f90c49c79b4edf5f6b7acbcdb0b0f376a4311fc036f1006679bd53ca9e".into(),
            ]);
            Json(ProveResponse::ProofSuccess(proof_output))
        };
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
                .unwrap();
        });

        let service = Self { server };
        Ok(service)
    }

    pub fn stop(self) {
        self.server.shutdown();
    }
}
