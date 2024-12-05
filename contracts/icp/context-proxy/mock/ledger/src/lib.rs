use std::cell::RefCell;

use candid::{CandidType, Deserialize};
use ic_ledger_types::{AccountIdentifier, BlockIndex, Tokens, TransferArgs, TransferError};

thread_local! {
    static BALANCE: RefCell<u64> = RefCell::new(1_000_000_000);
}

type TransferResult = Result<BlockIndex, TransferError>;

#[ic_cdk::update]
fn transfer(args: TransferArgs) -> TransferResult {
    ic_cdk::println!(
        "Mock ledger received transfer: to={:?}, amount={}",
        args.to,
        args.amount
    );

    // Verify fee
    if args.fee.e8s() != 10_000 {
        return Err(TransferError::BadFee {
            expected_fee: Tokens::from_e8s(10_000),
        });
    }

    let amount_e8s = args.amount.e8s();

    BALANCE.with(|balance| {
        let mut bal = balance.borrow_mut();

        // Check if we have enough balance
        if amount_e8s > *bal {
            return Err(TransferError::InsufficientFunds {
                balance: Tokens::from_e8s(*bal),
            });
        }

        // Subtract amount and fee
        *bal = bal.saturating_sub(amount_e8s);
        *bal = bal.saturating_sub(args.fee.e8s());

        ic_cdk::println!("New balance: {}", *bal);

        // Return mock block index
        Ok(1)
    })
}

#[ic_cdk::query]
fn account_balance(_args: AccountBalanceArgs) -> Tokens {
    BALANCE.with(|balance| Tokens::from_e8s(*balance.borrow()))
}

#[derive(CandidType, Deserialize)]
struct AccountBalanceArgs {
    account: AccountIdentifier,
}

ic_cdk::export_candid!();
