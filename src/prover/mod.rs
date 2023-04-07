//! This module contains exports for generic utilities for dealing with provers.
//!
//! These include utilities for interacting with the currently extant batch
//! insert proving service, as well as common types that will later be used with
//! the batch update proving service once that arrives.
//!
//! APIs are designed to be imported for use qualified (e.g.
//! `batch_insertion::Prover`, `batch_insertion::ProverMap` and so on).

pub mod batch_insertion;
pub mod proof;
