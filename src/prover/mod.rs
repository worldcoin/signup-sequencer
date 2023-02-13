pub mod batch_insertion;
pub mod proof;

use clap::Parser;

#[derive(Clone, Debug, PartialEq, Eq, Parser)]
#[group(skip)]
pub struct Options {
    #[clap(flatten)]
    /// The options for configuring the batch insertion prover service.
    pub batch_insertion: batch_insertion::Options,
}
