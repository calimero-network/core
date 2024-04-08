use std::collections::HashMap;

use borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::{app, env};

mod code_generated_from_calimero_sdk_macros;

#[app::state]
#[derive(Default, BorshSerialize, BorshDeserialize)]
struct KvStore {
    items: HashMap<String, String>,
}

struct MyType(Result<i32, i32>, i32);

struct Castam<'a>(&'a u8);

#[app::logic]
impl<'a> crate::Castam<'a> {
    pub fn method(
        &self,
        a: &'a [&'a crate::Castam<'a>],
        // ref mut g: &mut u8,
        // &mut g: &mut u8,
        // (e, v): (u8, u8),
        // all @ MyType(opt @ Ok(a) | opt @ Err(a), b): MyType,
    ) -> Result<(), &'static str> {
        // let d = g;
        todo!()
    }

    // pub fn method00(self) {}
    // pub fn method01(&self) {}
    // pub const fn method02(&self) {}
    // pub fn method03(self: Self) {}
    // pub fn method030(mut self: Self) {}
    // pub fn method040(self: (Self)) {}
    // pub fn method041(mut self: (Self)) {}
    // pub fn method050(self: &'a Self) {}
    // pub fn method051(mut self: &'a Self) {}
    // pub fn method060(self: &'a (Self)) {}
    // pub fn method061(mut self: &'a (Self)) {}
    // pub fn method070(self: &'a mut Self) {}
    // pub fn method071(mut self: &'a mut Self) {}
    // pub fn method080(self: &'a mut (Self)) {}
    // pub fn method081(mut self: &'a mut (Self)) {}
    // pub fn method090(self: KvStore) {}
    // pub fn method091(mut self: KvStore) {}
    // pub fn method100(self: &'a KvStore) {}
    // pub fn method101(mut self: &'a KvStore) {}
    // pub fn method110(self: &'a (KvStore)) {}
    // pub fn method111(mut self: &'a (KvStore)) {}
    // pub fn method120(self: &'a mut KvStore) {}
    // pub fn method121(mut self: &'a mut KvStore) {}
    // pub fn method130(self: &'a mut (KvStore)) {}
    // pub fn method131(mut self: &'a mut (KvStore)) {}
    // pub fn method140(self: Castam) {}
    // pub fn method151(self: &'a Castam) {}
}

impl KvStore {
    // pub fn method(
    //     self,
    //     all @ MyType(opt @ Ok(a) | opt @ Err(a), b): MyType,
    // ) -> Result<(), &'static str> {
    //     todo!()
    // }

    // #[app::destroy]
    // pub fn destroy(self, MyType(a, b): MyType) -> Result<(), &'static str> {
    //     Err("Failed.")
    // }

    // pub fn booly(self: &'a mut KvStore) -> Result<(), &'static str> {
    //     Err("Failed.")
    // }

    // pub fn destr(&mut self, MyType(a, b): MyType) -> Result<(), &'static str> {
    //     Err("Failed.")
    // }

    // pub fn or(&mut self, MyType(Ok(a) | Err(a), b): MyType) -> Result<(), &'static str> {
    //     Err("Failed.")
    // }

    // ? can we strip lifetimes, and replace them? that'll be awesome
    fn set(&mut self, key: &str, value: &str) -> Self {
        env::log(&format!("Setting key: {:?} to value: {:?}", key, value));

        self.items.insert(key.to_owned(), value.to_owned());

        Self {
            items: self.items.clone(),
        }
    }

    fn entries(&self) -> &HashMap<String, String> {
        env::log(&format!("Getting all entries"));

        &self.items
    }

    fn get(&self, key: &str) -> Option<&str> {
        env::log(&format!("Getting key: {:?}", key));

        self.items.get(key).map(|v| v.as_str())
    }

    fn get_unchecked(&self, key: &str) -> &str {
        env::log(&format!("Getting key without checking: {:?}", key));

        match self.items.get(key) {
            Some(value) => value.as_str(),
            None => env::panic_str("Key not found."),
        }
    }

    fn get_result(&self, key: &str) -> Result<&str, &str> {
        env::log(&format!("Getting key, possibly failing: {:?}", key));

        self.get(key).ok_or("Key not found.")
    }

    fn remove(&mut self, key: &str) {
        env::log(&format!("Removing key: {:?}", key));

        self.items.remove(key);
    }

    fn clear(&mut self) {
        env::log("Clearing all entries");

        self.items.clear();
    }
}
