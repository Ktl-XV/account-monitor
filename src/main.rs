use ethers::{
    core::{
        abi::AbiDecode,
        types::{Address, BlockNumber, Filter as LogFilter, Log, TransactionReceipt, H256, U256},
    },
    middleware::Middleware,
    providers::{Http, Provider, ProviderError},
};
use eyre::Result;
use log::{debug, error, info, warn};
use serde_derive::{Deserialize as DeserializeMacro, Serialize as SerializeMacro};
use std::collections::HashMap;
use std::env;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::{
    signal::unix::{signal, SignalKind},
    time::sleep,
};
use warp::Filter;

use serde::Serialize;

mod chain;
mod interesting_transaction;
mod notification;
mod token;
use account_monitor::FullString;
use chain::{Chain, ChainMode, EnvInitializable};
use interesting_transaction::{
    BuildNotification, InterestingTransaction, InterestingTransactionKind,
};
use notification::{Notification, Sendable};

#[derive(DeserializeMacro, SerializeMacro, Debug)]
struct WatchedAccount {
    address: String,
    label: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    env_logger::init();

    let addressbook = Arc::new(Mutex::new(HashMap::new()));

    let add_monitor_account = warp::post()
        .and(warp::path("accounts"))
        .and(warp::body::content_length_limit(1024 * 16))
        .and(warp::body::json())
        .map({
            let acc = addressbook.clone();
            move |account: WatchedAccount| {
                let test = Address::from_str(&account.address[..]);

                if test.is_err() {
                    warp::reply::with_status(
                        "Invalid account address".to_string(),
                        warp::http::StatusCode::UNPROCESSABLE_ENTITY,
                    )
                } else {
                    acc.lock()
                        .unwrap()
                        .insert(account.address.to_lowercase(), account.label);

                    let watched_accounts_count = acc.lock().unwrap().len();
                    info!("Watched Accounts: {}", watched_accounts_count);

                    warp::reply::with_status(
                        format!("Watching {} accounts\n", watched_accounts_count),
                        warp::http::StatusCode::ACCEPTED,
                    )
                }
            }
        });

    tokio::spawn(async move {
        warp::serve(add_monitor_account)
            .run(([0, 0, 0, 0], 3030))
            .await;
    });

    Notification {
        message: "Account Monitor Started, no accounts configured".to_string(),
        url: None,
    }
    .send()
    .await?;

    let chains = Chain::init_from_env_vec();

    for chain in chains.into_iter() {
        match chain.mode {
            ChainMode::Blocks => {
                tokio::spawn(monitor_chain_blocks(chain.clone(), addressbook.clone()));
            }
            ChainMode::Events => {
                tokio::spawn(monitor_chain_events(chain.clone(), addressbook.clone()));
            }
        }
    }

    let mut sigint = signal(SignalKind::interrupt()).unwrap();
    let mut sigterm = signal(SignalKind::terminate()).unwrap();
    tokio::select! {
        _ = sigint.recv() => info!("SIGINT"),
        _ = sigterm.recv() => info!("SIGTERM")
    }

    Ok(())
}

#[derive(SerializeMacro, Debug)]
#[serde(rename_all = "camelCase")]
struct AlchemyBlockReceiptsParam {
    block_number: BlockNumber,
}

#[derive(SerializeMacro, DeserializeMacro, Debug)]
struct AlchemyBlockReceipts {
    receipts: Vec<TransactionReceipt>,
}

async fn alchemy_get_block_receipts<T: Into<BlockNumber> + Send + Sync + Serialize>(
    provider: Provider<ethers_providers::Http>,
    block: T,
) -> Result<AlchemyBlockReceipts, ProviderError> {
    let param = AlchemyBlockReceiptsParam {
        block_number: block.into(),
    };

    provider
        .request("alchemy_getTransactionReceipts", [param])
        .await
}

async fn flexible_get_block_receipts<T: Into<BlockNumber> + Send + Sync + Serialize>(
    provider: Provider<ethers_providers::Http>,
    block: T,
) -> Result<Vec<TransactionReceipt>, ProviderError> {
    let is_provider_alchemy = provider
        .url()
        .host_str()
        .unwrap_or("not")
        .contains("alchemy.com");

    match is_provider_alchemy {
        true => {
            let wrapped_result = alchemy_get_block_receipts(provider, block).await;
            match wrapped_result {
                Ok(res) => Ok(res.receipts),
                Err(err) => Err(err),
            }
        }
        false => provider.get_block_receipts(block).await,
    }
}

