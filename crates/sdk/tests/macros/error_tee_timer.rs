//! A TEE timer must name a period, and takes no arguments.

use calimero_sdk::app;

#[app::state]
struct MyState;

#[app::logic]
impl MyState {
    #[app::init]
    pub fn init() -> MyState {
        MyState
    }

    #[app::tee(every = "30s")]
    pub fn sweep(&mut self, turn: u32) {
        let _ = turn;
    }

    #[app::tee(every = "soon")]
    pub fn vague(&mut self) {}

    #[app::tee(daily)]
    pub fn unknown(&mut self) {}
}

fn main() {}
