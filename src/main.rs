#![doc = include_str!("../Readme.md")]
#![warn(clippy::all, clippy::pedantic, clippy::cargo, clippy::nursery)]

use cli_batteries::version;

fn main() {
    cli_batteries::run(version!(), signup_sequencer::main);
}
