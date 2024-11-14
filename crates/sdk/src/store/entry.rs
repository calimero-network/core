use std::borrow::Borrow;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::fmt::Debug;
use std::mem::take;
use std::ops::{Deref, DerefMut};
use std::sync::LazyLock;
use std::{io, mem, ptr};

use borsh::{BorshDeserialize, BorshSerialize};
use sha2::{Digest, Sha256};

use super::base::{self, ChildRef, Data, Id, Keys, Metadata, RawEntry};
use super::env;

static EMPTY_HASH: LazyLock<[u8; 32]> = LazyLock::new(|| Sha256::new().finalize().into());

#[derive(Debug, BorshDeserialize)]
pub struct Entry<T> {
    id: Id,
    #[borsh(skip, bound(deserialize = ""))]
    item: Cache<T>,
    #[borsh(skip)]
    meta: Cache<Metadata>,
    #[borsh(skip, bound(deserialize = ""))]
    kids: Cache<BTreeSet<Entry<T>>>,
}

#[derive(Debug)]
struct Cache<T> {
    entry: RefCell<Option<CacheEntry<T>>>,
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            entry: RefCell::default(),
        }
    }
}

#[derive(Debug)]
struct CacheEntry<T> {
    item: Option<T>,
    state: CacheState,
}

#[derive(Debug)]
enum CacheState {
    Fresh,
    Clean,
    Dirty,
}

impl CacheState {
    fn set_dirty(&mut self) {
        if let Self::Clean = self {
            *self = Self::Dirty;
        }
    }
}

impl<T> Cache<T> {
    fn new(item: T) -> Self {
        Self {
            entry: RefCell::new(Some(CacheEntry {
                item: Some(item),
                state: CacheState::Fresh,
            })),
        }
    }

    fn get(&self) -> Option<&mut CacheEntry<T>> {
        let mut item = self.entry.borrow_mut();

        let item = item.as_mut()?;

        Some(unsafe { &mut *ptr::from_mut(item) })
    }

    fn get_or_init_with(&self, init: impl FnOnce() -> Option<T>) -> &mut CacheEntry<T> {
        let mut item = self.entry.borrow_mut();

        let item = item.get_or_insert_with(|| CacheEntry {
            item: init(),
            state: CacheState::Clean,
        });

        unsafe { &mut *ptr::from_mut(item) }
    }

    fn flush(&self, only_dirty: bool) -> Option<&T> {
        self.get()?.flush(only_dirty)
    }
}

impl<T> CacheEntry<T> {
    fn get(&self) -> Option<&T> {
        self.item.as_ref()
    }

    fn get_mut(&mut self) -> Option<&mut T> {
        self.state.set_dirty();
        self.get_mut_raw()
    }

    fn get_mut_raw(&mut self) -> Option<&mut T> {
        self.item.as_mut()
    }

    fn flush(&mut self, only_dirty: bool) -> Option<&T> {
        match &self.state {
            CacheState::Dirty => {}
            CacheState::Fresh if !only_dirty => {}
            _ => return None,
        }

        self.state = CacheState::Clean;

        self.item.as_ref()
    }

    fn take(&mut self) -> Option<T> {
        self.state.set_dirty();
        self.item.take()
    }
}

thread_local! {
    static CURRENT: RefCell<Option<BTreeSet<ChildRef>>> = RefCell::default();
}

pub fn commit() {
    CURRENT.with(|ctx| {
        let _ignored = ctx.borrow_mut().get_or_insert_with(Default::default);
    });
}

impl<T: BorshSerialize + Debug> BorshSerialize for Entry<T> {
    fn serialize<W: io::Write>(&self, writer: &mut W) -> io::Result<()> {
        let mut parent = CURRENT.with(|ctx| {
            let mut parent = ctx.borrow_mut();

            Some(mem::take(parent.as_mut()?))
        });

        dbg!(&parent);

        let res = (|| {
            self.id.serialize(writer)?;

            if parent.is_some() {
                self.flush_contained(false)?;
            }

            Ok(())
        })();

        dbg!("DONE WITH PARENT");
        dbg!(self.id);
        dbg!(&parent);

        CURRENT.with(|ctx| {
            if let Some(parent) = &mut parent {
                let mut current = ctx.borrow_mut();

                let children_has_changes = current.as_ref().map_or(false, |c| !c.is_empty());

                dbg!(children_has_changes);

                if children_has_changes {
                    let children = self.children_mut();

                    println!("-----");
                    dbg!(&children);

                    while let Some(child_ref) = current
                        .as_mut()
                        .expect("this should be guaranteed")
                        .pop_first()
                    {
                        let child = children
                            .take(&child_ref.id)
                            .unwrap_or_else(|| Self::from_ref(child_ref, false));

                        let child_meta = child.metadata_raw().get_mut_raw().unwrap();
                        // let child_meta = child.metadata_mut();

                        child_meta.hash = child_ref.hash;
                        child_meta.children = child_ref.children;
                        child_meta.created_at = child_ref.created_at;
                        child_meta.last_modified = child_ref.last_modified;

                        let _ignored = children.insert(child);
                    }

                    drop(current);
                }

                dbg!(&self.kids);

                if let Some(children) = self.kids.flush(false) {
                    let mut last_modified = 0;
                    let mut digest = Sha256::new();

                    for child in children.iter() {
                        // if child.item.get().is_none() {
                        //     child.serialize(&mut io::sink())?;
                        // }

                        let child_meta = child.metadata();

                        digest.update(child_meta.hash);
                        last_modified = last_modified.max(child_meta.last_modified);
                    }

                    let digest = digest.finalize().into();

                    let meta = self.metadata_mut();

                    if meta.hash != digest {
                        meta.hash = digest;
                        meta.children = children.len() as u64;
                        meta.last_modified = meta.last_modified.max(last_modified);
                    }

                    let _ignored = parent.insert(ChildRef::from(self));

                    self.flush_contained(true)?;
                }
            }

            let _ignored = ctx.replace(parent);

            io::Result::Ok(())
        })?;

        res
    }
}

