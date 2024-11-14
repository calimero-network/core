use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};

use super::map::Map;

#[derive(BorshSerialize, BorshDeserialize)]
struct Set<T: Debug> {
    inner: Map<T, ()>,
}
