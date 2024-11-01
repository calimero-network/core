#![allow(clippy::len_without_is_empty)]

use std::result::Result;

use calimero_sdk::app;
use calimero_sdk::types::Error;
use calimero_storage::collections::{UnorderedMap, UnorderedSet};
use calimero_storage::entities::Element;
use calimero_storage::AtomicUnit;

#[app::state]
#[derive(AtomicUnit, Clone, Debug, PartialEq, PartialOrd)]
#[root]
#[type_id(1)]
pub struct VisitedCities {
    visited: UnorderedMap<String, UnorderedSet<String>>,
    #[storage]
    storage: Element,
}

#[app::logic]
impl VisitedCities {
    #[app::init]
    pub fn init() -> VisitedCities {
        VisitedCities {
            visited: UnorderedMap::new().unwrap(),
            storage: Element::root(),
        }
    }

    pub fn add_person(&mut self, person: String) -> Result<bool, Error> {
        Ok(self.visited.insert(person, UnorderedSet::new().unwrap())?)
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
