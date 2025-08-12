use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::serde::{Deserialize, Serialize};
use calimero_sdk::{app, env};
use calimero_storage::collections::{UnorderedMap, Vector};
use thiserror::Error;
use types::id;
mod types;

id::define!(pub UserId<32, 44>);

#[app::event]
pub enum Event {
    CalendarEventCreated(String),
    CalendarEventEdited(String),
    CalendarEventDeleted(String),
}

// state
#[app::state(emits = Event)]
#[derive(Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct CalendarState {
    // Key is event.id and the value is event
    events: UnorderedMap<String, CalendarEventState>,
}

#[derive(Debug, PartialEq, PartialOrd, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct CalendarEventState {
    title: String,
    description: String,
    owner: UserId,
    start: String,
    end: String,
    event_type: String,
    color: String,
    peers: Vector<UserId>,
}

// request/response
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct ExecutorId([u8; 32]);

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[app::abi_type]
pub struct CalendarEvent {
    id: String,
    title: String,
    description: String,
    owner: UserId,
    start: String,
    end: String,
    event_type: String,
    color: String,
    peers: Vec<UserId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
#[app::abi_type]
pub struct CreateCalendarEvent {
    title: String,
    description: String,
    start: String,
    end: String,
    event_type: String,
    color: String,
    peers: Vec<UserId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "calimero_sdk::serde")]
pub struct UpdateCalendarEvent {
    title: Option<String>,
    description: Option<String>,
    start: Option<String>,
    end: Option<String>,
    event_type: Option<String>,
    color: Option<String>,
    peers: Option<Vec<UserId>>,
}

#[derive(Debug, Error, Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum Error {
    #[error("key not found: {0}")]
    NotFound(String),
    #[error("operation forbiden")]
    Forbidden,
}

#[app::logic]
impl CalendarState {
    #[app::init]
    pub fn init() -> CalendarState {
        CalendarState {
            events: UnorderedMap::new(),
        }
    }

    pub fn get_events(&self) -> app::Result<Vec<CalendarEvent>> {
        let executor_id = self.get_executor_id();

        let mut events = Vec::new();
        for (id, event) in self.events.entries()? {
            if !(event.owner == executor_id || event.peers.contains(&executor_id)?) {
                continue;
            }

            let mut peers = Vec::new();
            for peer in event.peers.iter()? {
                peers.push(peer);
            }
            events.push(CalendarEvent {
                id,
                title: event.title,
                description: event.description,
                owner: event.owner,
                start: event.start,
                end: event.end,
                event_type: event.event_type,
                color: event.color,
                peers,
            });
        }

        Ok(events)
    }

    pub fn create_event(&mut self, event_data: CreateCalendarEvent) -> app::Result<String> {
        app::log!("Creating calendar event {:?}", event_data);

        let id = self.generate_id();
        let executor_id = self.get_executor_id();

        let mut peers = Vector::new();
        for peer in event_data.peers {
            peers.push(peer)?;
        }

        let event = CalendarEventState {
            title: event_data.title,
            description: event_data.description,
            owner: executor_id,
            start: event_data.start,
            end: event_data.end,
            event_type: event_data.event_type,
            color: event_data.color,
            peers,
        };

        self.events.insert(id.clone(), event)?;

        app::emit!(Event::CalendarEventCreated(id.clone()));

        Ok(id)
    }

    pub fn update_event(
        &mut self,
        event_id: String,
        event_data: UpdateCalendarEvent,
    ) -> app::Result<String> {
        app::log!("Updating calendar event {} with {:?}", event_id, event_data);

        let Some(mut event) = self.events.get(&event_id)? else {
            app::bail!(Error::NotFound(event_id));
        };

        let executor_id = self.get_executor_id();
        if event.owner != executor_id {
            app::bail!(Error::Forbidden)
        }

        if let Some(data) = event_data.title {
            event.title = data
        }
        if let Some(data) = event_data.description {
            event.description = data
        }
        if let Some(data) = event_data.start {
            event.start = data
        }
        if let Some(data) = event_data.end {
            event.end = data
        }
        if let Some(data) = event_data.event_type {
            event.event_type = data
        }
        if let Some(data) = event_data.color {
            event.color = data
        }
        if let Some(data) = event_data.peers {
            event.peers.clear()?;

            for peer in data {
                event.peers.push(peer)?;
            }
        }

        self.events.insert(event_id.clone(), event)?;

        app::emit!(Event::CalendarEventCreated(event_id.clone()));

        Ok(event_id)
    }

    pub fn delete_event(&mut self, event_id: String) -> app::Result<String> {
        app::log!("Deleting calendar event {}", event_id);

        let executor_id = self.get_executor_id();

        let Some(event) = self.events.get(&event_id)? else {
            app::bail!(Error::NotFound(event_id));
        };

        if event.owner != executor_id {
            app::bail!(Error::Forbidden)
        }

        if self.events.remove(&event_id)?.is_none() {
            app::bail!(Error::NotFound(event_id));
        }

        Ok(event_id)
    }

    fn get_executor_id(&self) -> UserId {
        UserId::new(env::executor_id())
    }

    fn generate_id(&self) -> String {
        let mut buffer = [0u8; 16];
        env::random_bytes(&mut buffer);
        STANDARD.encode(&buffer)
    }
}
