//! `# Returns` documents a return value; a method that returns nothing has none.

use calimero_sdk::app;

#[app::state]
struct S;

#[app::logic]
impl S {
    #[app::init]
    pub fn init() -> S {
        S
    }

    /// Clears the board.
    ///
    /// # Returns
    /// Nothing useful.
    pub fn clear(&mut self) -> app::Result<()> {
        Ok(())
    }
}

fn main() {}