async fn monitor_chain_blocks(chain: Chain, addressbook: Arc<Mutex<HashMap<String, String>>>) {
    let provider = connect_and_verify(chain.clone()).await;

    let debug_block_var = env::var("DEBUG_BLOCK");
    if debug_block_var.is_ok() {
        warn!("Running in debug mode, getting single block");
        let debug_block_number = debug_block_var
            .unwrap()
            .parse::<i32>()
            .expect("Invalid DEBUG_BLOCK");

        let block = flexible_get_block_receipts(provider, debug_block_number)
            .await
            .unwrap();

        loop {
            let now = Instant::now();
            let notification = process_block(&block, &chain, addressbook.clone());

            if notification.is_some() {
                notification.unwrap().send().await.unwrap();
                info!("Notification sent, exiting");
                std::process::exit(0)
            }

            warn!("No transaction by monitored accounts found, have the accounts been setup?");

            let elapsed_time = now.elapsed();
            let sleep_time = chain.blocktime - elapsed_time;
            debug!("Sleeping for: {} ms", sleep_time.as_millis());
            sleep(sleep_time).await;
        }
    };

    info!("Starting Account Watcher for {} in Blocks Mode", chain.name);

    let mut next_block_number = provider.get_block_number().await.unwrap();

    loop {
        let now = Instant::now();
        let block_number = match provider.get_block_number().await {
            Ok(res) => res,
            Err(_) => {
                error!(
                    "Error while getting {} block number from RPC, retrying",
                    chain.name
                );
                continue;
            }
        };

        info!("Current block number on {}: {}", chain.name, block_number);

        while next_block_number <= block_number {
            debug!("Processing {} block {}", chain.name, next_block_number);
            let block_response =
                flexible_get_block_receipts(provider.clone(), next_block_number).await;

            let block = match block_response {
                Ok(res) => res,
                Err(_) => {
                    error!(
                        "Error while getting {} block receipts from RPC, retrying",
                        chain.name
                    );
                    continue;
                }
            };

            let notification = process_block(&block, &chain, addressbook.clone());

            if notification.is_some() {
                let sent_notification = notification.unwrap().send().await;
                if sent_notification.is_err() {
                    error!("Error while sending notification, retrying");
                    continue;
                }
            }
            next_block_number = next_block_number + 1
        }

        let elapsed_time = now.elapsed();

        if elapsed_time < chain.blocktime {
            let sleep_time = chain.blocktime - elapsed_time;
            debug!("Sleeping {} for: {} ms", chain.name, sleep_time.as_millis());
            sleep(sleep_time).await;
        }
    }
}

async fn monitor_chain_events(chain: Chain, addressbook: Arc<Mutex<HashMap<String, String>>>) {
    let provider = connect_and_verify(chain.clone()).await;

    let debug_block_var = env::var("DEBUG_BLOCK");
    if debug_block_var.is_ok() {
        warn!("Running in debug mode, getting single block");
        let debug_block_number = debug_block_var
            .unwrap()
            .parse::<u64>()
            .expect("Invalid DEBUG_BLOCK");

        let events = provider
            .get_logs(&LogFilter::new().select(debug_block_number))
            .await
            .unwrap();

        loop {
            let now = Instant::now();
            let notification = process_block_events(&events, &chain, addressbook.clone());

            if notification.is_some() {
                notification.unwrap().send().await.unwrap();
                info!("Notification sent, exiting");
                std::process::exit(0)
            }

            warn!("No transaction by monitored accounts found, have the accounts been setup?");

            let elapsed_time = now.elapsed();
            let sleep_time = chain.blocktime - elapsed_time;
            debug!("Sleeping for: {} ms", sleep_time.as_millis());
            sleep(sleep_time).await;
        }
    };

    info!("Starting Account Watcher for {} Event Mode", chain.name);

    let mut next_block_number = provider.get_block_number().await.unwrap();

    loop {
        let now = Instant::now();
        let block_number = match provider.get_block_number().await {
            Ok(res) => res,
            Err(_) => {
                error!(
                    "Error while getting {} block number from RPC, retrying",
                    chain.name
                );
                continue;
            }
        };

        let block_number_with_delay = block_number - 1;

        info!("Current block number on {}: {}", chain.name, block_number);

        if next_block_number <= block_number_with_delay {
            debug!(
                "Processing {} from block {} to block {}",
                chain.name, next_block_number, block_number_with_delay
            );
            let events = match provider
                .get_logs(
                    &LogFilter::new()
                        .from_block(next_block_number)
                        .to_block(block_number_with_delay),
                )
                .await
            {
                Ok(events) => events,
                Err(_) => {
                    error!(
                        "Error while getting {} events from RPC, retrying",
                        chain.name
                    );
                    continue;
                }
            };

            let notification = process_block_events(&events, &chain, addressbook.clone());

            if notification.is_some() {
                let sent_notification = notification.unwrap().send().await;
                if sent_notification.is_err() {
                    error!("Error while sending notification, retrying");
                    continue;
                }
            }
            next_block_number = block_number_with_delay + 1;
        }

        let elapsed_time = now.elapsed();

        if elapsed_time < chain.blocktime {
            let sleep_time = chain.blocktime - elapsed_time;
            debug!("Sleeping {} for: {} ms", chain.name, sleep_time.as_millis());
            sleep(sleep_time).await;
        }
    }
}

