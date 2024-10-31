#![allow(unused_crate_dependencies, reason = "False positives")]

use near_sdk::{env, near, PanicOnDefault};

#[near(contract_state)]
#[derive(PanicOnDefault, Clone, Copy, Debug)]
pub struct CounterContract {
    counter: u32,
}

#[near]
impl CounterContract {
    #[init]
    pub fn new() -> Self {
        Self { counter: 0 }
    }

    pub fn increment(&mut self) {
        self.counter += 1;
        env::log_str("Counter incremented");
    }

    pub fn get_count(&self) -> u32 {
        self.counter
    }
}
