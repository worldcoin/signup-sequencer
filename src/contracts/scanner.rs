use alloy::primitives::Address;
use alloy::providers::Provider;
use alloy::rpc::types::{
    BlockNumberOrTag as BlockNumber, Filter, FilterBlockOption, FilterSet, Log, Topic,
};

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
        address: Option<FilterSet<Address>>,
        topics: [Option<Topic>; 4],
    ) -> anyhow::Result<Vec<Log>> {
        let latest_block = self.read_provider.get_block_number().await?;
        let latest_block = latest_block.saturating_sub(self.chain_head_offset);

        if self.current_block >= latest_block {
            return Ok(Vec::new());
        }

        let from_block = self.current_block;
        let to_block = latest_block.min(from_block + self.window_size);

        let next_current_block = to_block + 1;

        let from_block = Some(BlockNumber::Number(from_block));
        let to_block = Some(BlockNumber::Number(to_block));

        let logs = self
            .read_provider
            .get_logs(&Filter {
                block_option: FilterBlockOption::Range {
                    from_block,
                    to_block,
                },
                address: address.unwrap_or_default(),
                topics: topics.map(Option::unwrap_or_default),
            })
            .await?;

        self.current_block = next_current_block;

        Ok(logs)
    }
}
