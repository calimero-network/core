use std::cell::RefCell;

use candid::Principal;
use ic_ledger_types::{AccountIdentifier, Memo, Subaccount, Tokens, TransferArgs, TransferError};

thread_local! {
    static CALLS: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
    static LEDGER_ID: RefCell<Option<Principal>> = RefCell::new(None);
}

#[ic_cdk::init]
fn init(ledger_id: Principal) {
    LEDGER_ID.with(|id| {
        *id.borrow_mut() = Some(ledger_id);
    });
}

#[ic_cdk::update]
async fn test_method(args: Vec<u8>) -> Vec<u8> {
    let self_id = ic_cdk::id();
    let caller = ic_cdk::caller();

    let ledger_id = LEDGER_ID.with(|id| id.borrow().expect("Ledger ID not initialized"));

    // Prepare transfer args to move the approved tokens
    let transfer_args = TransferArgs {
        memo: Memo(0),
        amount: Tokens::from_e8s(100_000_000), // Example amount, in practice this would be parsed from args
        fee: Tokens::from_e8s(10_000),
        from_subaccount: Some(Subaccount::from(caller)),
        to: AccountIdentifier::new(&self_id, &Subaccount([0; 32])),
        created_at_time: None,
    };

    // Execute the transfer with proper type annotations
    let transfer_result: Result<(Result<u64, TransferError>,), _> =
        ic_cdk::call(ledger_id, "transfer", (transfer_args,)).await;

    match transfer_result {
        Ok((Ok(_block_height),)) => {
            // Transfer successful, record the call
            CALLS.with(|calls| {
                calls.borrow_mut().push(args.clone());
            });
            args // Return the same args back
        }
        Ok((Err(transfer_error),)) => {
            ic_cdk::trap(&format!("Transfer failed: {:?}", transfer_error));
        }
        Err(e) => {
            ic_cdk::trap(&format!("Call to ledger failed: {:?}", e));
        }
    }
}

#[ic_cdk::update]
async fn test_method_no_transfer(args: Vec<u8>) -> Vec<u8> {
    // Simply record the call and return
    CALLS.with(|calls| {
        calls.borrow_mut().push(args.clone());
    });
    args
}

#[ic_cdk::query]
fn get_calls() -> Vec<Vec<u8>> {
    CALLS.with(|calls| calls.borrow().clone())
}

// Clear state (useful for testing)
#[ic_cdk::update]
fn clear_state() {
    CALLS.with(|calls| calls.borrow_mut().clear());
}

ic_cdk::export_candid!();
