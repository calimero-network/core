//! The cluster tests' key-value fixture on the v2 SDK (v1's `apps/kv-store`, cut to what the
//! cluster tests call). Method names and arguments are the ABI's camelCase ones.

use calimero_app_sdk::prelude::*;

#[derive(Record)]
#[table(key = "key")]
pub struct Entry {
    pub key: String,
    pub value: String,
}

/// A note, full-text searchable by title and body (spec section 7).
#[derive(Record)]
#[table(key = "id", search = ["title", "body"])]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
}

/// One tick of `beat`, in the order the ticks were applied.
#[derive(Record)]
#[table(key = "n")]
pub struct Beat {
    /// The tick's position, zero-padded so a scan returns ticks in order.
    pub n: String,
    pub scheduled: u64,
    /// `0`: the timer's first tick.
    pub previous: u64,
    /// The entry timestamp the tick ran at.
    pub at: u64,
}

/// A `beat` tick as `beats` lists it (D20: 64-bit integers are decimal strings).
#[derive(serde::Serialize)]
pub struct BeatOut {
    pub scheduled: String,
    pub previous: Option<String>,
    pub at: String,
}

#[app::state(version = 1)]
pub struct Kv {
    entries: Table<Entry>,
    size: Counter,
    beats: Table<Beat>,
    beat_count: Counter,
    /// `failUntil` and `stallUntil`, app times in ms as decimal strings.
    beat_faults: UnorderedMap<String, String>,
    /// Uploaded blobs by name (R8): a typed reference keeps the blob alive.
    files: UnorderedMap<String, BlobRef>,
    notes: Table<Note>,
}

/// A file as `file` lists it (D20: the size as a decimal string).
#[derive(serde::Serialize)]
pub struct FileOut {
    pub id: String,
    pub size: String,
    pub mime: String,
}

#[app::error]
pub enum Error {
    NotFound { key: String },
    BeatFailed,
}

#[app::event]
pub enum Event {
    Inserted { key: String, value: String },
    Updated { key: String, value: String },
    Removed { key: String },
    Cleared,
}

/// The most entries one `entries` call lists.
const LIST_LIMIT: u32 = 1000;
/// `beat`'s interval: long, so the other cluster tests see no ticks; timer tests move
/// the nodes' app clocks forward instead.
const BEAT_MS: u64 = 3_600_000;

#[app::logic]
#[allow(clippy::len_without_is_empty)]
impl Kv {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// Sets `key`; emits `Inserted` for a new key, `Updated` for an existing one.
    pub fn set(&mut self, key: String, value: String) -> app::Result<()> {
        let event = self.upsert(&key, &value)?;
        env::emit(event, Audience::All)?;
        Ok(())
    }

    /// Sets `key` without an event (for values over the 16 KiB event limit).
    pub fn put(&mut self, key: String, value: String) -> app::Result<()> {
        let _event = self.upsert(&key, &value)?;
        Ok(())
    }

    /// Writes `n` keys (`fill-0` ..) of `size` bytes each, one `Inserted` event per new key.
    pub fn fill(&mut self, n: u32, size: u32) -> app::Result<()> {
        let value = "x".repeat(size as usize);
        for i in 0..n {
            let event = self.upsert(&format!("fill-{i}"), &value)?;
            env::emit(event, Audience::All)?;
        }
        Ok(())
    }

    /// Never returns: runs until the node's fuel or wall-clock budget stops it.
    pub fn spin(&mut self) -> app::Result<()> {
        let mut n: u64 = 0;
        loop {
            n = std::hint::black_box(n.wrapping_add(1));
        }
    }

    /// The entry timestamp this call ran at (D19); writes nothing.
    pub fn stamp(&mut self) -> app::Result<u64> {
        Ok(env::time_now())
    }

    /// The time a view sees: the last applied entry's timestamp (D19).
    #[app::view]
    pub fn time_now(&self) -> app::Result<u64> {
        Ok(env::time_now())
    }

    /// A fresh unique id (D52); writes nothing.
    pub fn new_id(&mut self) -> app::Result<String> {
        Ok(env::unique_id())
    }

    /// A timer (D34): records the tick's slot and the previous one. A tick fails while
    /// its slot is within a `failBeats` window, and spins (until the node's deadline)
    /// while its entry time is within a `stallBeats` window.
    #[app::every("1h")]
    pub fn beat(&mut self) -> app::Result<()> {
        let Some(tick) = env::timer() else {
            app::bail!(Error::BeatFailed);
        };
        let now = env::time_now();
        if now < self.fault("stallUntil")? {
            let mut n: u64 = 0;
            loop {
                n = std::hint::black_box(n.wrapping_add(1));
            }
        }
        if tick.scheduled <= self.fault("failUntil")? {
            app::bail!(Error::BeatFailed);
        }
        let n = self.beat_count.increment()?;
        self.beats.upsert(Beat {
            n: format!("{n:020}"),
            scheduled: tick.scheduled,
            previous: tick.previous.unwrap_or(0),
            at: now,
        })?;
        Ok(())
    }

    /// Every `beat` tick applied, oldest first (up to 1000).
    #[app::view]
    pub fn beats(&self) -> app::Result<Vec<BeatOut>> {
        let page = self.beats.scan(FindOpts::default().limit(LIST_LIMIT))?;
        Ok(page
            .items
            .into_iter()
            .map(|b| BeatOut {
                scheduled: b.scheduled.to_string(),
                previous: (b.previous != 0).then(|| b.previous.to_string()),
                at: b.at.to_string(),
            })
            .collect())
    }

