//! Release 1.0.0 of the notes app the consent page tests upgrade (`crates/node/tests/consent_page.rs`).
//! `apps/v2-fixtures/notes-v2` is 2.0.0; the page names what changes between their ABIs.
//! Method names and arguments are the ABI's camelCase ones.

use calimero_sdk::prelude::*;

#[derive(Record)]
#[table(key = "id")]
pub struct Note {
    pub id: String,
    pub text: String,
}

#[app::state(version = 1)]
pub struct Notes {
    notes: Table<Note>,
}

#[app::event]
pub enum Event {
    Changed { id: String },
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

    /// Writes the note `id`.
    pub fn set(&mut self, id: String, text: String) -> app::Result<()> {
        self.notes.upsert(Note {
            id: id.clone(),
            text,
        })?;
        env::emit(Event::Changed { id }, Audience::All)?;
        Ok(())
    }

    /// Removes the note `id` (gone in 2.0.0).
    pub fn remove(&mut self, id: String) -> app::Result<()> {
        let _gone = self.notes.remove(&id)?;
        env::emit(Event::Changed { id }, Audience::All)?;
        Ok(())
    }
}
