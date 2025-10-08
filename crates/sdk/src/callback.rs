use std::collections::HashMap;
use std::sync::Mutex;

use crate::event::AppEventExt;

/// Registry for callback functions that handle events from other nodes
pub struct CallbackRegistry {
    callbacks: Mutex<HashMap<String, Box<dyn Fn(Box<dyn AppEventExt>) + Send + Sync>>>,
}

impl CallbackRegistry {
    pub fn new() -> Self {
        Self {
            callbacks: Mutex::new(HashMap::new()),
        }
    }

    pub fn register<E: AppEventExt + 'static, F: Fn(E) + Send + Sync + 'static>(
        &self,
        event_kind: &str,
        callback: F,
    ) {
        let callback = Box::new(move |event: Box<dyn AppEventExt>| {
            if let Ok(typed_event) = E::downcast(event) {
                callback(typed_event);
            }
        });

        self.callbacks
            .lock()
            .unwrap()
            .insert(event_kind.to_string(), callback);
    }

    pub fn handle_event(&self, event: Box<dyn AppEventExt>) {
        let event_kind = event.kind();
        if let Some(callback) = self.callbacks.lock().unwrap().get(&event_kind.to_string()) {
            callback(event);
        }
    }
}

impl Default for CallbackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Global callback registry instance
static CALLBACK_REGISTRY: std::sync::LazyLock<CallbackRegistry> = std::sync::LazyLock::new(|| CallbackRegistry::new());

/// Register a callback for a specific event type
pub fn register_callback<E: AppEventExt + 'static, F: Fn(E) + Send + Sync + 'static>(
    event_kind: &str,
    callback: F,
) {
    CALLBACK_REGISTRY.register(event_kind, callback);
}

/// Handle an incoming event from another node
pub fn handle_incoming_event(event: Box<dyn AppEventExt>) {
    CALLBACK_REGISTRY.handle_event(event);
}

