use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use velcro::btree_map;

use crate::entities::{AtomicUnit, ChildInfo, Collection, Data, Element};
use crate::interface::MainInterface;

/// For tests against empty data structs.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct EmptyData {
    /// Storage element for this data structure.
    pub storage: Element,
}

impl Data for EmptyData {
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

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
    /// The title of the page.
    pub title: String,
    /// Collection of paragraphs in this page.
    pub paragraphs: Paragraphs,
    /// Storage element for this data structure.
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
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        btree_map! {
            "Paragraphs".to_owned(): MainInterface::child_info_for(self.id()).unwrap_or_default(),
        }
    }

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
    /// The text content of the paragraph.
    pub text: String,
    /// Storage element for this data structure.
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
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
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
}

/// A simple person example struct. No children.
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Eq, PartialEq, PartialOrd)]
pub struct Person {
    /// The name of the person.
    pub name: String,
    /// The age of the person.
    pub age: u8,
    /// Storage element for this data structure.
    pub storage: Element,
}

impl Data for Person {
    fn collections(&self) -> BTreeMap<String, Vec<ChildInfo>> {
        BTreeMap::new()
    }

    fn element(&self) -> &Element {
        &self.storage
    }

    fn element_mut(&mut self) -> &mut Element {
        &mut self.storage
    }
}
