use core::cmp::Ordering;
use core::fmt;
use core::ops::Deref;
use std::borrow::Cow;

use borsh::{BorshDeserialize, BorshSerialize};

use super::env;

#[derive(Eq, Ord, Copy, Clone, Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
pub struct Id([u8; 32]);

impl Id {
    pub(super) fn new() -> Self {
        let mut id = [0; 32];

        env::random_bytes(&mut id);

        Self(id)
    }
}

impl From<[u8; 32]> for Id {
    fn from(id: [u8; 32]) -> Self {
        Self(id)
    }
}

impl Deref for Id {
    type Target = [u8; 32];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Eq, Copy, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Keys {
    pub data: Id,
    pub kids: Id,
}

#[derive(Eq, Copy, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]
pub struct Metadata {
    pub hash: [u8; 32],
    pub children: u64,
    pub created_at: u64,
    pub last_modified: u64,
    pub keys: Keys,
    // parent_id: when syncing
}

impl Metadata {
    pub fn is_deleted(&self) -> bool {
        self.hash == [0; 32]
    }
}

#[derive(Eq, Copy, Clone, Debug, PartialEq, BorshSerialize, BorshDeserialize)]

pub struct ChildRef {
    pub id: Id,
    pub hash: [u8; 32],
    #[borsh(skip)]
    pub children: u64,
    #[borsh(skip)]
    pub created_at: u64,
    #[borsh(skip)]
    pub last_modified: u64,
}

impl ChildRef {
    pub fn new_sparse(id: Id) -> Self {
        Self {
            id,
            hash: [0; 32],
            children: 0,
            created_at: 0,
            last_modified: 0,
        }
    }
}

impl Ord for ChildRef {
    fn cmp(&self, other: &Self) -> Ordering {
        self.created_at
            .cmp(&other.created_at)
            .then_with(|| self.id.cmp(&other.id))
    }
}

impl PartialOrd for ChildRef {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum RawEntry<'a> {
    Meta(Metadata),
    Data(Data<'a>),
    Kids(Vec<ChildRef>),
}

#[derive(Clone, BorshSerialize, BorshDeserialize)]
pub struct Data<'a>(pub Cow<'a, [u8]>);

impl fmt::Debug for Data<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() {
            fmt::Debug::fmt(&self.0, f)
        } else {
            fmt::Debug::fmt(&self.0.len(), f)
        }
    }
}

pub fn lookup(key: Id) -> Option<RawEntry<'static>> {
    let data = env::storage_read(*key)?;

    let entry = match borsh::from_slice(&data) {
        Ok(entry) => entry,
        Err(err) => env::panic_str(&format!("failed to deserialize state entry: {}", err)),
    };

    Some(entry)
}

pub fn write(key: Id, value: RawEntry<'_>) {
    let data = match borsh::to_vec(&value) {
        Ok(data) => data,
        Err(err) => env::panic_str(&format!("failed to serialize state entry: {}", err)),
    };

    let _ = env::storage_write(*key, data);
}

pub fn remove(key: Id) {
    let _ = env::storage_remove(*key);
}
