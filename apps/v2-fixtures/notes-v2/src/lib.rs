//! Release 2.0.0 of the notes app the consent page tests upgrade (`crates/node/tests/consent_page.rs`),
//! from `apps/v2-fixtures/notes-v1`: `remove` is gone, `set` now takes a title and needs a
//! passkey confirmation, `wipe`, the public `exists` and the timer `tidy` are new, so is the
//! `Wiped` event, and the state is at version 2 with an `archive` table.
//! Method names and arguments are the ABI's camelCase ones.

use calimero_sdk::prelude::*;

#[derive(Record)]
#[table(key = "id")]
pub struct Note {
    pub id: String,
    pub text: String,
}

#[app::state(version = 2)]
pub struct Notes {
    notes: Table<Note>,
    archive: Table<Note>,
}

#[app::event]
pub enum Event {
    Changed { id: String },
    Wiped,
}

#[app::logic]
impl Notes {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// The note `id`, or `null`.
    #[app::view]
    pub fn get(&self, id: String) -> app::Result<Option<String>> {
        Ok(self.notes.get(&id)?.map(|n| n.text))
    }

    /// Writes the note `id` as `title` and `text`, with the owner's passkey.
    #[app::confirm]
    pub fn set(&mut self, id: String, title: String, text: String) -> app::Result<()> {
        self.notes.upsert(Note {
            id: id.clone(),
            text: format!("{title}\n{text}"),
        })?;
        env::emit(Event::Changed { id }, Audience::All)?;
        Ok(())
    }

    /// Moves the note `id` to the archive.
    #[app::confirm]
    pub fn wipe(&mut self, id: String) -> app::Result<()> {
        if let Some(note) = self.notes.get(&id)? {
            let _gone = self.notes.remove(&id)?;
            self.archive.upsert(note)?;
        }
        env::emit(Event::Wiped, Audience::All)?;
        Ok(())
    }

    /// Whether the note `id` exists; callable without signing in.
    #[app::view]
    #[app::public]
    pub fn exists(&self, id: String) -> app::Result<bool> {
        Ok(self.notes.get(&id)?.is_some())
    }

    /// Empties the archive's `tidy` slot once an hour.
    #[app::every("1h")]
    pub fn tidy(&mut self) -> app::Result<()> {
        let _gone = self.archive.remove("tidy")?;
        Ok(())
    }
}
