use ethers::{
    core::{
        abi::AbiDecode,
        types::{Address, BlockNumber, Filter as LogFilter, Log, TransactionReceipt, H256, U256},
    },
    middleware::Middleware,
    providers::{Http, Provider, ProviderError},
};
use eyre::Result;
use lazy_static::lazy_static;
use log::{debug, error, info, warn};
use prometheus::{IntGauge, IntGaugeVec, Opts as PrometheusOpts, Registry};
use serde::Serialize;
use serde_derive::{Deserialize as DeserializeMacro, Serialize as SerializeMacro};
use serde_yaml::{self};
use std::collections::HashMap;
use std::env;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use std::time::Instant;
use tokio::{
    signal::unix::{signal, SignalKind},
    time::sleep,
};
use warp::Filter;

mod chain;
mod interesting_transaction;
mod notification;
mod token;
use account_monitor::FullString;
use chain::{Chain, ChainMode, EnvInitializable};
use interesting_transaction::{
    BuildNotification, InterestingTransaction, InterestingTransactionKind, SpamFilter,
};
use notification::{Notification, Sendable};

const MAX_BLOCK_RANGE: u64 = 100;
const START_BACKOFF_RETRY_COUNT: i32 = 3;

#[derive(DeserializeMacro, SerializeMacro, Debug)]
struct WatchedAccount {
    address: String,
    label: String,
}

lazy_static! {
    pub static ref REGISTRY: Registry = Registry::new();
    pub static ref CURRENT_BLOCK: IntGaugeVec = IntGaugeVec::new(
        PrometheusOpts::new("current_block", "Current Block on each chain"),
        &["chain"]
    )
    .expect("metric can be created");
    pub static ref MONITORED_ACCOUNTS: IntGauge =
        IntGauge::new("monitored_accounts", "Count of monitored accounts")
            .expect("metric can be created");
}

