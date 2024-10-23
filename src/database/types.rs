use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::encode::IsNull;
use sqlx::error::BoxDynError;
use sqlx::prelude::FromRow;
use sqlx::{Database, Decode, Encode, Postgres, Type};

use crate::{identity_tree::Hash, utils::batch_type::BatchType};
use crate::prover::identity::Identity;

#[derive(Hash, PartialEq, Eq)]
pub struct DeletionEntry {
    pub leaf_index: usize,
    pub commitment: Hash,
}

#[derive(Debug, Clone, FromRow)]
pub struct BatchEntry {
    pub id: i64,
    pub next_root: Hash,
    // In general prev_root is present all the time except the first row (head of the batches
    // chain)
    pub prev_root: Option<Hash>,
    pub created_at: DateTime<Utc>,
    pub batch_type: BatchType,
    pub data: sqlx::types::Json<BatchEntryData>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchEntryData {
    pub identities: Vec<Identity>,
    pub indexes: Vec<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commitments(pub Vec<Hash>);

impl Encode<'_, Postgres> for Commitments {
    fn encode_by_ref(
        &self,
        buf: &mut <Postgres as Database>::ArgumentBuffer<'_>,
    ) -> Result<IsNull, BoxDynError> {
        let commitments = &self
            .0
            .iter()
            .map(|c| c.to_be_bytes()) // Why be not le?
            .collect::<Vec<[u8; 32]>>();

        <&Vec<[u8; 32]> as Encode<Postgres>>::encode(commitments, buf)
    }
}

impl Decode<'_, Postgres> for Commitments {
    fn decode(value: <Postgres as Database>::ValueRef<'_>) -> Result<Self, BoxDynError> {
        let value = <Vec<[u8; 32]> as Decode<Postgres>>::decode(value)?;

        let res = value.iter().map(|&v| Hash::from_be_bytes(v)).collect();

        Ok(Commitments(res))
    }
}

impl Type<Postgres> for Commitments {
    fn type_info() -> <Postgres as sqlx::Database>::TypeInfo {
        <&Vec<&[u8]> as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &<Postgres as sqlx::Database>::TypeInfo) -> bool {
        <&Vec<&[u8]> as Type<Postgres>>::compatible(ty)
    }
}

impl From<Vec<Hash>> for Commitments {
    fn from(value: Vec<Hash>) -> Self {
        Commitments(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafIndexes(pub Vec<usize>);

impl Encode<'_, Postgres> for LeafIndexes {
    fn encode_by_ref(
        &self,
        buf: &mut <Postgres as Database>::ArgumentBuffer<'_>,
    ) -> Result<IsNull, BoxDynError> {
        let commitments = &self
            .0
            .iter()
            .map(|&c| c as i64) // Why be not le?
            .collect();

        <&Vec<i64> as Encode<Postgres>>::encode(commitments, buf)
    }
}

impl Decode<'_, Postgres> for LeafIndexes {
    fn decode(value: <Postgres as Database>::ValueRef<'_>) -> Result<Self, BoxDynError> {
        let value = <Vec<i64> as Decode<Postgres>>::decode(value)?;

        let res = value.iter().map(|&v| v as usize).collect();

        Ok(LeafIndexes(res))
    }
}

impl Type<Postgres> for LeafIndexes {
    fn type_info() -> <Postgres as sqlx::Database>::TypeInfo {
        <&Vec<i64> as Type<Postgres>>::type_info()
    }

    fn compatible(ty: &<Postgres as sqlx::Database>::TypeInfo) -> bool {
        <&Vec<i64> as Type<Postgres>>::compatible(ty)
    }
}

impl From<Vec<usize>> for LeafIndexes {
    fn from(value: Vec<usize>) -> Self {
        LeafIndexes(value)
    }
}
