use std::collections::BTreeMap;
use std::sync::LazyLock;

use borsh::{to_vec, BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};
use velcro::btree_map;

use crate::address::Id;
use crate::entities::{AtomicUnit, ChildInfo, Collection, Data, Element};
use crate::interface::{Interface, StorageError};

/// A set of non-empty test UUIDs.
pub const TEST_UUID: [[u8; 16]; 5] = [
    [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    [2, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    [3, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    [4, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
    [5, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16],
];

/// A set of non-empty test IDs.
pub static TEST_ID: LazyLock<[Id; 5]> = LazyLock::new(|| {
    [
        Id::from(TEST_UUID[0]),
        Id::from(TEST_UUID[1]),
        Id::from(TEST_UUID[2]),
        Id::from(TEST_UUID[3]),
        Id::from(TEST_UUID[4]),
    ]
});

/// For tests against empty data structs.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct EmptyData {
    pub storage: Element,
}

impl Data for EmptyData {
    fn calculate_merkle_hash(&self) -> Result<[u8; 32], StorageError> {
        let mut hasher = Sha256::new();
        hasher.update(self.element().id().as_bytes());
        hasher.update(&to_vec(&self.element().metadata).map_err(StorageError::SerializationError)?);
        Ok(hasher.finalize().into())
    }

    fn calculate_merkle_hash_for_child(
        &self,
        collection: &str,
        _slice: &[u8],
    ) -> Result<[u8; 32], StorageError> {
        Err(StorageError::UnknownCollectionType(collection.to_owned()))
    }

    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }

    fn is_root() -> bool {
        true
    }
}

/// A simple page with a title, and paragraphs as children.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Page {
    pub title: String,
    pub paragraphs: Paragraphs,
    pub storage: Element,
}

impl Page {
    /// Creates a new page with a title from an existing element.
    pub fn new_from_element(title: &str, element: Element) -> Self {
        Self {
            title: title.to_owned(),
            paragraphs: Paragraphs::new(),
            storage: element,
        }
    }
}

impl AtomicUnit for Page {}

impl Data for Page {
    fn calculate_merkle_hash(&self) -> Result<[u8; 32], StorageError> {
        let mut hasher = Sha256::new();
        hasher.update(self.element().id().as_bytes());
        hasher.update(&to_vec(&self.title).map_err(StorageError::SerializationError)?);
        hasher.update(&to_vec(&self.element().metadata).map_err(StorageError::SerializationError)?);
        Ok(hasher.finalize().into())
    }

    fn calculate_merkle_hash_for_child(
        &self,
        collection: &str,
        slice: &[u8],
    ) -> Result<[u8; 32], StorageError> {
        match collection {
            "Paragraphs" => {
                let child = <Paragraphs as Collection>::Child::try_from_slice(slice)
                    .map_err(|e| StorageError::DeserializationError(e))?;
                child.calculate_merkle_hash()
            }
            _ => Err(StorageError::UnknownCollectionType(collection.to_owned())),
        }
    }

    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        btree_map! {
            "Paragraphs".to_owned(): Interface::child_info_for(self.id(), &self.paragraphs).unwrap_or_default(),
        }
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }

    fn is_root() -> bool {
        true
    }
}

/// A simple paragraph with text. No children. Belongs to a page.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Paragraph {
    pub text: String,
    pub storage: Element,
}

impl Paragraph {
    /// Creates a new paragraph with text, from an existing element.
    pub fn new_from_element(text: &str, element: Element) -> Self {
        Self {
            text: text.to_owned(),
            storage: element,
        }
    }
}

impl AtomicUnit for Paragraph {}

impl Data for Paragraph {
    fn calculate_merkle_hash(&self) -> Result<[u8; 32], StorageError> {
        let mut hasher = Sha256::new();
        hasher.update(self.element().id().as_bytes());
        hasher.update(&to_vec(&self.text).map_err(StorageError::SerializationError)?);
        hasher.update(&to_vec(&self.element().metadata).map_err(StorageError::SerializationError)?);
        Ok(hasher.finalize().into())
    }

    fn calculate_merkle_hash_for_child(
        &self,
        collection: &str,
        _slice: &[u8],
    ) -> Result<[u8; 32], StorageError> {
        Err(StorageError::UnknownCollectionType(collection.to_owned()))
    }

    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }

    fn is_root() -> bool {
        false
    }
}

/// A collection of paragraphs for a page.
#[derive(BorshDeserialize, BorshSerialize, Clone, Copy, Debug, Eq, PartialEq, PartialOrd)]
pub struct Paragraphs;

impl Paragraphs {
    /// Creates a new paragraph collection.
    pub fn new() -> Self {
        Self {}
    }
}

impl Collection for Paragraphs {
    type Child = Paragraph;

    fn name(&self) -> &'static str {
        "Paragraphs"
    }
}

/// A simple person example struct. No children.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Person {
    pub name: String,
    pub age: u8,
    pub storage: Element,
}

impl Data for Person {
    fn calculate_merkle_hash(&self) -> Result<[u8; 32], StorageError> {
        let mut hasher = Sha256::new();
        hasher.update(self.element().id().as_bytes());
        hasher.update(&to_vec(&self.name).map_err(StorageError::SerializationError)?);
        hasher.update(&to_vec(&self.age).map_err(StorageError::SerializationError)?);
        hasher.update(&to_vec(&self.element().metadata).map_err(StorageError::SerializationError)?);
        Ok(hasher.finalize().into())
    }

    fn calculate_merkle_hash_for_child(
        &self,
        collection: &str,
        _slice: &[u8],
    ) -> Result<[u8; 32], StorageError> {
        Err(StorageError::UnknownCollectionType(collection.to_owned()))
    }

    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }

    fn is_root() -> bool {
        true
    }
}
