use calimero_sdk::app;
use calimero_sdk::serde::Serialize;
use calimero_storage::collections::{LwwRegister, UnorderedSet};

const SCHEMA_VERSION_V1: &str = "1.0.0";

/// v1 state for the unordered-set migration scenario. `tags` is a plain
/// content-addressed `UnorderedSet` (no executor identity in its element ids).
#[app::state(emits = for<'a> Event<'a>)]
pub struct ScenarioUnorderedSetV1 {
    tags: UnorderedSet<String>,
    title: LwwRegister<String>,
}

#[app::event]
pub enum Event<'a> {
    TagAdded { tag: &'a str },
}

#[derive(Debug, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct SchemaInfo {
    pub schema_version: String,
    pub title: String,
    pub tag_count: u64,
}

#[app::logic]
impl ScenarioUnorderedSetV1 {
    #[app::init]
    pub fn init() -> ScenarioUnorderedSetV1 {
        ScenarioUnorderedSetV1 {
            tags: UnorderedSet::new(),
            title: LwwRegister::new("untitled".to_owned()),
        }
    }

    pub fn set_title(&mut self, title: String) -> app::Result<()> {
        self.title.set(title);
        Ok(())
    }

    pub fn add_tag(&mut self, tag: String) -> app::Result<bool> {
        let inserted = self.tags.insert(tag.clone())?;
        app::emit!(Event::TagAdded { tag: &tag });
        Ok(inserted)
    }

    pub fn has_tag(&self, tag: String) -> app::Result<bool> {
        Ok(self.tags.contains(&tag)?)
    }

    pub fn tag_count(&self) -> app::Result<u64> {
        Ok(self.tags.len()? as u64)
    }

    pub fn schema_info(&self) -> app::Result<SchemaInfo> {
        Ok(SchemaInfo {
            schema_version: SCHEMA_VERSION_V1.to_owned(),
            title: self.title.get().clone(),
            tag_count: self.tags.len()? as u64,
        })
    }
}
