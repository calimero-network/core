use candid::{candid_method, export_service};
use ic_cdk_macros::*;
use std::cell::RefCell;

thread_local! {
    static CALLS: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
}

#[update]
#[candid_method]
fn test_method(args: Vec<u8>) {
    CALLS.with(|calls| {
        calls.borrow_mut().push(args);
    });
}

#[query]
#[candid_method]
fn get_calls() -> Vec<Vec<u8>> {
    CALLS.with(|calls| calls.borrow().clone())
}

export_service!(); 