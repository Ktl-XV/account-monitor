use ethers::core::types::U256;
use std::env;
use std::str::FromStr;
use std::time::Duration;
use strum_macros::EnumString;

#[derive(Clone, Debug)]
pub enum ChainMode {
    Blocks,
    Events,
}

#[derive(Clone, Debug, EnumString)]
pub enum SpamFilterLevel {
    None,
    KnownAssets,
    SelfSubmittedTxs,
}

#[derive(Clone, Debug)]
pub struct Chain {
    pub id: Option<U256>,
    pub name: String,
    pub blocktime: Duration,
    pub explorer: Option<String>,
    pub rpc: String,
    pub mode: ChainMode,
    pub spam_filter_level: SpamFilterLevel,
}

pub trait EnvInitializable {
    fn init_from_env(suffix: Option<String>) -> Self;
    fn init_from_env_vec() -> Vec<Self>
    where
        Self: std::marker::Sized;
}

impl EnvInitializable for Chain {
    fn init_from_env(suffix: Option<String>) -> Chain {
        let clean_sufix = suffix.unwrap_or("".to_string());

        let chain_id_var = format!("CHAIN_ID{}", clean_sufix);
        let chain_name_var = format!("CHAIN_NAME{}", clean_sufix);
        let chain_blocktime_var = format!("CHAIN_BLOCKTME{}", clean_sufix);
        let chain_explorer_var = format!("CHAIN_EXPLORER{}", clean_sufix);
        let chain_rpc_var = format!("CHAIN_RPC{}", clean_sufix);
        let chain_mode_var = format!("CHAIN_MODE{}", clean_sufix);
        let chain_spam_filter_level_var = format!("CHAIN_SPAM_FILTER_LEVEL{}", clean_sufix);

        Chain {
            id: match &env::var(&chain_id_var) {
                Ok(chain_id) => Some(U256::from_dec_str(chain_id).expect("Invalid CHAIN_ID")),
                Err(_) => None,
            },

            name: env::var(&chain_name_var)
                .unwrap_or_else(|_| panic!("Missing {}", &chain_name_var)),
            blocktime: Duration::from_millis(
                env::var(&chain_blocktime_var)
                    .unwrap_or_else(|_| panic!("Missing {}", &chain_blocktime_var))
                    .parse::<u64>()
                    .expect("Invalid CHAIN_BLOCKTME"),
            ),
            explorer: env::var(&chain_explorer_var).ok(),
            rpc: env::var(&chain_rpc_var).unwrap_or_else(|_| panic!("Missing {}", &chain_rpc_var)),
            mode: match env::var(&chain_mode_var)
                .unwrap_or("Blocks".to_string())
                .as_str()
            {
                "Blocks" => ChainMode::Blocks,
                "Events" => ChainMode::Events,
                &_ => panic!("Invalid {}", &chain_mode_var),
            },
            spam_filter_level: SpamFilterLevel::from_str(
                &env::var(&chain_spam_filter_level_var).unwrap_or("KnownAssets".to_string()),
            )
            .unwrap_or_else(|_| panic!("Invalid {}", &chain_spam_filter_level_var)),
        }
    }

    fn init_from_env_vec() -> Vec<Chain> {
        env::var("CHAINS")
            .expect("Missing CHAINS")
            .split(',')
            .map(|chain| Self::init_from_env(Some(format!("_{}", chain))))
            .collect()
    }
}
