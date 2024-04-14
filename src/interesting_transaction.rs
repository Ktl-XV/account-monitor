use ethers::core::types::{Address, H256, U256};
use log::debug;
use std::collections::HashMap;

use crate::{
    chain::Chain,
    notification::Notification,
    token::{FromChainAddress, Token},
};
use account_monitor::{address_name, scale_amount, FullString};

pub enum InterestingTransactionKind {
    Send,
    Transfer,
    Other,
}

pub struct InterestingTransaction {
    pub hash: H256,
    pub from: Option<Address>,
    pub to: Option<Address>,
    pub kind: InterestingTransactionKind,
    pub amount: U256,
    pub token: Option<Address>,
    pub involved_account: Option<Address>,
}

pub trait BuildNotification {
    fn build_notification(
        &self,
        chain: &Chain,
        addressbook: HashMap<String, String>,
    ) -> Notification;
}

impl BuildNotification for InterestingTransaction {
    fn build_notification(
        &self,
        chain: &Chain,
        addressbook: HashMap<String, String>,
    ) -> Notification {
        debug!("Interesting tx: {}", self.hash.full_string().unwrap());
        let url = format!("{}/tx/{}", chain.explorer, self.hash.full_string().unwrap());
        let message = match self.kind {
            InterestingTransactionKind::Send => {
                format!(
                    "Sending native from {} to {} on {}",
                    address_name(addressbook.clone(), self.from).unwrap(),
                    address_name(addressbook, self.to).unwrap(),
                    chain.name
                )
            }

            InterestingTransactionKind::Transfer => {
                let token: Token = Token::from_chain_address(chain, self.token.unwrap());

                let scaled_amount = scale_amount(self.amount, token.decimals);
                format!(
                    "Transfering {} {} from {} to {} on {}",
                    scaled_amount,
                    token.symbol,
                    address_name(addressbook.clone(), self.from).unwrap(),
                    address_name(addressbook.clone(), self.to).unwrap(),
                    chain.name
                )
            }

            InterestingTransactionKind::Other => {
                format!(
                    "Unknown operation involving {} on {}",
                    address_name(addressbook.clone(), self.involved_account).unwrap(),
                    chain.name
                )
            }
        };

        Notification {
            message,
            url: Some(url),
        }
    }
}
