
use clap::Parser;
use ethers::types::H160;

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
   /// OpenZeppelin Defender API Key
   #[clap(long, env)]
   pub oz_api_key: String,

   /// OpenZeppelin Defender API Secret
   #[clap(long, env)]
   pub oz_api_secret: String,

   /// OpenZeppelin Defender API Secret
   #[clap(long, env, default_value="0x30dcc24131223d4f8af69226e7b11b83e6a68b8b")]
   pub oz_address: H160, 
}

#[derive(Clone, Debug)]
pub struct Provider {
    inner:        Arc<InnerProvider>,
    address:      Address,
    legacy:       bool,
    send_timeout: Duration,
    mine_timeout: Duration,
}