impl<T> Entry<T> {
    fn flush_contained(&self, write_meta: bool) -> io::Result<()>
    where
        T: BorshSerialize + Debug,
    {
        if let Some(metadata) = self.internal_flush(write_meta, false)? {
            if let Some(item) = self.item.flush(false) {
                dbg!(item);

                let item = borsh::to_vec(item)?.into();

                base::write(metadata.keys.data, RawEntry::Data(Data(item)));
            }

            let kids = self.kids.flush(false);

            dbg!(&kids);

            if let Some(children) = kids {
                let mut kids = Vec::with_capacity(children.len());

                for child in children {
                    // if child.item.get().is_none() {
                    //     child.serialize(&mut io::sink())?;
                    // } else {
                    child.flush_contained(write_meta)?;
                    // }

                    kids.push(ChildRef::from(child));
                }

                base::write(metadata.keys.kids, RawEntry::Kids(kids));
            }
        }

        Ok(())
    }

    fn internal_flush(&self, write_meta: bool, only_dirty: bool) -> io::Result<Option<&Metadata>> {
        dbg!(&self.id);
        dbg!(&self.meta);
        dbg!(write_meta);

        if write_meta {
            let x = self.meta.flush(only_dirty);

            dbg!(&x);

            if let Some(meta) = x {
                base::write(self.id, RawEntry::Meta(*meta));
            }
        }

        let metadata = self.metadata();

        dbg!(metadata);

        if !dbg!(metadata.is_deleted()) {
            return Ok(Some(metadata));
        }

        let children = self.children_raw();

        if let Some(children) = children.get_mut_raw() {
            while let Some(child) = children.pop_first() {
                drop(child);
            }
        }

        if children.flush(true).is_some() {
            base::remove(metadata.keys.kids);
        }

        if let Some(entry) = self.item.get() {
            if entry.flush(true).is_some() {
                base::remove(metadata.keys.data);
            }
        } /* else if base::has(metadata.keys.data) {
              base::remove(metadata.keys.data);
          } */

        Ok(None)
    }
}

impl<T> Entry<T> {
    pub fn new(value: T) -> Self {
        Self::new_raw(None, Some(value))
    }

    pub fn new_dangling() -> Self {
        Self::new_raw(None, None)
    }

    pub(super) fn new_raw(id: Option<Id>, value: Option<T>) -> Self {
        let mut buf = [0; (32 * 3)];

        env::random_bytes(&mut buf[..32 * (2 + (id.is_none() as usize))]);

        let mut ids = buf
            .chunks_exact(32)
            .map(|chunk| <Id as From<[u8; 32]>>::from(chunk.try_into().unwrap()));

        let meta_id = id.unwrap_or_else(|| ids.next().unwrap());
        let item_id = ids.next().unwrap();
        let kids_id = ids.next().unwrap();

        let now = env::time_now();

        Self {
            id: meta_id,
            item: value.map_or_else(Cache::default, Cache::new),
            meta: Cache::new(Metadata {
                hash: *EMPTY_HASH,
                created_at: now,
                last_modified: now,
                children: 0,
                keys: Keys {
                    data: item_id,
                    kids: kids_id,
                },
            }),
            kids: Cache::new(BTreeSet::new()),
        }
    }

    pub const fn id(&self) -> Id {
        self.id
    }

    fn metadata_raw(&self) -> &mut CacheEntry<Metadata> {
        self.meta.get_or_init_with(|| {
            let meta = match base::lookup(self.id)? {
                RawEntry::Meta(meta) => meta,
                entry => {
                    env::panic_str(&format!("expected Meta, found {:?}", entry));
                }
            };

            Some(meta)
        })
    }

    fn metadata(&self) -> &Metadata {
        self.metadata_raw().get().expect("failed to get metadata")
    }

    fn metadata_mut(&self) -> &mut Metadata {
        self.metadata_raw()
            .get_mut()
            .expect("failed to get metadata")
    }
}

impl<T: BorshDeserialize> Entry<T> {
    fn item_raw(&self) -> &mut CacheEntry<T> {
        self.item.get_or_init_with(|| {
            let metadata = self.metadata();

            if metadata.is_deleted() {
                return None;
            }

            let item = match base::lookup(metadata.keys.data)? {
                RawEntry::Data(Data(item)) => item,
                entry => {
                    env::panic_str(&format!("expected Data, found {:?}", entry));
                }
            };

            let item = borsh::from_slice(&item).expect("failed to deserialize item");

            Some(item)
        })
    }

