//use ethers::providers::Middleware;
//use ethers::types::{Address, BlockNumber, Filter, FilterBlockOption, Log, Topic, ValueOrArray};
use alloy::primitives::{Address, BlockNumber};
use alloy::providers::Provider;
use alloy::rpc::types::{Filter, FilterBlockOption, FilterSet, Log, Topic, ValueOrArray};

pub struct BlockScanner<T> {
    read_provider: T,
    current_block: u64,
    window_size: u64,

    // How many blocks from the chain head to scan to
    // e.g. if latest block is 20 and offset is set to 3
    // then the scanner will scan until block 17
    chain_head_offset: u64,
}

impl<T> BlockScanner<T>
where
    T: Provider,
{
    pub async fn new_latest(read_provider: T, window_size: u64) -> anyhow::Result<Self> {
        let latest_block = read_provider.get_block_number().await?;

        Ok(Self {
            read_provider,
            current_block: latest_block,
            window_size,
            chain_head_offset: 0,
        })
    }

    pub fn with_offset(mut self, chain_head_offset: u64) -> Self {
        self.chain_head_offset = chain_head_offset;
        self
    }

    pub async fn next(
        &mut self,
        address: Option<ValueOrArray<Address>>,
        topics: [Topic; 4],
    ) -> anyhow::Result<Vec<Log>> {
        let latest_block = self.read_provider.get_block_number().await?;
        let latest_block = latest_block.saturating_sub(self.chain_head_offset);

        if self.current_block >= latest_block {
            return Ok(Vec::new());
        }

        let from_block = self.current_block;
        let to_block = latest_block.min(from_block + self.window_size);

        let next_current_block = to_block + 1;

        let mut filter = Filter::new().select(from_block..=to_block);
        if let Some(address) = address {
            filter = filter.address(address);
        }
        filter.topics = topics;

        let logs = self.read_provider.get_logs(&filter).await?;

        self.current_block = next_current_block;

        Ok(logs)
    }
}
