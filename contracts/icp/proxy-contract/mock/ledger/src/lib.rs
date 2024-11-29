use candid::{candid_method, export_service};
use ic_cdk_macros::*;
use std::cell::RefCell;

thread_local! {
    static BALANCE: RefCell<u64> = RefCell::new(1_000_000_000);
}

#[update]
#[candid_method]
fn transfer(to: String, amount: u64) {
    BALANCE.with(|balance| {
        let mut bal = balance.borrow_mut();
        *bal = bal.saturating_sub(amount);
    });
}

#[query]
#[candid_method]
fn balance() -> u64 {
    BALANCE.with(|balance| *balance.borrow())
}

export_service!();