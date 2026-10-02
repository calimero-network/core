//! An `emit!` naming a method of the same impl that is not `#[app::handler]`.

use calimero_sdk::app;

#[app::event]
pub enum Event {
    Touched,
}

#[app::state(emits = Event)]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    pub fn touch(&mut self) {
        app::emit!((Event::Touched, "on_touched"));
        app::emit!((Event::Touched, "tee:on_tee"));
        // Declared, and a name this impl does not define: both left alone.
        app::emit!((Event::Touched, "on_declared"));
        app::emit!((Event::Touched, "elsewhere"));
    }

    pub fn on_touched(&mut self) {}

    pub fn on_tee(&mut self) {}

    #[app::handler]
    pub fn on_declared(&mut self) {}
}

fn main() {}
