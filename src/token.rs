use ethers::core::types::Address;
use rusqlite::{named_params, Connection};

use crate::chain::Chain;
use account_monitor::FullString;

pub struct Token {
    pub symbol: String,
    pub decimals: u32, // ERC20 supports only u8, but format units expects u32
}

pub trait FromChainAddress {
    fn from_chain_address(chain: &Chain, address: Address) -> Token;
}

impl FromChainAddress for Token {
    fn from_chain_address(chain: &Chain, address: Address) -> Token {
        let connection = Connection::open("rotki_db.db").unwrap();
        let query = "SELECT
                   decimals,
                   symbol
                FROM evm_tokens
                JOIN common_asset_details ON evm_tokens.identifier = common_asset_details.identifier
                WHERE
                  lower(address) = lower(:address) AND
                  chain = :chain";
        let mut statement = connection.prepare(query).unwrap();

        let res: Result<Token, rusqlite::Error> = statement.query_row(
            named_params! {":address": address.full_string(),":chain": chain.id.unwrap().as_u64()},
            |row| {
                Ok(Token {
                    decimals: row.get(0).unwrap(),
                    symbol: row.get(1).unwrap(),
                })
            },
        );

        match res {
            Ok(res_token) => res_token,
            Err(_) => Token {
                decimals: 18,
                symbol: "UNK".to_owned(),
            },
        }
    }
}
