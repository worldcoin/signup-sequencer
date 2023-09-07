use std::time::Duration;

use ethers::providers::Middleware;
use ethers::types::{Address, BlockNumber, Filter, FilterBlockOption, Log, Topic, ValueOrArray};

use crate::ethereum::ReadProvider;

pub struct BlockScanner {
    read_provider:      ReadProvider,
    current_block:      u64,
    window_size:        u64,
    time_between_scans: Duration,
}

impl BlockScanner {
    pub async fn next(
        &mut self,
        address: Option<ValueOrArray<Address>>,
        topics: [Option<Topic>; 4],
    ) -> anyhow::Result<Vec<Log>> {
        let latest_block = self.read_provider.get_block_number().await?;

        let latest_block = latest_block.as_u64();

        let from_block = self.current_block;
        let to_block = latest_block.min(from_block + self.window_size);

        let next_current_block = to_block + 1;

        let from_block = Some(BlockNumber::Number(from_block.into()));
        let to_block = Some(BlockNumber::Number(to_block.into()));

        let logs = self
            .read_provider
            .get_logs(&Filter {
                block_option: FilterBlockOption::Range {
                    from_block,
                    to_block,
                },
                address,
                topics,
            })
            .await?;

        self.current_block = next_current_block;

        Ok(logs)
    }
}