    /// Makes the `beat` ticks of the next `n` slots (from this call's time) fail.
    pub fn fail_beats(&mut self, n: u32) -> app::Result<()> {
        let until = env::time_now() + u64::from(n) * BEAT_MS;
        self.beat_faults
            .insert(&"failUntil".to_owned(), &until.to_string())?;
        Ok(())
    }

    /// Makes `beat` ticks that run within `ms` of this call's time spin until they time
    /// out; returns the app time the window ends at.
    pub fn stall_beats(&mut self, ms: u64) -> app::Result<u64> {
        let until = env::time_now() + ms;
        self.beat_faults
            .insert(&"stallUntil".to_owned(), &until.to_string())?;
        Ok(until)
    }

    /// Names an uploaded blob `name`: state refers to it from now on.
    pub fn attach(&mut self, name: String, blob: BlobRef) -> app::Result<()> {
        self.files.insert(&name, &blob)?;
        Ok(())
    }

    /// Drops the name; the blob goes once nothing refers to it.
    pub fn detach(&mut self, name: String) -> app::Result<bool> {
        Ok(self.files.remove(&name)?.is_some())
    }

    /// The blob named `name`, with what the node recorded of it; `null` if the name is
    /// unknown or the blob was deleted.
    #[app::view]
    pub fn file(&self, name: String) -> app::Result<Option<FileOut>> {
        let Some(blob) = self.files.get(&name)? else {
            return Ok(None);
        };
        Ok(blob.info().map(|i| FileOut {
            id: blob.id(),
            size: i.size.to_string(),
            mime: i.mime,
        }))
    }

    /// A download grant for the blob named `name`, valid `ttl` seconds.
    #[app::view]
    pub fn share(&self, name: String, ttl: u32) -> app::Result<Option<String>> {
        Ok(self.files.get(&name)?.map(|b| b.grant(ttl)))
    }

    /// Writes note `id`.
    pub fn note(&mut self, id: String, title: String, body: String) -> app::Result<()> {
        self.notes.upsert(Note { id, title, body })?;
        Ok(())
    }

    /// Removes note `id`.
    pub fn drop_note(&mut self, id: String) -> app::Result<bool> {
        Ok(self.notes.remove(&id)?)
    }

    /// The ids of the notes matching `text`, best first.
    #[app::view]
    pub fn find_notes(&self, text: String) -> app::Result<Vec<String>> {
        let found = self.notes.search(&text, SearchOpts::default().limit(50))?;
        Ok(found.items.into_iter().map(|f| f.record.id).collect())
    }

    /// The value of `key`, or `null`.
    #[app::view]
    pub fn get(&self, key: String) -> app::Result<Option<String>> {
        Ok(self.entries.get(&key)?.map(|e| e.value))
    }

    /// The value of `key`; `NotFound` when absent.
    #[app::view]
    pub fn get_result(&self, key: String) -> app::Result<String> {
        match self.entries.get(&key)? {
            Some(e) => Ok(e.value),
            None => Err(Error::NotFound { key }.into()),
        }
    }

    /// Every entry (up to 1000).
    #[app::view]
    pub fn entries(&self) -> app::Result<std::collections::BTreeMap<String, String>> {
        let page = self.entries.scan(FindOpts::default().limit(LIST_LIMIT))?;
        Ok(page.items.into_iter().map(|e| (e.key, e.value)).collect())
    }

    /// How many keys there are.
    #[app::view]
    pub fn len(&self) -> app::Result<u32> {
        Ok(u32::try_from(self.size.get()?).unwrap_or(u32::MAX))
    }

    /// Removes `key`; returns the value it had.
    pub fn remove(&mut self, key: String) -> app::Result<Option<String>> {
        let Some(old) = self.entries.get(&key)? else {
            return Ok(None);
        };
        self.entries.remove(&key)?;
        self.size.add(-1)?;
        env::emit(Event::Removed { key }, Audience::All)?;
        Ok(Some(old.value))
    }

    /// Removes every entry. Needs a fresh passkey confirmation.
    #[app::confirm]
    pub fn clear(&mut self) -> app::Result<()> {
        self.remove_all()
    }

    /// The same as `clear`, under the brief's name.
    #[app::confirm]
    pub fn wipe(&mut self) -> app::Result<()> {
        self.remove_all()
    }

    fn upsert(&mut self, key: &str, value: &str) -> app::Result<Event> {
        let fresh = !self.entries.contains(key)?;
        self.entries.upsert(Entry {
            key: key.to_owned(),
            value: value.to_owned(),
        })?;
        if fresh {
            self.size.add(1)?;
            Ok(Event::Inserted {
                key: key.to_owned(),
                value: value.to_owned(),
            })
        } else {
            Ok(Event::Updated {
                key: key.to_owned(),
                value: value.to_owned(),
            })
        }
    }

    fn fault(&self, name: &str) -> app::Result<u64> {
        Ok(self
            .beat_faults
            .get(&name.to_owned())?
            .and_then(|v| v.parse().ok())
            .unwrap_or(0))
    }

    fn remove_all(&mut self) -> app::Result<()> {
        loop {
            let page = self.entries.scan(FindOpts::default().limit(LIST_LIMIT))?;
            if page.items.is_empty() {
                break;
            }
            for e in page.items {
                self.entries.remove(&e.key)?;
                self.size.add(-1)?;
            }
        }
        env::emit(Event::Cleared, Audience::All)?;
        Ok(())
    }
}
