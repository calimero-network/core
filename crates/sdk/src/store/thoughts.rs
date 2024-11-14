use std::cell::RefCell;
use std::collections::BTreeSet;

mod collections {
    use super::*;

    struct Root<T> {
        entry: Entry<T>,
        // item: keep and fetch
        // children: keep but never fetch
    }

    struct Map<K, V> {
        entries: Entry<(K, V)>,
        // item: don't keep, never fetch
        // children: keep and fetch
    }

    struct List<T> {
        entries: Entry<T>,
        // item: don't keep, never fetch
        // children: keep and fetch
    }

    struct Set<T> {
        entries: Entry<T>,
        // item: don't keep, never fetch
        // children: keep and fetch
    }
}

#[derive(Copy, Clone)]
struct Id([u8; 32]);

impl Id {
    fn new() -> Self {
        todo!()
    }
}

struct Entry<T> {
    id: Id,
    item: Option<CacheEntry<T>>,
    meta: Option<CacheEntry<Metadata>>,
    children: Option<CacheEntry<BTreeSet<Entry<T>>>>,
}

struct Metadata {
    hash: [u8; 32],
    created: u64,
    modified: u64,
    n_children: u64,
}

enum CacheEntry<T> {
    Clean(T),
    Dirty(T),
    Fresh(T),
}

// root: Entry<AppState>

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

// struct Thing {
//     counts: List<u8>,
// }

// struct Item {
//     names: List<String>,
//     things: List<Thing>,
// }

// struct State {
//     items: List<Item>,
// }

// List< Vec< Entry<T> > >
// ^-x   ^-?  ^-x

struct Context {
    heirarchy: Vec<Id>,
}

thread_local! {
    static CONTEXT: RefCell<Context> = RefCell::new(Context { heirarchy: vec![] });
}

fn with_context<F, R>(id: Id, f: F) -> R
where
    F: FnOnce() -> R,
{
    CONTEXT.with(|cx| {
        let mut cx = cx.borrow_mut();

        if let Some(_parent) = cx.heirarchy.last() {
            // todo! record parent -> id relationship
        }

        cx.heirarchy.push(id);

        let result = f();

        let _ignored = cx.heirarchy.pop();

        result
    })
}

// impl Context {
//     // fn current(&self) -> Id {
//     //     self.heirarchy.last().unwrap().clone()
//     // }

//     // fn with<T, F>(&self, id: Id, f: F)
//     // where
//     //     F: FnOnce(&Entry<T>),
//     // {
//     //     todo!()
//     // }
// }

// impl<T> Serialize for Entry<T> {
//     fn serialize(&self) {
//         with_context(self.id, || {
//             // todo! serialize
//         });
//     }
// }

impl<T> Entry<T> {
    pub fn new(value: T) -> Self {
        Self::new_raw(Id::new(), value)
    }

    pub(crate) fn new_raw(id: Id, value: T) -> Self {
        Self {
            id,
            item: Some(CacheEntry::Fresh(value)),
            meta: Some(CacheEntry::Fresh(Metadata {
                hash: [0; 32],
                created: 0,
                modified: 0,
                n_children: 0,
            })),
            children: Some(CacheEntry::Fresh(BTreeSet::new())),
        }
    }
}

impl<T> Drop for Entry<T> {
    fn drop(&mut self) {
        // if being flushed, and is dirty, write to storage
        // if not being flushed, delete from storage

        todo!()
    }
}

// A { B { C { D } } E }
// A
// A B
// A B C
// A B C D
// A E

mod thoughts {
    use std::collections::{BTreeMap, BTreeSet};
    use std::marker::PhantomData;

    //? only static data in state (so merkle_hash<T>:=hash( concat(encode<T>, child hashes (sorted by ID) ) )
    //? no exposure of lower-level detail in runtime (like hash)

    struct Id([u8; 32]);
    struct Hash([u8; 32]);
    trait StateEntry {
        fn flush(&self) {}
    }

    struct Child {
        id: Id,
        hash: [u8; 32],
    }

    struct Collection<T> {
        id: Id,
        // inner: Map<Id, Option<Entry<T>>>
        // modified: Set<Id>
        _priv: PhantomData<T>,
    }

    // lookup(Id)
    //    => Meta { id: Option<Id>, hash: [u8; 32], last_modified: u64, children: Child { id, hash } [] }
    //    => Data (Vec<u8>)
    // write(Id, T) where T: Encode
    //    =>

    // #[derive(StateEntry)]
    // #[on_change = "handle_change"]
    // struct Thing {
    //     a: OtherThing,
    // }

    // <T: Something>::{
    //    fn erase(&mut self) -> Result<()> {}
    //    fn flush(&mut self) -> Result<Option<Hash>> {
    //        // write to storage (meta & data)
    //    }
    //
    //    # subsciptions (called if any Something::flush returns a new hash)
    //    on_change: fn(&mut T) -> impl IntoResult,
    // }

    // fn handle_change(new: &mut Thing) {
    //     // if self.a.was_modified() {}
    //     // ...
    // }

    mod imp {
        use super::*;

        enum Entry {
            Meta {
                id: Id,
                hash: Hash,
                last_modified: u64,
                children: Vec<Child>,
            },
            Data(Vec<u8>),
        }

        struct Child {
            id: Id,
            hash: Hash,
        }

        pub fn lookup(id: Id) -> Option<Entry> {
            unimplemented!()
        }
    }

    // A -> B
    //  (id & hash & last_modified & children { id, hash }[])
    enum SyncArtifact {
        Meta(ArtifactMeta),
        Data(ArtifactData),
    }

    enum ArtifactMeta {
        Want(Id),
        Have {
            id: Id,
            hash: Hash,
            last_modified: u64,
            children: Vec<Child>,
        },
    }

    // struct AppState {
    //    a: Map<Key, Value>
    // }
    //
    // struct Value {
    //    b: List<String>
    // }
    //
    // { id: <root>, children: [<a>], data: [] }
    // { id:    <a>, children: [<b>], data: [] }
    // { id:    <b>, children: [<~1~>, <~2~>], data: [] }
    // { id:  <~1~>, children: [], data: ["hello"] }
    // { id:  <~2~>, children: [], data: ["world"] }
    //
    // A -- [Meta::Have { <root> }] -> B
    // B -- [
    //        # if B:<root> != A:<root>
    //          # if B:<root>:mtime > A:<root>:mtime
    //            Data::Have { <root>, <root>:data }
    //          # else if B:<root>:mtime < A:<root>:mtime
    //            Data::Want { <root> }
    //          # else
    //            Meta::Want { <root> }
    //          # fi
    //        # else if B:<a> != A:<a>
    //          Meta::Want {    <a> },
    //        # fi
    //      ] -> A

    enum ArtifactData {
        Want(Id),
        Have {
            id: Id,
            parent_id: Id,
            last_modified: u64,
            data: Vec<u8>,
        },
    }

    // B -> A
    //  if hash is different, decide based on last_modified
    //  if last_modified is equal,
    // meta sync // state sync

    // SyncEntry { id: Id, hash: [u8; 32], last_modified: u64, data: Vec<u8> }

    // id: Id,
    // data: Vec<u8>,
    // children: {id, hash}

    // struct Entry<T> {
    //     item: T,
    //     hash: [u8; 32],
    // }

    // collection.get_mut(id) -> EntryMut
    //     -> collection.cache should populate as CacheEntry::Clean
    // EntryMut: Drop
    //     -> collection.cache promote to CacheEntry::Dirty from CacheEntry::Clean
}
