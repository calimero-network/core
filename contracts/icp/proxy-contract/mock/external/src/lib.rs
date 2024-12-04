use std::cell::RefCell;

thread_local! {
    static CALLS: RefCell<Vec<Vec<u8>>> = RefCell::new(Vec::new());
}

#[ic_cdk::update]
fn test_method(args: Vec<u8>) -> Vec<u8> {
    CALLS.with(|calls| {
        calls.borrow_mut().push(args.clone());
        args // Return the same args back
    })
}

#[ic_cdk::query]
fn get_calls() -> Vec<Vec<u8>> {
    CALLS.with(|calls| calls.borrow().clone())
}

ic_cdk::export_candid!();
