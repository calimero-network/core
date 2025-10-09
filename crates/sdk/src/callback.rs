use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Mutex;

/// Registry for callback functions that handle events from other nodes
pub struct CallbackRegistry {
    callbacks: Mutex<HashMap<String, Vec<Box<dyn Fn(&serde_json::Value) + Send + Sync>>>>,
}

impl CallbackRegistry {
    pub fn new() -> Self {
        Self {
            callbacks: Mutex::new(HashMap::new()),
        }
    }

    pub fn register<E, F>(&self, event_kind: &str, callback: F)
    where
        E: for<'de> serde::Deserialize<'de> + 'static,
        F: Fn(E) + Send + Sync + 'static,
    {
        let wrapper: Box<dyn Fn(&serde_json::Value) + Send + Sync> =
            Box::new(move |value: &serde_json::Value| {
                match serde_json::from_value::<E>(value.clone()) {
                    Ok(typed) => callback(typed),
                    Err(err) => crate::env::log(&format!(
                        "Failed to deserialize event in callback: {:?}",
                        err
                    )),
                }
            });

        let mut callbacks = self.callbacks.lock().unwrap();
        callbacks.entry(event_kind.to_owned()).or_insert_with(Vec::new).push(wrapper);
    }

    pub fn handle_event_value(&self, event_value: &serde_json::Value) {
        if let Some(kind) = event_value.get("kind").and_then(|k| k.as_str()) {
            if let Some(callbacks) = self.callbacks.lock().unwrap().get(kind) {
                for callback in callbacks {
                    (callback)(event_value);
                }
            }
        }
    }
}

impl Default for CallbackRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for CallbackRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallbackRegistry")
            .field("callbacks", &"<callback functions>")
            .finish()
    }
}

/// Global callback registry instance
static CALLBACK_REGISTRY: std::sync::LazyLock<CallbackRegistry> =
    std::sync::LazyLock::new(|| CallbackRegistry::new());

/// Register a callback for an event type with borrowed data (like Event<'_>)
pub fn register_callback_borrowed<F>(event_kind: &str, callback: F)
where
    F: for<'a> Fn(&'a serde_json::Value) + Send + Sync + 'static,
{
    let wrapper = Box::new(move |event_value: &serde_json::Value| {
        callback(event_value);
    });
    
    let mut callbacks = CALLBACK_REGISTRY.callbacks.lock().unwrap();
    callbacks.entry(event_kind.to_owned()).or_insert_with(Vec::new).push(wrapper);
}

/// Handle an incoming event from another node
pub fn handle_incoming_event_value(value: &serde_json::Value) {
    CALLBACK_REGISTRY.handle_event_value(value);
}

/// Trait for types that can register callbacks
pub trait CallbackRegistryTrait {
    fn register_callbacks(&mut self);
}


// Thread-local mutable pointer to current app instance for callback dispatch
thread_local! {
    static APP_PTR: RefCell<*mut ()> = const { RefCell::new(std::ptr::null_mut()) };
}

/// Set the current app instance for callbacks. Caller must ensure lifetime covers callback dispatch scope.
pub fn set_current_app_mut<T>(app: &mut T) {
    APP_PTR.with(|slot| {
        let ptr = std::ptr::addr_of_mut!(*app);
        *slot.borrow_mut() = ptr.cast::<()>();
    });
}

/// With a mutable reference to the current app instance of type T, if set.
pub fn with_current_app_mut<T, R>(f: impl FnOnce(&mut T) -> R) -> Option<R> {
    APP_PTR.with(|slot| {
        let ptr = *slot.borrow();
        if ptr.is_null() {
            None
        } else {
            // SAFETY: set_current_app_mut guarantees ptr is a valid &mut T for the duration of dispatch
            let app = unsafe { &mut *(ptr as *mut T) };
            Some(f(app))
        }
    })
}

/// Register callbacks if the app implements CallbackRegistryTrait
pub fn register_callbacks_if_implemented<T>(app: &mut T) 
where
    T: CallbackRegistryTrait,
{
    app.register_callbacks();
}
