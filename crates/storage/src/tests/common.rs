use std::sync::LazyLock;

use borsh::{BorshDeserialize, BorshSerialize};

use crate::address::Id;
use crate::entities::{AtomicUnit, Collection, Data, Element};

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
    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
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
    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
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
    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}

/// A collection of paragraphs for a page.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Paragraphs {
    pub child_ids: Vec<Id>,
}

impl Paragraphs {
    /// Creates a new paragraph collection.
    pub fn new() -> Self {
        Self { child_ids: vec![] }
    }
}

impl Collection for Paragraphs {
    type Child = Paragraph;

    fn child_ids(&self) -> &Vec<Id> {
        &self.child_ids
    }

    fn has_children(&self) -> bool {
        !self.child_ids.is_empty()
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
    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}
