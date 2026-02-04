//! Tests for valid event definitions with various type patterns

use calimero_sdk::app;

// Basic event with simple variants
#[app::event]
pub enum SimpleEvent {
    Created,
    Updated,
    Deleted,
}

// Event with data in variants
#[app::event]
pub enum DataEvent {
    ValueSet { key: String, value: String },
    ValueRemoved { key: String },
    CountUpdated { new_count: u64 },
}

// Event with lifetime in variants
#[app::event]
pub enum RefEvent<'a> {
    StringRef { data: &'a str },
    SliceRef { items: &'a [u8] },
    MultiRef { first: &'a str, second: &'a str },
}

// Event with nested types
#[app::event]
pub enum NestedEvent {
    WithOption { value: Option<String> },
    WithVec { items: Vec<u64> },
    WithBoth { opt_items: Option<Vec<String>> },
}

// Event with tuple variants
#[app::event]
pub enum TupleEvent {
    Pair { data: (String, u64) },
    Triple { data: (String, u64, bool) },
    NestedTuple { data: ((String, u64), Vec<u8>) },
}

// Event with unit struct-like variant
#[app::event]
pub enum MixedEvent {
    Unit,
    Tuple(String, u64),
    Struct { key: String, value: u64 },
}

// Event with complex nested references
#[app::event]
pub enum ComplexRefEvent<'a> {
    SliceOfStrings { items: &'a [&'a str] },
    OptionalRef { data: Option<&'a str> },
    VecRef { items: &'a Vec<String> },
}

fn main() {}
