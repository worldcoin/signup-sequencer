use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use clap::Parser;
use ethers::prelude::k256::ecdsa::SigningKey;

#[derive(Debug, Clone, Parser)]
#[clap(rename_all = "kebab-case")]
struct Args {
    /// The port at which to serve
    ///
    /// Set to 0 to use a random port
    #[clap(short, long, env, default_value = "9876")]
    port:       u16,
    /// The RPC url to use
    ///
    /// Uses a default value compatible with anvil
    #[clap(short, long, env, default_value = "http://127.0.0.1:8545")]
    rpc_url:    String,
    /// A hex encoded private key
    ///
    /// By default uses a private key used by anvil
    #[clap(
        short,
        long,
        env,
        default_value = "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80"
    )]
    secret_key: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let private_key = args.secret_key.trim_start_matches("0x");
    let private_key = hex::decode(private_key)?;

    let signing_key = SigningKey::from_bytes(&private_key)?;

    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), args.port);

    let handle = micro_oz::spawn(addr, args.rpc_url, signing_key).await?;

    tracing::info!("Micro OZ listening on {}", handle.endpoint());

    handle.wait().await;

    Ok(())
}
