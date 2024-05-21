use calimero_sdk::app;

use crate::Choice;

#[app::event]
pub enum Event<'a> {
    PlayerCommited { id: usize },
    NewPlayer { id: usize, name: &'a str },
    PlayerRevealed { id: usize, reveal: &'a Choice },
    GameOver(Option<usize>),
    StateDumped,
}
