#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::UnorderedMap;
use thiserror::Error;

#[app::state(emits = Event)]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct SampleApp {
    balances: UnorderedMap<String, usize>,
}

#[derive(Debug)]
#[app::event]
pub enum Event {
    BalanceUpdated { account: String, value: usize },
    BalanceSet { account: String, value: usize },
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error<'a> {
    #[error("Account not found: {0}")]
    NotFound(&'a str),
}

#[app::logic]
impl SampleApp {
    #[app::init]
    pub fn init() -> SampleApp {
        SampleApp {
            balances: UnorderedMap::new(),
        }
    }

    pub fn set(&mut self, account: String, value: usize) -> app::Result<()> {
        app::log!("Setting the account balance of {} ", account);

        if let Some(balance) = self.balances.insert(account.clone(), value)? {
            app::emit!(Event::BalanceUpdated {
                account,
                value: balance
            });
        } else {
            app::emit!(Event::BalanceSet { account, value });
        }

        Ok(())
    }

    pub fn get(&self, account: &str) -> app::Result<usize> {
        if let Some(balance) = self.balances.get(account)? {
            return Ok(balance);
        }
        app::bail!(Error::NotFound(account));
    }
}
