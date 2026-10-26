use calimero_sdk::app;
use calimero_storage::collections::LwwRegister;

#[app::state]
struct State {
    value: LwwRegister<String>,
}

#[app::logic]
impl State {
    #[app::init]
    pub fn init() -> State {
        State {
            value: LwwRegister::new(String::new()),
        }
    }

    #[app::view]
    pub fn get(&self) -> &str {
        &self.value
    }

    #[app::xcall]
    pub fn ping(&mut self) {}

    pub fn set(&mut self, value: String) {
        self.value.set(value);
    }
}

fn main() {}
