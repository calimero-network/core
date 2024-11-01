#![allow(
    clippy::use_self,
    clippy::must_use_candidate,
    unused_crate_dependencies,
    reason = "False positives"
)]

use near_sdk::{env, near, PanicOnDefault};

#[near(contract_state)]
#[derive(PanicOnDefault, Clone, Copy, Debug)]
pub struct CounterContract {
    counter: u32,
}

#[near]
impl CounterContract {
    #[init]
    pub const fn new() -> Self {
        Self { counter: 0 }
    }

    pub fn increment(&mut self) {
        self.counter = self.counter.wrapping_add(1);
        env::log_str("Counter incremented");
    }

    pub const fn get_count(&self) -> u32 {
        self.counter
    }
}