fn register_custom_metrics() {
    REGISTRY
        .register(Box::new(CURRENT_BLOCK.clone()))
        .expect("collector can be registered");
    REGISTRY
        .register(Box::new(MONITORED_ACCOUNTS.clone()))
        .expect("collector can be registered");
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv::dotenv().ok();
    env_logger::init();

    register_custom_metrics();

    let addressbook = Arc::new(Mutex::new(HashMap::new()));

    let addrbook = addressbook.clone();

    let add_monitor_account = warp::post()
        .and(warp::path("accounts"))
        .and(warp::body::content_length_limit(1024 * 16))
        .and(warp::body::json())
        .map({
            move |account: WatchedAccount| {
                let test = Address::from_str(&account.address[..]);

                if test.is_err() {
                    warp::reply::with_status(
                        "Invalid account address".to_string(),
                        warp::http::StatusCode::UNPROCESSABLE_ENTITY,
                    )
                } else {
                    let watched_accounts_count = watch_account(addrbook.clone(), account);
                    info!("Watched Accounts: {}", watched_accounts_count);
                    MONITORED_ACCOUNTS.set(watched_accounts_count as i64);

                    warp::reply::with_status(
                        format!("Watching {} accounts\n", watched_accounts_count),
                        warp::http::StatusCode::ACCEPTED,
                    )
                }
            }
        });

    let metrics_route = warp::get().and(warp::path("metrics")).map(|| {
        use prometheus::Encoder;
        let encoder = prometheus::TextEncoder::new();

        let mut buffer = Vec::new();
        if let Err(e) = encoder.encode(&REGISTRY.gather(), &mut buffer) {
            error!("could not encode custom metrics: {}", e);
        };
        let res = match String::from_utf8(buffer.clone()) {
            Ok(v) => v,
            Err(e) => {
                error!("custom metrics could not be from_utf8'd: {}", e);
                String::default()
            }
        };
        buffer.clear();

        res
    });

    tokio::spawn(async move {
        warp::serve(metrics_route.or(add_monitor_account))
            .run(([0, 0, 0, 0], 3030))
            .await;
    });

    let mut watched_accounts_count: u32 = 0;
    let static_accounts_path_var = env::var("STATIC_ACCOUNTS_PATH");
    if static_accounts_path_var.is_ok() {
        let static_accounts_path = static_accounts_path_var.unwrap();

        let file =
            std::fs::File::open(static_accounts_path).expect("Could not open accounts file.");
        let accounts_to_add: Vec<WatchedAccount> =
            serde_yaml::from_reader(file).expect("Could not read accounts.");
        watched_accounts_count = accounts_to_add
            .into_iter()
            .map(|acc| watch_account(addressbook.clone(), acc))
            .max()
            .unwrap();
    }

    MONITORED_ACCOUNTS.set(watched_accounts_count as i64);
    Notification {
        message: format!(
            "Account Monitor Started, {} accounts configured",
            watched_accounts_count
        )
        .to_string(),
        url: None,
    }
    .send()
    .await?;

    let chains = Chain::init_from_env_vec();

    let debug_block_var = env::var("DEBUG_BLOCK");
    if debug_block_var.is_ok() {
        warn!("Running in debug mode, getting single block");
        let debug_block_number = debug_block_var
            .unwrap()
            .parse::<u64>()
            .expect("Invalid DEBUG_BLOCK");

        for chain in chains.into_iter() {
            match chain.mode {
                ChainMode::Blocks => {
                    tokio::spawn(debug_chain_blocks(
                        chain,
                        addressbook.clone(),
                        debug_block_number,
                    ));
                }
                ChainMode::Events => {
                    tokio::spawn(debug_chain_events(
                        chain,
                        addressbook.clone(),
                        debug_block_number,
                    ));
                }
            }
        }
    } else {
        for chain in chains.into_iter() {
            match chain.mode {
                ChainMode::Blocks => {
                    tokio::spawn(monitor_chain_blocks(chain, addressbook.clone()));
                }
                ChainMode::Events => {
                    tokio::spawn(monitor_chain_events(chain, addressbook.clone()));
                }
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

fn watch_account(
    addressbook: Arc<Mutex<HashMap<String, String>>>,
    new_account: WatchedAccount,
) -> u32 {
    addressbook
        .lock()
        .unwrap()
        .insert(new_account.address.to_lowercase(), new_account.label);

    addressbook.lock().unwrap().len() as u32
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
    provider: &Provider<ethers_providers::Http>,
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
    provider: &Provider<ethers_providers::Http>,
    block: T,
) -> Result<Vec<TransactionReceipt>, ProviderError> {
    let is_provider_alchemy = provider
        .url()
        .host_str()
        .unwrap_or("not")
        .contains("alchemy.com");

    if is_provider_alchemy {
        let wrapped_result = alchemy_get_block_receipts(provider, block).await;
        return match wrapped_result {
            Ok(res) => Ok(res.receipts),
            Err(err) => Err(err),
        };
    }
    provider.get_block_receipts(block).await
}

async fn debug_chain_blocks(
    chain: Chain,
    addressbook: Arc<Mutex<HashMap<String, String>>>,
    debug_block_number: u64,
) {
    let (chain, provider) = connect_and_verify(chain).await;

    let block = flexible_get_block_receipts(&provider, debug_block_number)
        .await
        .unwrap();

    loop {
        let now = Instant::now();
        let interesting_transactions = process_block(&block, addressbook.clone());

        let notifications =
            build_notifications(interesting_transactions, &chain, addressbook.clone());

        if !notifications.is_empty() {
            for notification in notifications {
                notification.send().await.unwrap();
            }
            info!("Notification sent, exiting");
            std::process::exit(0)
        }

        warn!("No transaction by monitored accounts found, have the accounts been setup?");

        let elapsed_time = now.elapsed();
        let sleep_time = chain.blocktime - elapsed_time;
        debug!("Sleeping for: {} ms", sleep_time.as_millis());
        sleep(sleep_time).await;
    }
}

async fn monitor_chain_blocks(chain: Chain, addressbook: Arc<Mutex<HashMap<String, String>>>) {
    let (chain, provider) = connect_and_verify(chain).await;

    info!("Starting Account Watcher for {} in Blocks Mode", chain.name);

    let mut next_block_number = provider.get_block_number().await.unwrap();

    let mut retry_count = 0;

    loop {
        let now = Instant::now();
        let block_number = match provider.get_block_number().await {
            Ok(res) => res,
            Err(_) => {
                error!(
                    "Error while getting {} block number from RPC, retrying",
                    chain.name
                );

                if retry_count > START_BACKOFF_RETRY_COUNT {
                    error!(
                        "{} retry count {}, waiting {} seconds before next retry",
                        chain.name,
                        retry_count,
                        chain.blocktime.as_secs()
                    );
                    sleep(chain.blocktime).await;
                }
                retry_count += 1;
                continue;
            }
        };

        debug!("Current block number on {}: {}", chain.name, block_number);

        while next_block_number <= block_number {
            debug!("Processing {} block {}", chain.name, next_block_number);
            let block_response = flexible_get_block_receipts(&provider, next_block_number).await;

            let block = match block_response {
                Ok(res) => res,
                Err(_) => {
                    error!(
                        "Error while getting {} block receipts from RPC, retrying",
                        chain.name
                    );
                    break;
                }
            };

            let interesting_transactions = process_block(&block, addressbook.clone());
            let notifications =
                build_notifications(interesting_transactions, &chain, addressbook.clone());

            for notification in notifications {
                let sent_notification = notification.send().await;
                if sent_notification.is_err() {
                    error!("Error while sending notification, retrying");
                    break;
                }
            }
            next_block_number = next_block_number + 1
        }

        CURRENT_BLOCK
            .with_label_values(&[chain.name.as_str()])
            .set(block_number.try_into().unwrap());

        retry_count = 0;

        let elapsed_time = now.elapsed();

        if elapsed_time < chain.blocktime {
            let sleep_time = chain.blocktime - elapsed_time;
            debug!("Sleeping {} for: {} ms", chain.name, sleep_time.as_millis());
            sleep(sleep_time).await;
        }
    }
}

async fn debug_chain_events(
    chain: Chain,
    addressbook: Arc<Mutex<HashMap<String, String>>>,
    debug_block_number: u64,
) {
    let (chain, provider) = connect_and_verify(chain).await;

    let events = provider
        .get_logs(&LogFilter::new().select(debug_block_number))
        .await
        .unwrap();

    loop {
        let now = Instant::now();
        let interesting_transactions = parse_logs(&events, addressbook.clone());
        let notifications =
            build_notifications(interesting_transactions, &chain, addressbook.clone());

        if !notifications.is_empty() {
            for notification in notifications {
                notification.send().await.unwrap();
            }
            info!("Notification sent, exiting");
            std::process::exit(0)
        }

        warn!("No transaction by monitored accounts found, have the accounts been setup?");

        let elapsed_time = now.elapsed();
        let sleep_time = chain.blocktime - elapsed_time;
        debug!("Sleeping for: {} ms", sleep_time.as_millis());
        sleep(sleep_time).await;
    }
}

async fn monitor_chain_events(chain: Chain, addressbook: Arc<Mutex<HashMap<String, String>>>) {
    let (chain, provider) = connect_and_verify(chain).await;

    info!("Starting Account Watcher for {} Event Mode", chain.name);

    let mut next_block_number = provider.get_block_number().await.unwrap();

    let mut retry_count = 0;

    loop {
        let now = Instant::now();
        let block_number = match provider.get_block_number().await {
            Ok(res) => res,
            Err(_) => {
                error!(
                    "Error while getting {} block number from RPC, retrying",
                    chain.name
                );

                if retry_count > START_BACKOFF_RETRY_COUNT {
                    error!(
                        "{} retry count {}, waiting {} seconds before next retry",
                        chain.name,
                        retry_count,
                        chain.blocktime.as_secs()
                    );
                    sleep(chain.blocktime).await;
                }
                retry_count += 1;
                continue;
            }
        };

        let block_number_with_delay = block_number - 1;

        debug!("Current block number on {}: {}", chain.name, block_number);

        CURRENT_BLOCK
            .with_label_values(&[chain.name.as_str()])
            .set(block_number.try_into().unwrap());

        if next_block_number <= block_number_with_delay {
            let to_block = if block_number_with_delay - next_block_number <= MAX_BLOCK_RANGE.into()
            {
                block_number_with_delay
            } else {
                next_block_number + MAX_BLOCK_RANGE
            };

            debug!(
                "Processing {} from block {} to block {}",
                chain.name, next_block_number, to_block
            );
            let events = match provider
                .get_logs(
                    &LogFilter::new()
                        .from_block(next_block_number)
                        .to_block(to_block),
                )
                .await
            {
                Ok(events) => events,
                Err(_) => {
                    error!(
                        "Error while getting {} events from RPC, retrying",
                        chain.name
                    );

                    if retry_count > START_BACKOFF_RETRY_COUNT {
                        error!(
                            "{} retry count {}, waiting {} seconds before next retry",
                            chain.name,
                            retry_count,
                            chain.blocktime.as_secs()
                        );
                        sleep(chain.blocktime).await;
                    }
                    retry_count += 1;
                    continue;
                }
            };

            let interesting_transactions = parse_logs(&events, addressbook.clone());

            let notifications =
                build_notifications(interesting_transactions, &chain, addressbook.clone());

            for notification in notifications {
                let sent_notification = notification.send().await;
                if sent_notification.is_err() {
                    error!("Error while sending notification, retrying");
                    continue;
                }
            }
            next_block_number = to_block + 1;
        }

        retry_count = 0;

        let elapsed_time = now.elapsed();

        if elapsed_time < chain.blocktime {
            let sleep_time = chain.blocktime - elapsed_time;
            debug!("Sleeping {} for: {} ms", chain.name, sleep_time.as_millis());
            sleep(sleep_time).await;
        }
    }
}

fn parse_logs(
    logs: &[Log],
    addressbook_mutex: Arc<Mutex<HashMap<String, String>>>,
) -> Vec<InterestingTransaction> {
    let addressbook = addressbook_mutex.lock().unwrap();

    let watched_addresses_as_topics: Vec<H256> = addressbook
        .keys()
        .map(|addr| H256::from(Address::from_str(addr).unwrap()))
        .collect();

    let mut interesting_transactions: Vec<InterestingTransaction> = vec![];
    for log in logs.iter() {
        for topic in log.topics.iter() {
            if watched_addresses_as_topics.contains(topic) {
                let involved_account = Address::from_str(&topic.full_string()[26..]).unwrap();

                let start_interesting_transactions_count = interesting_transactions.len();

                if log.topics[0]
                    == H256::from_str(
                        "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
                        // TRANSFER_TOPIC
                    )
                    .unwrap()
                {
                    interesting_transactions.push(InterestingTransaction {
                        hash: log.transaction_hash.unwrap(),
                        from: Some(Address::from(log.topics[1])),
                        to: Some(Address::from(log.topics[2])),
                        kind: InterestingTransactionKind::Transfer,
                        amount: Some(U256::decode(&log.data).unwrap_or(U256::from("0"))),
                        token: Some(log.address),
                        involved_account,
                    });
                }
                if log.topics[0]
                    == H256::from_str(
                        "0xc3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62", //TRANSFER_SINGLE ERC1155
                    )
                    .unwrap()
                {
                    interesting_transactions.push(InterestingTransaction {
                        hash: log.transaction_hash.unwrap(),
                        from: Some(Address::from(log.topics[2])),
                        to: Some(Address::from(log.topics[3])),
                        kind: InterestingTransactionKind::Transfer1155,
                        amount: Some(U256::from("0")),
                        token: Some(log.address),
                        involved_account,
                    });
                }
                if log.topics[0]
                    == H256::from_str(
                        "0x8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925", // APPROVE
                    )
                    .unwrap()
                {
                    interesting_transactions.push(InterestingTransaction {
                        hash: log.transaction_hash.unwrap(),
                        from: Some(Address::from(log.topics[1])),
                        to: Some(Address::from(log.topics[2])),
                        kind: InterestingTransactionKind::Approval,
                        amount: Some(U256::decode(&log.data).unwrap_or(U256::from("0"))),
                        token: Some(log.address),
                        involved_account,
                    });
                }
                if log.topics[0]
                    == H256::from_str(
                        "0x3d0ce9bfc3ed7d6862dbb28b2dea94561fe714a1b4d019aa8af39730d1ad7c3d", // SafeSend
                    )
                    .unwrap()
                {
                    interesting_transactions.push(InterestingTransaction {
                        hash: log.transaction_hash.unwrap(),
                        from: Some(Address::from(log.topics[1])),
                        to: Some(Address::from(log.address)),
                        kind: InterestingTransactionKind::Send,
                        amount: Some(U256::decode(&log.data).unwrap_or(U256::from("0"))),
                        token: None,
                        involved_account,
                    });
                }

                // Add as unknown transaction if no known logs were emmited
                if interesting_transactions.len() == start_interesting_transactions_count {
                    interesting_transactions.push(InterestingTransaction {
                        hash: log.transaction_hash.unwrap(),
                        involved_account,
                        from: None,
                        to: None,
                        kind: InterestingTransactionKind::Other,
                        amount: None,
                        token: None,
                    });
                }
            }
        }
    }
    interesting_transactions
}

fn process_block(
    block: &[TransactionReceipt],
    addressbook_mutex: Arc<Mutex<HashMap<String, String>>>,
) -> Vec<InterestingTransaction> {
    block
        .iter()
        .flat_map(|receipt| {
            let mut interesting_transactions = parse_logs(&receipt.logs, addressbook_mutex.clone());
            let addressbook = addressbook_mutex.lock().unwrap();
            if interesting_transactions.is_empty() {
                let involved_account = if addressbook.contains_key(&receipt.from.full_string()) {
                    Some(Address::from_str(&receipt.from.full_string()).unwrap())
                } else if receipt.to.is_some()
                    && addressbook.contains_key(&receipt.to.unwrap().full_string())
                {
                    Some(Address::from_str(&receipt.to.unwrap().full_string()).unwrap())
                } else {
                    None
                };

                if let Some(involved_account) = involved_account {
                    interesting_transactions.push(InterestingTransaction {
                        hash: receipt.transaction_hash,
                        from: Some(receipt.from),
                        to: receipt.to,
                        kind: if receipt.gas_used.unwrap() == U256::from_dec_str("21000").unwrap() {
                            InterestingTransactionKind::Send
                        } else {
                            InterestingTransactionKind::Other
                        },
                        amount: None,
                        token: None,
                        involved_account,
                    });
                }
            }
            interesting_transactions
        })
        .collect()
}

fn build_notifications(
    interesting_transactions: Vec<InterestingTransaction>,
    chain: &Chain,
    addressbook_mutex: Arc<Mutex<HashMap<String, String>>>,
) -> Vec<Notification> {
    let addressbook = addressbook_mutex.lock().unwrap();

    interesting_transactions
        .into_iter()
        .filter_map(|tx| {
            if tx.is_spam(&chain.spam_filter_level) {
                info!("Spam tx {} on {}", tx.hash.full_string(), chain.name);
                None
            } else {
                Some(tx)
            }
        })
        .fold(
            // Only one notification per transaction
            HashMap::<H256, InterestingTransaction>::new(),
            |mut acc, tx| {
                match acc.get(&tx.hash) {
                    Some(current_tx) => {
                        if tx.kind > current_tx.kind {
                            acc.insert(tx.hash, tx);
                        }
                    }
                    None => {
                        acc.insert(tx.hash, tx);
                    }
                };
                acc
            },
        )
        .values()
        .map(|tx| tx.build_notification(chain, &addressbook))
        .collect()
}

pub async fn connect_and_verify(mut chain: Chain) -> (Chain, Provider<Http>) {
    let url = reqwest::Url::parse(chain.rpc.as_str()).expect("Invalid RPC");
    let http_client = reqwest::Client::builder()
        .timeout(Duration::new(5, 0))
        .build()
        .unwrap();

    let provider = Provider::new(Http::new_with_client(url, http_client));

    let chainid = provider.get_chainid().await.unwrap();

    if chain.id.is_some() {
        if chainid != chain.id.unwrap() {
            panic!(
                "Configured for {} ({}) but connected to {}",
                chain.name,
                chain.id.unwrap(),
                chainid
            );
        }
    } else {
        chain.id = Some(chainid);
    }

    (chain, provider)
}
