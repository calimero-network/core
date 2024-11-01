use borsh::{BorshDeserialize, BorshSerialize};

use crate::event::AppEvent;
use crate::external::External;

pub trait AppState: BorshSerialize + BorshDeserialize + AppStateInit {
    type Event<'a>: AppEvent + 'a;

    #[inline]
    fn external(&self) -> External {
        External {}
    }
}

pub trait Identity<This = Self> {}

impl<T: AppState> Identity<T> for T {}

#[diagnostic::on_unimplemented(
    message = "(calimero)> no method named `#[app::init]` found for type `{Self}`",
    label = "add an `#[app::init]` method to this type"
)]
pub trait AppStateInit: Sized {
    type Return: Identity<Self>;
}
