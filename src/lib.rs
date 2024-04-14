use ethers::core::{
    types::{Address, H256, U256},
    utils::format_units,
};

use eyre::Result;
use std::collections::HashMap;

pub trait FullString {
    fn full_string(&self) -> Result<String>;
}

impl FullString for Address {
    fn full_string(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?.replace('"', ""))
    }
}

impl FullString for H256 {
    fn full_string(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?.replace('"', ""))
    }
}

pub fn address_name(
    addressbook: HashMap<String, String>,
    address: Option<Address>,
) -> Result<String> {
    match address {
        Some(address) => {
            let full_address = address.full_string()?.to_owned();

            let res = if addressbook.contains_key(&full_address) {
                (*addressbook.get(&full_address).unwrap())
                    .clone()
                    .to_string()
            } else if &full_address == "0x0000000000000000000000000000000000000000" {
                "NULL".to_owned()
            } else if &full_address == "0x4822521e6135cd2599199c83ea35179229a172ee" {
                "Gnosis Pay Spender".to_owned()
            } else {
                full_address
            };

            Ok(res)
        }
        None => Ok("Unknown".to_owned()),
    }
}

pub fn scale_amount(amount: U256, decimals: u32) -> String {
    let scaled_amount = format_units(amount, decimals).unwrap();

    let separator_index = scaled_amount.find('.').unwrap();
    let least_significant_index = scaled_amount.len()
        - scaled_amount
            .chars()
            .rev()
            .position(|c| !(c == '0' || c == '.'))
            .unwrap_or(scaled_amount.len());

    let cutoff = if separator_index > least_significant_index {
        separator_index
    } else {
        least_significant_index
    };

    scaled_amount[..cutoff].to_owned()
}
