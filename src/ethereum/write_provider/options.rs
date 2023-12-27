use std::num::ParseIntError;
use std::str::FromStr;
use std::time::Duration;

use anyhow::anyhow;
use clap::Parser;
use ethers::types::H160;

// TODO: Log and metrics for signer / nonces.
#[derive(Clone, Debug, Eq, PartialEq, Parser)]
#[group(skip)]
pub struct Options {
    // ### OZ Params ###
    #[clap(long, env, default_value = "https://api.defender.openzeppelin.com")]
    pub oz_api_url: Option<String>,

    /// OpenZeppelin Defender API Key
    #[clap(long, env)]
    pub oz_api_key: Option<String>,

    /// OpenZeppelin Defender API Secret
    #[clap(long, env)]
    pub oz_api_secret: Option<String>,

    /// OpenZeppelin Defender API Secret
    #[clap(long, env)]
    pub oz_address: Option<H160>,

    /// For how long OpenZeppelin should track and retry the transaction (in
    /// seconds) Default: 7 days (7 * 24 * 60 * 60 = 604800 seconds)
    #[clap(long, env, value_parser=duration_from_str, default_value="604800")]
    pub oz_transaction_validity: Duration,

    #[clap(long, env, value_parser=duration_from_str, default_value="60")]
    pub oz_send_timeout: Duration,

    #[clap(long, env, value_parser=duration_from_str, default_value="60")]
    pub oz_mine_timeout: Duration,

    #[clap(long, env)]
    pub oz_gas_limit: Option<u64>,

    // ### TxSitter Params ###
    #[clap(long, env)]
    pub tx_sitter_url: Option<String>,

    #[clap(long, env)]
    pub tx_sitter_address: Option<H160>,

    #[clap(long, env)]
    pub tx_sitter_gas_limit: Option<u64>,
}

fn duration_from_str(value: &str) -> Result<Duration, ParseIntError> {
    Ok(Duration::from_secs(u64::from_str(value)?))
}

impl Options {
    pub fn to_parsed(&self) -> anyhow::Result<ParsedOptions> {
        let oz_options = OzOptions::try_from(self);
        if let Ok(oz_options) = oz_options {
            return Ok(ParsedOptions::Oz(oz_options));
        }

        let tx_sitter_options = TxSitterOptions::try_from(self);
        if let Ok(tx_sitter_options) = tx_sitter_options {
            return Ok(ParsedOptions::TxSitter(tx_sitter_options));
        }

        Err(anyhow!("Invalid options"))
    }
}

pub enum ParsedOptions {
    Oz(OzOptions),
    TxSitter(TxSitterOptions),
}

impl ParsedOptions {
    pub fn address(&self) -> H160 {
        match self {
            Self::Oz(oz_options) => oz_options.oz_address,
            Self::TxSitter(tx_sitter_options) => tx_sitter_options.tx_sitter_address,
        }
    }
}

pub struct OzOptions {
    pub oz_api_url: String,

    /// OpenZeppelin Defender API Key
    pub oz_api_key: String,

    /// OpenZeppelin Defender API Secret
    pub oz_api_secret: String,

    /// OpenZeppelin Defender API Secret
    pub oz_address: H160,

    /// For how long OpenZeppelin should track and retry the transaction (in
    /// seconds) Default: 7 days (7 * 24 * 60 * 60 = 604800 seconds)
    pub oz_transaction_validity: Duration,

    pub oz_send_timeout: Duration,

    pub oz_mine_timeout: Duration,

    pub oz_gas_limit: Option<u64>,
}

impl<'a> TryFrom<&'a Options> for OzOptions {
    type Error = anyhow::Error;

    fn try_from(value: &'a Options) -> Result<Self, Self::Error> {
        Ok(Self {
            oz_api_url:              value
                .oz_api_url
                .clone()
                .ok_or_else(|| anyhow!("Missing oz_api_url"))?,
            oz_api_key:              value
                .oz_api_key
                .clone()
                .ok_or_else(|| anyhow!("Missing oz_api_key"))?,
            oz_api_secret:           value
                .oz_api_secret
                .clone()
                .ok_or_else(|| anyhow!("Missing oz_api_secret"))?,
            oz_address:              value
                .oz_address
                .ok_or_else(|| anyhow!("Missing oz_address"))?,
            oz_transaction_validity: value.oz_transaction_validity,
            oz_send_timeout:         value.oz_send_timeout,
            oz_mine_timeout:         value.oz_mine_timeout,
            oz_gas_limit:            value.oz_gas_limit,
        })
    }
}

pub struct TxSitterOptions {
    pub tx_sitter_url:       String,
    pub tx_sitter_address:   H160,
    pub tx_sitter_gas_limit: Option<u64>,
}

impl<'a> TryFrom<&'a Options> for TxSitterOptions {
    type Error = anyhow::Error;

    fn try_from(value: &'a Options) -> Result<Self, Self::Error> {
        Ok(Self {
            tx_sitter_url:       value
                .tx_sitter_url
                .clone()
                .ok_or_else(|| anyhow!("Missing tx_sitter_url"))?,
            tx_sitter_address:   value
                .tx_sitter_address
                .ok_or_else(|| anyhow!("Missing tx_sitter_address"))?,
            tx_sitter_gas_limit: value.tx_sitter_gas_limit,
        })
    }
}
