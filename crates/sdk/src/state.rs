use borsh::{BorshDeserialize, BorshSerialize};

use crate::event::AppEvent;

pub trait AppState: Default + BorshSerialize + BorshDeserialize + AppStateInit {
    type Event<'a>: AppEvent + 'a;
}

pub trait Identity<This = Self> {}

impl<T: AppState> Identity<T> for T {}

#[diagnostic::on_unimplemented(
    message = "no method named `#[app::init]` found for type `{Self}`",
    label = "add an `#[app::init]` method to this type"
)]
pub trait AppStateInit: Sized {
    type Return: Identity<Self>;
}
