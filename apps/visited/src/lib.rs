#![allow(clippy::len_without_is_empty)]

use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_storage::collections::{UnorderedMap, UnorderedSet};

#[app::state]
#[derive(Debug, BorshSerialize, BorshDeserialize)]
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

    pub fn add_person(&mut self, person: String) -> app::Result<bool> {
        Ok(self.visited.insert(person, UnorderedSet::new())?.is_some())
    }

    pub fn add_visited_city(&mut self, person: String, city: String) -> app::Result<bool> {
        Ok(self.visited.get(&person)?.unwrap().insert(city)?)
    }

    pub fn get_person_with_most_cities_visited(&self) -> app::Result<String> {
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