fn parse_logs(
    logs: Vec<Log>,
    addressbook: HashMap<String, String>,
) -> Option<InterestingTransaction> {
    let watched_addresses_as_topics: Vec<H256> = addressbook
        .keys()
        .map(|addr| H256::from(Address::from_str(addr).unwrap()))
        .collect();

    for log in logs.iter() {
        for topic in log.topics.iter() {
            if watched_addresses_as_topics.contains(topic) {
                if log.topics[0]
                    == H256::from_str(
                        "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
                        // TRANSFER_TOPIC
                    )
                    .unwrap()
                {
                    return Some(InterestingTransaction {
                        hash: log.transaction_hash.unwrap(),
                        from: Some(Address::from(log.topics[1])),
                        to: Some(Address::from(log.topics[2])),
                        kind: InterestingTransactionKind::Transfer,
                        amount: U256::decode(&log.data).unwrap_or(U256::from("0")),
                        token: Some(log.address),
                        involved_account: None,
                    });
                } else {
                    return Some(InterestingTransaction {
                        hash: log.transaction_hash.unwrap(),
                        involved_account: watched_addresses_as_topics
                            .iter()
                            .filter_map(|account_as_topic| {
                                if account_as_topic == topic {
                                    Some(
                                        Address::from_str(
                                            &account_as_topic.full_string().unwrap()[26..],
                                        )
                                        .unwrap(),
                                    )
                                } else {
                                    None
                                }
                            })
                            .next(),

                        from: None,
                        to: None,
                        kind: InterestingTransactionKind::Other,
                        amount: U256::from("0"),
                        token: None,
                    });
                }
            }
        }
    }
    None
}

fn process_block(
    block: &[TransactionReceipt],
    chain: &Chain,
    addressbook_mutex: Arc<Mutex<HashMap<String, String>>>,
) -> Option<Notification> {
    for receipt in block.iter() {
        let mut tx: Option<InterestingTransaction> = None;

        let addressbook = addressbook_mutex.lock().unwrap();

        if addressbook.contains_key(&receipt.from.full_string().unwrap())
            || (receipt.to.is_some()
                && addressbook.contains_key(&receipt.to.unwrap().full_string().unwrap()))
        {
            tx = Some(InterestingTransaction {
                hash: receipt.transaction_hash,
                from: Some(receipt.from),
                to: receipt.to,
                kind: if receipt.gas_used.unwrap() == U256::from_dec_str("21000").unwrap() {
                    InterestingTransactionKind::Send
                } else {
                    InterestingTransactionKind::Other
                },
                amount: U256::from_dec_str("0").unwrap(),
                token: None,
                involved_account: None,
            });
        }

        let interesting_transaction_from_logs =
            parse_logs(receipt.logs.clone(), addressbook.clone());

        if interesting_transaction_from_logs.is_some() {
            tx = interesting_transaction_from_logs;
        }

        if let Some(tx) = tx {
            return Some(tx.build_notification(chain, addressbook.clone()));
        }
    }
    None
}

fn process_block_events(
    logs: &[Log],
    chain: &Chain,
    addressbook_mutex: Arc<Mutex<HashMap<String, String>>>,
) -> Option<Notification> {
    let addressbook = addressbook_mutex.lock().unwrap();

    let tx = parse_logs(Vec::from(logs), addressbook.clone());

    if let Some(tx) = tx {
        return Some(tx.build_notification(chain, addressbook.clone()));
    }

    None
}

pub async fn connect_and_verify(chain: Chain) -> Provider<Http> {
    let provider =
        Provider::<Http>::try_from(chain.rpc.clone()).expect("could not instantiate HTTP Provider");

    let chainid = provider.get_chainid().await.unwrap();

    if chainid != chain.id {
        panic!(
            "Configured for {} ({}) but connected to {}",
            chain.name, chain.id, chainid
        );
    }
    provider
}
