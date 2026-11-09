//! Test guest for `tests/host_collections.rs`: one table, one map and one tally, every
//! operation exposed as a method, built with and without `host-collections`. Names have
//! several words so camelCase shows in keys and errors.

use calimero_sdk::prelude::*;
use serde_json::{json, Value};

#[derive(Record)]
#[table(
    key = "item_id",
    index(by_owner = ["owner_name"]),
    unique(by_email = ["email"]),
    count(per_owner = ["owner_name"]),
)]
pub struct Item {
    pub item_id: u64,
    pub owner_name: String,
    pub email: Option<String>,
    pub note: String,
}

#[app::state(version = 1)]
pub struct HostTable {
    shop_items: Table<Item>,
    user_notes: UnorderedMap<(String, u64), String>,
    vote_tally: Tally<String>,
}

fn item(i: &Item) -> Value {
    json!({
        "itemId": i.item_id.to_string(),
        "ownerName": i.owner_name,
        "email": i.email,
        "note": i.note,
    })
}

fn opts(cursor: Option<String>, limit: Option<u32>, reverse: bool) -> FindOpts {
    FindOpts {
        limit,
        cursor,
        reverse,
        from: None,
        to: None,
    }
}

fn items(page: Page<Item>) -> Value {
    json!({
        "items": page.items.iter().map(item).collect::<Vec<_>>(),
        "next": page.next,
    })
}

fn notes(page: Page<((String, u64), String)>) -> Value {
    json!({
        "items": page
            .items
            .iter()
            .map(|((who, n), v)| json!([who, n.to_string(), v]))
            .collect::<Vec<_>>(),
        "next": page.next,
    })
}

#[app::logic]
impl HostTable {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    // --- table ---------------------------------------------------------------------

    pub fn insert(
        &mut self,
        item_id: u64,
        owner_name: String,
        email: Option<String>,
        note: String,
    ) -> app::Result<()> {
        self.shop_items.insert(Item {
            item_id,
            owner_name,
            email,
            note,
        })?;
        Ok(())
    }

    pub fn upsert(
        &mut self,
        item_id: u64,
        owner_name: String,
        email: Option<String>,
        note: String,
    ) -> app::Result<()> {
        self.shop_items.upsert(Item {
            item_id,
            owner_name,
            email,
            note,
        })?;
        Ok(())
    }

    #[app::view]
    pub fn get(&self, item_id: u64) -> app::Result<Option<Value>> {
        Ok(self.shop_items.get(&item_id)?.as_ref().map(item))
    }

    #[app::view]
    pub fn contains(&self, item_id: u64) -> app::Result<bool> {
        Ok(self.shop_items.contains(&item_id)?)
    }

    pub fn set_note(&mut self, item_id: u64, note: String) -> app::Result<bool> {
        Ok(self.shop_items.update(&item_id, |i| i.note = note)?)
    }

    pub fn set_owner(&mut self, item_id: u64, owner_name: String) -> app::Result<bool> {
        Ok(self
            .shop_items
            .update(&item_id, |i| i.owner_name = owner_name)?)
    }

    pub fn set_email(&mut self, item_id: u64, email: Option<String>) -> app::Result<bool> {
        Ok(self.shop_items.update(&item_id, |i| i.email = email)?)
    }

    /// Changes the key: refused (`KeyChanged`).
    pub fn set_id(&mut self, item_id: u64, new_id: u64) -> app::Result<bool> {
        Ok(self.shop_items.update(&item_id, |i| i.item_id = new_id)?)
    }

    pub fn remove(&mut self, item_id: u64) -> app::Result<bool> {
        Ok(self.shop_items.remove(&item_id)?)
    }

    #[app::view]
    pub fn find(
        &self,
        index: String,
        owner_name: Option<String>,
        cursor: Option<String>,
        limit: Option<u32>,
        reverse: bool,
    ) -> app::Result<Value> {
        let prefix: Vec<KeyPart> = owner_name.iter().map(|o| o.to_key_part()).collect();
        let page = self
            .shop_items
            .find(&index, &prefix, opts(cursor, limit, reverse))?;
        Ok(items(page))
    }

    #[app::view]
    pub fn scan(
        &self,
        cursor: Option<String>,
        limit: Option<u32>,
        reverse: bool,
    ) -> app::Result<Value> {
        Ok(items(self.shop_items.scan(opts(cursor, limit, reverse))?))
    }

    /// A scan from an id on: `from` is served by the guest path on both builds.
    #[app::view]
    pub fn scan_from(&self, item_id: u64) -> app::Result<Value> {
        let opts = FindOpts::default().from(vec![item_id.to_key_part()]);
        Ok(items(self.shop_items.scan(opts)?))
    }

    #[app::view]
    pub fn count(&self, name: String, owner_name: Option<String>) -> app::Result<u64> {
        let prefix: Vec<KeyPart> = owner_name.iter().map(|o| o.to_key_part()).collect();
        Ok(self.shop_items.count(&name, &prefix)?)
    }

    // --- map -----------------------------------------------------------------------

    pub fn note_set(&mut self, who: String, n: u64, text: String) -> app::Result<Option<String>> {
        Ok(self.user_notes.insert(&(who, n), &text)?)
    }

    #[app::view]
    pub fn note_get(&self, who: String, n: u64) -> app::Result<Option<String>> {
        Ok(self.user_notes.get(&(who, n))?)
    }

    #[app::view]
    pub fn note_has(&self, who: String, n: u64) -> app::Result<bool> {
        Ok(self.user_notes.contains(&(who, n))?)
    }

    pub fn note_remove(&mut self, who: String, n: u64) -> app::Result<Option<String>> {
        Ok(self.user_notes.remove(&(who, n))?)
    }

    #[app::view]
    pub fn notes(
        &self,
        who: Option<String>,
        cursor: Option<String>,
        limit: Option<u32>,
        reverse: bool,
    ) -> app::Result<Value> {
        let opts = opts(cursor, limit, reverse);
        let page = match who {
            Some(who) => self.user_notes.prefix(&[who.to_key_part()], opts)?,
            None => self.user_notes.page(opts)?,
        };
        Ok(notes(page))
    }

    #[app::view]
    pub fn notes_starting(&self, start: String) -> app::Result<Value> {
        let page = self
            .user_notes
            .starts_with(&[start.to_key_part()], FindOpts::default())?;
        Ok(notes(page))
    }

    #[app::view]
    pub fn notes_ends(&self) -> app::Result<Value> {
        let end =
            |e: Option<((String, u64), String)>| e.map(|((w, n), v)| json!([w, n.to_string(), v]));
        Ok(json!([
            end(self.user_notes.first()?),
            end(self.user_notes.last()?)
        ]))
    }

    #[app::view]
    pub fn notes_len(&self) -> app::Result<u64> {
        Ok(self.user_notes.len()?)
    }

    pub fn notes_clear(&mut self) -> app::Result<u64> {
        Ok(self.user_notes.clear()?)
    }

    // --- tally ---------------------------------------------------------------------

    pub fn vote(&mut self, choice: String, delta: i64) -> app::Result<()> {
        self.vote_tally.add(&choice, delta)?;
        Ok(())
    }

    #[app::view]
    pub fn votes(&self, choice: String) -> app::Result<i64> {
        Ok(self.vote_tally.get(&choice)?)
    }
}
