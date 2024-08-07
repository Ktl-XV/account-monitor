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
    BuildNotification, InterestingTransaction, InterestingTransactionKind,
};
use notification::{Notification, Sendable};

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

    let static_accounts_path_var = env::var("STATIC_ACCOUNTS_PATH");
    let mut watched_accounts_count: u32 = 0;
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
                        chain.clone(),
                        addressbook.clone(),
                        debug_block_number as i32,
                    ));
                }
                ChainMode::Events => {
                    tokio::spawn(debug_chain_events(
                        chain.clone(),
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
                    tokio::spawn(monitor_chain_blocks(chain.clone(), addressbook.clone()));
                }
                ChainMode::Events => {
                    tokio::spawn(monitor_chain_events(chain.clone(), addressbook.clone()));
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

async fn debug_chain_blocks(
    chain: Chain,
    addressbook: Arc<Mutex<HashMap<String, String>>>,
    debug_block_number: i32,
) {
    let provider = connect_and_verify(chain.clone()).await;

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
}

async fn monitor_chain_blocks(chain: Chain, addressbook: Arc<Mutex<HashMap<String, String>>>) {
    let provider = connect_and_verify(chain.clone()).await;

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

        debug!("Current block number on {}: {}", chain.name, block_number);

        CURRENT_BLOCK
            .with_label_values(&[chain.name.as_str()])
            .set(block_number.try_into().unwrap());

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
                    break;
                }
            };

            let notification = process_block(&block, &chain, addressbook.clone());

            if notification.is_some() {
                let sent_notification = notification.unwrap().send().await;
                if sent_notification.is_err() {
                    error!("Error while sending notification, retrying");
                    break;
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

async fn debug_chain_events(
    chain: Chain,
    addressbook: Arc<Mutex<HashMap<String, String>>>,
    debug_block_number: u64,
) {
    let provider = connect_and_verify(chain.clone()).await;

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
}

async fn monitor_chain_events(chain: Chain, addressbook: Arc<Mutex<HashMap<String, String>>>) {
    let provider = connect_and_verify(chain.clone()).await;

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

        debug!("Current block number on {}: {}", chain.name, block_number);

        CURRENT_BLOCK
            .with_label_values(&[chain.name.as_str()])
            .set(block_number.try_into().unwrap());

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

    let mut tx = None;
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
                    if watched_addresses_as_topics.contains(&log.topics[1]) {
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
                        debug!("Ignoring Spam");
                    }
                } else if log.topics[0]
                    == H256::from_str(
                        "0xc3d58168c5ae7397731d063d5bbf3d657854427343f4c083240f7aacaa2d0f62", //TRANSFER_SINGLE ERC1155
                    )
                    .unwrap()
                {
                    if watched_addresses_as_topics.contains(&log.topics[2]) {
                        return Some(InterestingTransaction {
                            hash: log.transaction_hash.unwrap(),
                            from: Some(Address::from(log.topics[2])),
                            to: Some(Address::from(log.topics[3])),
                            kind: InterestingTransactionKind::Transfer1155,
                            amount: (U256::from("0")),
                            token: Some(log.address),
                            involved_account: None,
                        });
                    } else {
                        debug!("Ignoring Spam");
                    }
                } else {
                    tx = Some(InterestingTransaction {
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
    tx
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
