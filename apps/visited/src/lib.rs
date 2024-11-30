#![allow(clippy::len_without_is_empty)]

use std::result::Result;

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::types::Error;
use calimero_storage::collections::{UnorderedMap, UnorderedSet};

#[app::state]
#[derive(Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct VisitedCities {
    visited: UnorderedMap<String, UnorderedSet<String>>,
}

#[app::logic]
impl VisitedCities {
    #[app::init]
    pub fn init() -> VisitedCities {
        VisitedCities {
            visited: UnorderedMap::new(),
        }
    }

    pub fn add_person(&mut self, person: String) -> Result<bool, Error> {
        Ok(self.visited.insert(person, UnorderedSet::new())?.is_some())
    }

    pub fn add_visited_city(&mut self, person: String, city: String) -> Result<bool, Error> {
        Ok(self.visited.get(&person)?.unwrap().insert(city)?)
    }

    pub fn get_person_with_most_cities_visited(&self) -> Result<String, Error> {
        let mut max = 0;
        let mut person = String::new();

        for entry in self.visited.entries()? {
            let (person_key, cities_set) = entry;
            if cities_set.len()? > max {
                max = cities_set.len()?;
                person = person_key.clone();
            }
        }
        Ok(person)
    }
}
