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

/// A two-part key, for prefix pages: one group's members are a prefix of the key.
#[derive(Record)]
#[table(key = ["group_name", "member"])]
pub struct Membership {
    pub group_name: String,
    pub member: String,
    pub since: u64,
}

#[app::state(version = 1)]
pub struct HostTable {
    shop_items: Table<Item>,
    group_members: Table<Membership>,
    user_notes: UnorderedMap<(String, u64), String>,
    /// 64-bit values: decimal strings from the host, numbers to plain serde.
    big_counts: UnorderedMap<String, Option<u64>>,
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

    /// `upsert_many` of one record per id (email by position, owner and note shared): how
    /// many were new. With `catch`, a refusal is answered instead of failing the call, so the
    /// run keeps whatever the batch wrote (nothing, all or nothing).
    pub fn upsert_many(
        &mut self,
        item_ids: Vec<u32>,
        emails: Vec<Option<String>>,
        owner_name: String,
        note: String,
        catch: bool,
    ) -> app::Result<Value> {
        let records = item_ids
            .into_iter()
            .zip(emails)
            .map(|(item_id, email)| Item {
                item_id: item_id.into(),
                owner_name: owner_name.clone(),
                email,
                note: note.clone(),
            });
        match self.shop_items.upsert_many(records) {
            Ok(added) => Ok(json!({ "added": added })),
            Err(error) if catch => Ok(json!({ "refused": format!("{error:?}") })),
            Err(error) => Err(error.into()),
        }
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

    pub fn join(&mut self, group_name: String, member: String, since: u64) -> app::Result<()> {
        self.group_members.upsert(Membership {
            group_name,
            member,
            since,
        })?;
        Ok(())
    }

    /// One group's members (`scan_prefix`), or every membership with no group.
    #[app::view]
    pub fn members(
        &self,
        group_name: Option<String>,
        cursor: Option<String>,
        limit: Option<u32>,
        reverse: bool,
    ) -> app::Result<Value> {
        let prefix: Vec<KeyPart> = group_name.iter().map(|g| g.to_key_part()).collect();
        let page = self
            .group_members
            .scan_prefix(&prefix, opts(cursor, limit, reverse))?;
        Ok(json!({
            "items": page
                .items
                .iter()
                .map(|m| json!([m.group_name, m.member, m.since.to_string()]))
                .collect::<Vec<_>>(),
            "next": page.next,
        }))
    }

    /// A prefix of the whole key and one more part: refused (`PrefixTooLong`).
    #[app::view]
    pub fn members_too_long(&self) -> app::Result<Value> {
        let prefix = ["g".to_key_part(), "m".to_key_part(), "x".to_key_part()];
        let page = self
            .group_members
            .scan_prefix(&prefix, FindOpts::default())?;
        Ok(json!(page.items.len()))
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

    /// Set a 64-bit map value (or none); the old one as a decimal string, else null.
    pub fn big_set(&mut self, who: String, n: Option<u64>) -> app::Result<Value> {
        let old = self.big_counts.insert(&who, &n)?;
        Ok(json!(old.map(|v| v.map(|v| v.to_string()))))
    }

    #[app::view]
    pub fn big_get(&self, who: String) -> app::Result<Value> {
        let got = self.big_counts.get(&who)?;
        Ok(json!(got.map(|v| v.map(|v| v.to_string()))))
    }

    pub fn big_remove(&mut self, who: String) -> app::Result<Value> {
        let old = self.big_counts.remove(&who)?;
        Ok(json!(old.map(|v| v.map(|v| v.to_string()))))
    }

    #[app::view]
    pub fn bigs(&self) -> app::Result<Value> {
        let page = self.big_counts.page(FindOpts::default())?;
        Ok(json!(page
            .items
            .iter()
            .map(|(who, v)| json!([who, v.map(|v| v.to_string())]))
            .collect::<Vec<_>>()))
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

    // --- collections the state does not declare: the guest path serves them ---------

    /// Writes to a table, a map and a tally built by hand, several calls each, and reads
    /// them back in the same invocation.
    pub fn scratch_write(&mut self, item_id: u64, owner_name: String) -> app::Result<Value> {
        let mut table: Table<Item> = Table::new("scratchItems");
        let mut map: UnorderedMap<String, String> = UnorderedMap::new("scratchMap");
        let mut tally: Tally<String> = Tally::new("scratchTally");
        table.insert(Item {
            item_id,
            owner_name: owner_name.clone(),
            email: None,
            note: "s".into(),
        })?;
        table.upsert(Item {
            item_id: item_id + 1,
            owner_name: owner_name.clone(),
            email: Some(format!("{owner_name}@scratch")),
            note: "t".into(),
        })?;
        let _old = map.insert(&owner_name, &"v".to_owned())?;
        let _old = map.insert(&format!("{owner_name}2"), &"w".to_owned())?;
        tally.add(&owner_name, 2)?;
        tally.add(&owner_name, 3)?;
        Self::scratch(&table, &map, &tally, &owner_name)
    }

    #[app::view]
    pub fn scratch_read(&self, owner_name: String) -> app::Result<Value> {
        let table: Table<Item> = Table::new("scratchItems");
        let map: UnorderedMap<String, String> = UnorderedMap::new("scratchMap");
        let tally: Tally<String> = Tally::new("scratchTally");
        Self::scratch(&table, &map, &tally, &owner_name)
    }

    fn scratch(
        table: &Table<Item>,
        map: &UnorderedMap<String, String>,
        tally: &Tally<String>,
        owner_name: &str,
    ) -> app::Result<Value> {
        let prefix = [owner_name.to_key_part()];
        Ok(json!({
            "found": items(table.find("byOwner", &prefix, FindOpts::default())?),
            "scan": items(table.scan(FindOpts::default().limit(1))?),
            "count": table.count("perOwner", &prefix)?.to_string(),
            "map": notes_of(map.page(FindOpts::default())?),
            "mapGet": map.get(&owner_name.to_owned())?,
            "tally": tally.get(&owner_name.to_owned())?.to_string(),
        }))
    }
}

fn notes_of(page: Page<(String, String)>) -> Value {
    json!({ "items": page.items, "next": page.next })
}
