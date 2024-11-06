use ethers::core::{
    types::{Address, H256, U256},
    utils::format_units,
};
use rusqlite::{named_params, Connection};
use std::collections::HashMap;

pub trait FullString {
    fn full_string(&self) -> String;
}

impl FullString for Address {
    fn full_string(&self) -> String {
        serde_json::to_string(self).unwrap().replace('"', "")
    }
}

impl FullString for H256 {
    fn full_string(&self) -> String {
        serde_json::to_string(self).unwrap().replace('"', "")
    }
}

pub trait ToLabel {
    fn to_label(&self, addressbook: &HashMap<String, String>) -> String;
}

impl ToLabel for Address {
    fn to_label(&self, addressbook: &HashMap<String, String>) -> String {
        let full_address = &self.full_string();

        if addressbook.contains_key(full_address) {
            (*addressbook.get(full_address).unwrap())
                .clone()
                .to_string()
        } else if full_address == "0x0000000000000000000000000000000000000000" {
            "NULL".to_owned()
        } else if full_address == "0x4822521e6135cd2599199c83ea35179229a172ee" {
            "Gnosis Pay Spender".to_owned()
        } else {
            full_address.to_string()
        }
    }
}

pub trait IsKnownToken {
    fn is_known_token(&self) -> bool;
}

impl IsKnownToken for Address {
    fn is_known_token(&self) -> bool {
        let connection = Connection::open("rotki_db.db").unwrap();
        let query = "SELECT COUNT(1)
                FROM evm_tokens
                WHERE
                  lower(address) = lower(:address)";
        let mut statement = connection.prepare(query).unwrap();

        let res: Result<bool, rusqlite::Error> = statement
            .query_row(named_params! {":address": self.full_string()}, |row| {
                Ok(1 == row.get::<usize, i32>(0).unwrap())
            });

        res.unwrap()
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
