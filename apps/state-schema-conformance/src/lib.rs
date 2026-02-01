#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_storage::collections::{Counter, LwwRegister, UnorderedMap, UnorderedSet, Vector};

// Test types for state schema conformance

// Newtype bytes
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct UserId32([u8; 32]);

// Record types - these are used in LwwRegister for CRDT semantics
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Person {
    id: UserId32,
    name: String,
    age: u32,
}

// Profile with all CRDT fields (can be used directly in UnorderedMap)
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct Profile {
    bio: LwwRegister<Option<String>>,
    visit_count: Counter,
}

impl calimero_storage::collections::Mergeable for Profile {
    fn merge(
        &mut self,
        other: &Self,
    ) -> Result<(), calimero_storage::collections::crdt_meta::MergeError> {
        self.bio.merge(&other.bio);
        self.visit_count.merge(&other.visit_count)?;
        Ok(())
    }
}

// Variant types
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum Status {
    Active { timestamp: u64 },
    Inactive,
    Pending { reason: String },
}

// State with comprehensive Calimero collection types
#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct StateSchemaConformance {
    // Maps with various value types (all using UnorderedMap with LwwRegister values)
    string_map: UnorderedMap<String, LwwRegister<String>>, // map<string, string>
    int_map: UnorderedMap<String, LwwRegister<u32>>,       // map<string, u32>
    record_map: UnorderedMap<String, LwwRegister<Person>>, // map<string, Person>
    nested_map: UnorderedMap<String, UnorderedMap<String, LwwRegister<u32>>>, // map<string, map<string, u32>> (direct nesting)

    // Lists using Vector (Calimero collection) - Vector items must be CRDTs
    counter_list: Vector<Counter>,              // list<Counter>
    register_list: Vector<LwwRegister<String>>, // list<LwwRegister<String>>
    record_list: Vector<LwwRegister<Person>>,   // list<Person> (wrapped for CRDT)
    nested_list: Vector<Vector<Counter>>,       // list<list<Counter>>

    // Nested collections
    map_of_counters: UnorderedMap<String, Counter>, // map<string, Counter>
    map_of_lists: UnorderedMap<String, Vector<Counter>>, // map<string, list<Counter>>
    list_of_maps: Vector<UnorderedMap<String, LwwRegister<u32>>>, // list<map<string, u32>>

    // Sets
    string_set: UnorderedSet<String>, // set<string>

    // Counters
    visit_counter: Counter, // counter

    // Records with collections (Profile implements Mergeable)
    profile_map: UnorderedMap<String, Profile>, // map<string, Profile>

    // Variants wrapped in LwwRegister (for CRDT semantics)
    status: LwwRegister<Status>, // Variant enum

    // Newtype bytes wrapped in LwwRegister
    user_id: LwwRegister<UserId32>, // Newtype [u8; 32]

    // Scalar types wrapped in LwwRegister (required for CRDT semantics)
    counter: LwwRegister<u64>,
    name: LwwRegister<String>,
    active: LwwRegister<bool>,
}

#[app::logic]
impl StateSchemaConformance {
    #[app::init]
    pub fn init() -> StateSchemaConformance {
        StateSchemaConformance {
            string_map: UnorderedMap::new_with_field_name(None, "string_map"),
            int_map: UnorderedMap::new_with_field_name(None, "int_map"),
            record_map: UnorderedMap::new_with_field_name(None, "record_map"),
            nested_map: UnorderedMap::new_with_field_name(None, "nested_map"),
            counter_list: Vector::new_with_field_name(None, "counter_list"),
            register_list: Vector::new_with_field_name(None, "register_list"),
            record_list: Vector::new_with_field_name(None, "record_list"),
            nested_list: Vector::new_with_field_name(None, "nested_list"),
            map_of_counters: UnorderedMap::new_with_field_name(None, "map_of_counters"),
            map_of_lists: UnorderedMap::new_with_field_name(None, "map_of_lists"),
            list_of_maps: Vector::new_with_field_name(None, "list_of_maps"),
            string_set: UnorderedSet::new_with_field_name(None, "string_set"),
            visit_counter: Counter::new_with_field_name(None, "visit_counter"),
            profile_map: UnorderedMap::new_with_field_name(None, "profile_map"),
            status: LwwRegister::new(Status::Inactive),
            user_id: LwwRegister::new(UserId32([0; 32])),
            counter: LwwRegister::new(0),
            name: LwwRegister::new(String::new()),
            active: LwwRegister::new(false),
        }
    }
}
