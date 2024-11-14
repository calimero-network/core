use std::fmt::Debug;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::store::entry::Entry;

#[derive(Debug, BorshSerialize, BorshDeserialize)]
struct Vector<T: Debug> {
    inner: Entry<T>,
}