    fn item(&self) -> Option<&T> {
        self.item_raw().get()
    }

    fn item_mut(&self) -> Option<&mut T> {
        self.item_raw().get_mut()
    }
}

impl<T> Entry<T> {
    fn from_ref(child_ref: ChildRef, prevalidate: bool) -> Self {
        let child = Self {
            id: child_ref.id,
            item: Cache::default(),
            meta: Cache::default(),
            kids: Cache::default(),
        };

        if prevalidate {
            let meta = child.metadata();

            if !(child_ref.hash == meta.hash && child_ref.created_at == meta.created_at) {
                env::panic_str("fatal: parent encodes dependence on drifted child")
            }
        }

        child
    }
}

impl<T> From<&Entry<T>> for ChildRef {
    fn from(value: &Entry<T>) -> Self {
        let meta = value.metadata();

        ChildRef {
            id: value.id,
            hash: meta.hash,
            children: meta.children,
            created_at: meta.created_at,
            last_modified: meta.last_modified,
        }
    }
}

impl<T> Eq for Entry<T> {}

impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id && self.metadata() == other.metadata() /* && self.item == other.item */
    }
}

impl<T> Ord for Entry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.id.cmp(&other.id)
    }
}

impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Borrow<Id> for Entry<T> {
    fn borrow(&self) -> &Id {
        &self.id
    }
}

impl<T> Entry<T> {
    fn children_raw(&self) -> &mut CacheEntry<BTreeSet<Entry<T>>> {
        self.kids.get_or_init_with(|| {
            let children = match base::lookup(self.metadata().keys.kids)? {
                RawEntry::Kids(children) => children,
                entry => {
                    env::panic_str(&format!("expected Kids, found {:?}", entry));
                }
            };

            let kids = children
                .into_iter()
                .map(|child_ref| Entry::from_ref(child_ref, true))
                .collect();

            Some(kids)
        })
    }

    fn children(&self) -> &BTreeSet<Entry<T>> {
        self.children_raw().get().expect("failed to get children")
    }

    fn children_mut(&self) -> &mut BTreeSet<Entry<T>> {
        self.children_raw()
            .get_mut()
            .expect("failed to get children")
    }
}

trait IntoEntry<T> {
    fn into_entry(self) -> Entry<T>;
}

impl<T> IntoEntry<T> for T {
    fn into_entry(self) -> Entry<T> {
        Entry::new(self)
    }
}

impl<T> IntoEntry<T> for Entry<T> {
    fn into_entry(self) -> Entry<T> {
        self
    }
}

impl<T> Entry<T> {
    pub fn insert(&mut self, id: Id, value: T) {
        let children = self.children_mut();

        let entry = children.take(&id);

        let entry = entry.unwrap_or_else(|| {
            self.metadata_mut().children += 1;
            Entry::new_raw(Some(id), Some(value))
        });

        let _ = children.insert(entry);
    }
}

impl<T: BorshSerialize + BorshDeserialize> Entry<T> {
    pub fn get(&self, id: &Id) -> Option<&T> {
        let entry = self.children().get(id);

        entry.and_then(|entry| entry.item())
    }

    pub fn len(&self) -> usize {
        self.metadata().children as usize
    }

    pub fn entries(&self) -> impl Iterator<Item = &T> {
        self.children().iter().flat_map(|entry| entry.item())
    }

    pub fn get_mut(&mut self, id: &Id) -> Option<EntryMut<'_, T>> {
        let item = self.children_mut().take(id)?;

        Some(EntryMut {
            parent: self,
            child: Some(item),
        })
    }
}

pub struct EntryMut<'a, T> {
    parent: &'a mut Entry<T>,
    child: Option<Entry<T>>,
}

impl<T: BorshDeserialize> EntryMut<'_, T> {
    pub fn get(&self) -> &T {
        self.child
            .as_ref()
            .and_then(|entry| entry.item())
            .expect("item should exist")
    }

    pub fn get_mut(&mut self) -> &mut T {
        self.child
            .as_mut()
            .and_then(|entry| entry.item_mut())
            .expect("item should exist")
    }

    pub fn remove(mut self) -> T {
        let child = self.child.take().expect("child should exist");

        child.item_raw().take().expect("item should exist")
    }
}

impl<T> Drop for EntryMut<'_, T> {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            let _ignored = self.parent.children_mut().insert(child);
        }
    }
}

impl<T> Drop for Entry<T> {
    fn drop(&mut self) {
        dbg!("del");

        let committing = CURRENT.with(|ctx| ctx.borrow_mut().is_some());

        dbg!(committing);

        if committing {
            return;
        }

        let metadata = self.metadata_mut();

        metadata.hash = [0; 32];
        metadata.children = 0;

        let _ignored = self.internal_flush(true, true).expect("failed to flush");
    }
}

#[cfg(test)]
#[path = "entry_tests.rs"]
mod tests;
