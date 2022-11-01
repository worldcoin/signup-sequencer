use ethers::providers::{Provider, JsonRpcClient, ProviderError, Middleware};
use ethers::types::{Filter, U64, Log};
use eyre::ErrReport;
use futures::Stream;
use std::future::Future;
use std::sync::Arc;
use std::{
    collections::VecDeque,
    pin::Pin,
    task::{Context, Poll},
};
use thiserror::Error;

use crate::database::Database;

// Helper type alias
#[cfg(target_arch = "wasm32")]
pub(crate) type PinBoxFut<'a, T, Err> = Pin<Box<dyn Future<Output = Result<T, Err>> + 'a>>;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) type PinBoxFut<'a, T, Err> =
    Pin<Box<dyn Future<Output = Result<T, Err>> + Send + 'a>>;

pub struct CachingLogQuery<'a, P> {
    provider: &'a Provider<P>,
    filter: Filter,
    from_block: Option<U64>,
    page_size: u64,
    current_logs: VecDeque<Log>,
    last_eth_block: Option<U64>,
    last_db_block: Option<u64>,
    state: CachingLogQueryState<'a>,
    database: Option<Arc<Database>>,
}

enum CachingLogQueryState<'a> {
    Initial,
    LoadLastEthBlock(PinBoxFut<'a, U64, ProviderError>),
    LoadEthLogs(PinBoxFut<'a, Vec<Log>, ProviderError>),
    LoadLastDbBlock(PinBoxFut<'a, u64, ErrReport>),
    LoadDbLogs(PinBoxFut<'a, Vec<String>, ErrReport>),
    CacheEthLogs(PinBoxFut<'a, (), ErrReport>),
    ConsumeDb,
    ConsumeEth,
}

impl<'a, P> CachingLogQuery<'a, P>
where
    P: JsonRpcClient,
{
    pub fn new(provider: &'a Provider<P>, filter: &Filter) -> Self {
        Self {
            provider,
            filter: filter.clone(),
            from_block: filter.get_from_block(),
            page_size: 10000,
            current_logs: VecDeque::new(),
            last_eth_block: None,
            last_db_block: None,
            state: CachingLogQueryState::Initial,
            database: None,
        }
    }

    /// set page size for pagination
    pub fn with_page_size(mut self, page_size: u64) -> Self {
        self.page_size = page_size;
        self
    }

    pub fn with_database(mut self, database: Arc<Database>) -> Self {
        self.database = Some(database);
        self
    }
}

macro_rules! rewake_with_new_state {
    ($ctx:ident, $this:ident, $new_state:expr) => {
        $this.state = $new_state;
        $ctx.waker().wake_by_ref();
        return Poll::Pending
    };
}

#[derive(Error, Debug)]
pub enum CachingLogQueryError<E> {
    #[error(transparent)]
    LoadLastBlockError(E),
    #[error(transparent)]
    LoadLogsError(E),
}

type Item = Result<Log, CachingLogQueryError<ProviderError>>;

impl<'a, P> CachingLogQuery<'a, P>
where
    P: JsonRpcClient,
{
    fn fetch_page(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Item>> {
        // load new logs if there are still more pages to go through
        // can safely assume this will always be set in this state
        let from_block = self.from_block.unwrap();
        let to_block = from_block + self.page_size;

        // no more pages to load, and everything is consumed
        // can safely assume this will always be set in this state
        if from_block > self.last_eth_block.unwrap() {
            return Poll::Ready(None)
        }
        // load next page
        println!("loading eth blocks {}-{}", from_block, to_block);

        let filter = self.filter.clone().from_block(from_block).to_block(to_block);
        let provider = self.provider;
        let fut = Box::pin(async move { provider.get_logs(&filter).await });
        rewake_with_new_state!(ctx, self, CachingLogQueryState::LoadEthLogs(fut));
    }
}

impl<'a, P> Stream for CachingLogQuery<'a, P>
where
    P: JsonRpcClient,
{
    type Item = Result<Log, CachingLogQueryError<ProviderError>>;

    fn poll_next(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match &mut self.state {
            CachingLogQueryState::Initial => {
                if !self.filter.is_paginatable() {
                    // if not paginatable, load logs and consume
                    let filter = self.filter.clone();
                    let provider = self.provider;
                    let fut = Box::pin(async move { provider.get_logs(&filter).await });
                    rewake_with_new_state!(ctx, self, CachingLogQueryState::LoadEthLogs(fut));
                } else {
                    // if paginatable, load last block
                    let fut = self.provider.get_block_number();
                    rewake_with_new_state!(ctx, self, CachingLogQueryState::LoadLastEthBlock(fut));
                }
            }
            CachingLogQueryState::LoadLastDbBlock(fut) => {
                match futures_util::ready!(fut.as_mut().poll(ctx)) {
                    Ok(last_block) => {
                        self.last_db_block = Some(last_block);
                        println!("last db block: {}", last_block);

                        if let Some(database) = self.database.clone() {
                            let fut = Box::pin(async move { database.load_logs().await });
                            rewake_with_new_state!(ctx, self, CachingLogQueryState::LoadDbLogs(fut));
                        } else {
                            panic!("db required");
                        }
                    }
                    Err(err) => Poll::Ready(Some(Err(CachingLogQueryError::LoadLastBlockError(ProviderError::CustomError(err.to_string()))))),
                }
            }
            CachingLogQueryState::LoadDbLogs(fut) => {
                match futures_util::ready!(fut.as_mut().poll(ctx)) {
                    Ok(logs) => {
                        self.current_logs = VecDeque::from(vec![]);
                        rewake_with_new_state!(ctx, self, CachingLogQueryState::ConsumeDb);
                    }
                    Err(err) => Poll::Ready(Some(Err(CachingLogQueryError::LoadLogsError(ProviderError::CustomError(err.to_string()))))),
                }
            }
            CachingLogQueryState::LoadLastEthBlock(fut) => {
                match futures_util::ready!(fut.as_mut().poll(ctx)) {
                    Ok(last_block) => {
                        self.last_eth_block = Some(last_block);
                        println!("last eth block: {}", last_block);

                        if let Some(database) = self.database.clone() {
                            let fut = Box::pin(async move { database.get_block_number().await });
                            rewake_with_new_state!(ctx, self, CachingLogQueryState::LoadLastDbBlock(fut));
                        } else {
                            panic!("db required");
                        }
                    }
                    Err(err) => Poll::Ready(Some(Err(CachingLogQueryError::LoadLastBlockError(err)))),
                }
            }
            CachingLogQueryState::LoadEthLogs(fut) => {
                match futures_util::ready!(fut.as_mut().poll(ctx)) {
                    Ok(logs) => {
                        let db_copy = logs.iter().map(|_| "x".to_owned()).collect();
                        self.current_logs = VecDeque::from(logs);
                        if let Some(database) = self.database.clone() {
                            let from = self.from_block.unwrap().as_u64();
                            let to = from + self.page_size;
                            let fut = Box::pin(async move { database.save_logs(from, to, db_copy).await });
                            rewake_with_new_state!(ctx, self, CachingLogQueryState::CacheEthLogs(fut));
                        } else {
                            rewake_with_new_state!(ctx, self, CachingLogQueryState::ConsumeEth);
                        }

                    }
                    Err(err) => Poll::Ready(Some(Err(CachingLogQueryError::LoadLogsError(err)))),
                }
            },
            CachingLogQueryState::CacheEthLogs(fut) => {
                match futures_util::ready!(fut.as_mut().poll(ctx)) {
                    Ok(_) => {
                        rewake_with_new_state!(ctx, self, CachingLogQueryState::ConsumeEth);
                    }
                    Err(err) => Poll::Ready(Some(Err(CachingLogQueryError::LoadLastBlockError(ProviderError::CustomError(err.to_string()))))),
                }

            }
            CachingLogQueryState::ConsumeDb => {
                let log = self.current_logs.pop_front();
                if log.is_none() {
                    self.fetch_page(ctx)
                } else {
                    Poll::Ready(log.map(Ok))
                }
            }
            CachingLogQueryState::ConsumeEth => {
                let log = self.current_logs.pop_front();
                if log.is_none() {
                    // start working on the next page
                    self.from_block = Some(self.from_block.unwrap() + self.page_size + 1);
                    // consumed all the logs
                    if !self.filter.is_paginatable() {
                        Poll::Ready(None)
                    } else {
                        self.fetch_page(ctx)
                    }
                } else {
                    Poll::Ready(log.map(Ok))
                }
            }
        }
    }
}


/*                         // load new logs if there are still more pages to go through
                        // can safely assume this will always be set in this state
                        let from_block = self.from_block.unwrap();
                        let to_block = from_block + self.page_size;

                        // no more pages to load, and everything is consumed
                        // can safely assume this will always be set in this state
                        if from_block > self.last_eth_block.unwrap() {
                            return Poll::Ready(None)
                        }
                        // load next page
                        println!("loading eth blocks {}-{}", from_block, to_block);
                        self.from_block = Some(to_block + 1);

                        let filter = self.filter.clone().from_block(from_block).to_block(to_block);
                        let provider = self.provider;
                        let fut = Box::pin(async move { provider.get_logs(&filter).await });
                        rewake_with_new_state!(ctx, self, CachingLogQueryState::LoadEthLogs(fut));
 */