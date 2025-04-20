use ethers::core::types::{Address, H256, U256};
use log::debug;
use std::collections::HashMap;

use crate::{
    chain::{Chain, SpamFilterLevel},
    notification::Notification,
    token::{FromChainAddress, Token},
};
use account_monitor::{scale_amount, FullString, IsKnownToken, ToLabel};

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum InterestingTransactionKind {
    Send = 100,
    Transfer = 50,
    Transfer1155 = 49,
    Approval = 25,
    Other = 0,
}

#[derive(Debug)]
pub struct InterestingTransaction {
    pub hash: H256,
    pub from: Option<Address>,
    pub to: Option<Address>,
    pub kind: InterestingTransactionKind,
    pub amount: Option<U256>,
    pub contract: Option<Address>,
    pub involved_account: Address,
}

pub trait BuildNotification {
    fn build_notification(
        &self,
        chain: &Chain,
        addressbook: &HashMap<String, String>,
    ) -> Notification;
}

impl BuildNotification for InterestingTransaction {
    fn build_notification(
        &self,
        chain: &Chain,
        addressbook: &HashMap<String, String>,
    ) -> Notification {
        debug!("Interesting tx: {}", self.hash.full_string());

        let message = match self.kind {
            InterestingTransactionKind::Send => {
                if self.amount.is_some() {
                    let scaled_amount = scale_amount(self.amount.unwrap(), 18);
                    format!(
                        "Sending {} native from {} to {} on {}",
                        scaled_amount,
                        self.from.unwrap().to_label(addressbook),
                        self.to.unwrap().to_label(addressbook),
                        chain.name
                    )
                } else {
                    format!(
                        "Sending native from {} to {} on {}",
                        self.from.unwrap().to_label(addressbook),
                        self.to.unwrap().to_label(addressbook),
                        chain.name
                    )
                }
            }

            InterestingTransactionKind::Transfer => {
                let token: Token = Token::from_chain_address(chain, self.contract.unwrap());

                let scaled_amount = scale_amount(self.amount.unwrap(), token.decimals);
                format!(
                    "Transfering {} {} from {} to {} on {}",
                    scaled_amount,
                    token.symbol,
                    self.from.unwrap().to_label(addressbook),
                    self.to.unwrap().to_label(addressbook),
                    chain.name
                )
            }

            InterestingTransactionKind::Transfer1155 => {
                let token: Token = Token::from_chain_address(chain, self.contract.unwrap());

                format!(
                    "Transfering ERC1155 {} from {} to {} on {}",
                    token.symbol,
                    self.from.unwrap().to_label(addressbook),
                    self.to.unwrap().to_label(addressbook),
                    chain.name
                )
            }

            InterestingTransactionKind::Approval => {
                let token: Token = Token::from_chain_address(chain, self.contract.unwrap());

                let scaled_amount = match self.amount.unwrap() == U256::MAX {
                    true => "Infinite".to_string(),
                    false => scale_amount(self.amount.unwrap(), token.decimals),
                };
                format!(
                    "Approving {} to spend {} {} from {} on {}",
                    self.to.unwrap().to_label(addressbook),
                    scaled_amount,
                    token.symbol,
                    self.from.unwrap().to_label(addressbook),
                    chain.name
                )
            }

            InterestingTransactionKind::Other => {
                format!(
                    "Unknown operation involving {} on {}",
                    self.involved_account.to_label(addressbook),
                    chain.name
                )
            }
        };

        let url = chain
            .explorer
            .clone()
            .map(|explorer| format!("{}/tx/{}", explorer, self.hash.full_string()));

        Notification { message, url }
    }
}

pub trait SpamFilter {
    fn is_spam(&self, spam_filter_level: &SpamFilterLevel) -> bool;
}

impl SpamFilter for InterestingTransaction {
    fn is_spam(&self, spam_filter_level: &SpamFilterLevel) -> bool {
        match spam_filter_level {
            SpamFilterLevel::None => false,
            SpamFilterLevel::KnownAssets => match self.kind {
                InterestingTransactionKind::Send => false,
                InterestingTransactionKind::Other => false,
                _ => {
                    !(self.contract.unwrap().is_known_token())
                        || self.involved_account != self.from.unwrap()
                }
            },

            SpamFilterLevel::SelfSubmittedTxs => match self.kind {
                InterestingTransactionKind::Send => false,
                InterestingTransactionKind::Other => false,
                _ => self.involved_account != self.from.unwrap(),
            },
        }
    }
}
