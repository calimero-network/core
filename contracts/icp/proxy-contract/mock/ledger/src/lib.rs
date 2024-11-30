use std::cell::RefCell;
use candid::{CandidType, Deserialize, Principal};

thread_local! {
    static BALANCE: RefCell<u64> = RefCell::new(1_000_000_000);
}

#[derive(CandidType, Deserialize)]
struct TransferArgs {
    to: Principal,
    amount: u128,
}

#[ic_cdk::update]
fn transfer(args: Vec<u8>) {
    let transfer_args: TransferArgs = candid::decode_one(&args)
        .expect("Failed to decode transfer args");
        
    ic_cdk::println!("Mock ledger received transfer: to={:?}, amount={}", 
        transfer_args.to, transfer_args.amount);
        
    BALANCE.with(|balance| {
        let mut bal = balance.borrow_mut();
        *bal = bal.saturating_sub(transfer_args.amount.try_into().unwrap());
        ic_cdk::println!("New balance: {}", *bal);
    });
}

#[ic_cdk::query]
fn balance() -> u128 {
    BALANCE.with(|balance| {
        let bal = *balance.borrow();
        bal.into()
    })
}

ic_cdk::export_candid!();
