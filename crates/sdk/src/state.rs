use borsh::{BorshDeserialize, BorshSerialize};

use crate::event::AppEvent;

pub trait AppState: Default + BorshSerialize + BorshDeserialize + AppStateInit {
    type Event<'a>: AppEvent + 'a;
}

#[diagnostic::on_unimplemented(
    message = "The type `{Self}` doesn't have an `#[app::init]` method",
    label = "add an `#[app::init]` method to this type"
)]
pub trait AppStateInit {}
