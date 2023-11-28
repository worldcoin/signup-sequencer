#![doc = include_str!("../Readme.md")]
#![warn(clippy::cargo)]
#![allow(clippy::module_name_repetitions, clippy::wildcard_imports)]

use cli_batteries::{run, version};
use signup_sequencer::{main as sequencer_app, Options};

async fn app(options: Options) -> eyre::Result<()> {
    sequencer_app(options)
        .await
        .map_err(|e| eyre::eyre!("{:?}", e))
}

fn main() {
    run(version!(semaphore, ethers), app);
}
