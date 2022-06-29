#![doc = include_str!("../Readme.md")]
#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

use signup_sequencer::main as app;

#[tokio::main]
async fn main() -> eyre::Result<()>{
    app().await
}
