#![doc = include_str!("../Readme.md")]
#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

use cli_batteries::{run, version};
use signup_sequencer::{main as sequencer_app, Options};

async fn app(options: Options) -> eyre::Result<()> {
    sequencer_app(options)
        .await
        .map_err(|e| eyre::eyre!("{:?}", e))
}

fn main() {
    println!("Hello, world!");
    run(version!(semaphore, ethers), app);
}
