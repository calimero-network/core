//! Interior-mutability / shared-ownership wrappers are rejected in state.

use std::cell::RefCell;
use std::rc::Rc;

use calimero_sdk::app;

#[app::state]
struct BadState {
    cell: RefCell<u64>,
    shared: Rc<u64>,
}

#[app::logic]
impl BadState {
    #[app::init]
    pub fn init() -> BadState {
        BadState {
            cell: RefCell::new(0),
            shared: Rc::new(0),
        }
    }
}

fn main() {}
