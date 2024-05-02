use borsh::{BorshDeserialize, BorshSerialize};

use crate::event::AppEvent;

pub trait AppState: Default + BorshSerialize + BorshDeserialize {
    type Event<'a>: AppEvent + 'a;
}
