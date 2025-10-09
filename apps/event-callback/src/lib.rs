use calimero_sdk::app;
use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
use calimero_sdk::callback::CallbackRegistryTrait;

// Define events that can be emitted
#[app::event]
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub enum Event {
    UserRegistered {
        user_id: String,
        email: String,
    },
    OrderCreated {
        order_id: String,
        user_id: String,
        amount: u64,
    },
    UserLoggedIn {
        user_id: String,
    },
}

// Define the application state
#[app::state(emits = Event)]
#[derive(BorshSerialize, BorshDeserialize)]
#[borsh(crate = "calimero_sdk::borsh")]
pub struct AppState {
    pub users: std::collections::HashMap<String, String>, // user_id -> email
    pub orders: std::collections::HashMap<String, String>, // order_id -> user_id
}

// Define the application logic
#[app::logic]
impl AppState {
    #[app::init]
    pub fn init() -> Self {
        let mut app = Self {
            users: std::collections::HashMap::new(),
            orders: std::collections::HashMap::new(),
        };
        
        // Register callbacks
        app.register_callbacks();
        
        app
    }

    pub fn register_user(&mut self, user_id: &str, email: &str) -> app::Result<()> {
        app::log!("📝 REGISTER_USER: Starting registration for user_id={}, email={}", user_id, email);
        
        self.users.insert(user_id.to_string(), email.to_string());
        app::log!("📝 REGISTER_USER: User stored in local state");

        // Emit an event that will be propagated to other nodes
        app::log!("📤 EMITTING EVENT: UserRegistered for user_id={}, email={}", user_id, email);
        app::emit!(Event::UserRegistered { user_id: user_id.to_string(), email: email.to_string() });
        app::log!("📤 EVENT EMITTED: UserRegistered event sent");

        Ok(())
    }

    pub fn create_order(&mut self, order_id: &str, user_id: &str, amount: u64) -> app::Result<()> {
        self.orders
            .insert(order_id.to_string(), user_id.to_string());

        // Emit an event that will be propagated to other nodes
        app::emit!(Event::OrderCreated {
            order_id: order_id.to_string(),
            user_id: user_id.to_string(),
            amount
        });

        Ok(())
    }

    pub fn user_login(&mut self, user_id: &str) -> app::Result<()> {
        // Emit an event that will be propagated to other nodes
        app::emit!(Event::UserLoggedIn { user_id: user_id.to_string() });

        Ok(())
    }

    // Query methods for testing
    pub fn get_user_email(&self, user_id: &str) -> Option<String> {
        self.users.get(user_id).cloned()
    }

    pub fn get_order_user(&self, order_id: &str) -> Option<String> {
        self.orders.get(order_id).cloned()
    }

    pub fn get_user_count(&self) -> usize {
        self.users.len()
    }

    pub fn get_order_count(&self) -> usize {
        self.orders.len()
    }

    #[app::callback("UserRegistered")]
    pub fn on_user_registered(&mut self, event: Event) {
        app::log!("🔔 CALLBACK TRIGGERED: on_user_registered called with event: {:?}", event);
        match event {
            Event::UserRegistered { user_id, email } => {
                app::log!(
                    "Received user registration from another node: user_id={}, email={}",
                    user_id,
                    email
                );
                // Test state mutation - add a marker to show callback executed
                let callback_key = format!("callback_{}", user_id);
                self.users.insert(callback_key.clone(), "callback_executed".to_string());
                app::log!("Callback executed and state mutated! Added key: {}", callback_key);
                app::log!("Current users after callback: {:?}", self.users);
            }
            _ => {
                app::log!("❌ Unexpected event type in UserRegistered callback: {:?}", event);
            }
        }
    }

    // Callback for order creation events from other nodes
    #[app::callback("OrderCreated")]
    pub fn on_order_created(&mut self, event: Event) {
        match event {
            Event::OrderCreated {
                order_id,
                user_id,
                amount,
            } => {
                app::log!(
                    "Received order creation from another node: order_id={}, user_id={}, amount={}",
                    order_id,
                    user_id,
                    amount
                );
                // Here you could update analytics, send notifications, etc.
            }
            _ => {} // This should never happen since we're only called for OrderCreated events
        }
    }

    // Callback for user login events from other nodes
    #[app::callback("UserLoggedIn")]
    pub fn on_user_logged_in(&mut self, event: Event) {
        match event {
            Event::UserLoggedIn { user_id } => {
                app::log!("User logged in on another node: user_id={}", user_id);
                // Here you could update analytics, send notifications, etc.
            }
            _ => {} // This should never happen since we're only called for UserLoggedIn events
        }
    }
}

// Implement CallbackRegistryTrait for AppState
impl calimero_sdk::callback::CallbackRegistryTrait for AppState {
    fn register_callbacks(&mut self) {
        // Register callback for UserRegistered events
        calimero_sdk::callback::register_callback_borrowed(
            "UserRegistered",
            |event_value| {
                match calimero_sdk::serde_json::from_value::<Event>(event_value.clone()) {
                    Ok(event) => {
                        calimero_sdk::callback::with_current_app_mut(|app: &mut AppState| {
                            app.on_user_registered(event);
                        }).expect("Failed to get mutable app reference for callback");
                    }
                    Err(err) => {
                        calimero_sdk::env::log(&format!(
                            "Failed to deserialize UserRegistered event: {:?}",
                            err
                        ));
                    }
                }
            }
        );
        
        // Register callback for OrderCreated events
        calimero_sdk::callback::register_callback_borrowed(
            "OrderCreated",
            |event_value| {
                match calimero_sdk::serde_json::from_value::<Event>(event_value.clone()) {
                    Ok(event) => {
                        calimero_sdk::callback::with_current_app_mut(|app: &mut AppState| {
                            app.on_order_created(event);
                        }).expect("Failed to get mutable app reference for callback");
                    }
                    Err(err) => {
                        calimero_sdk::env::log(&format!(
                            "Failed to deserialize OrderCreated event: {:?}",
                            err
                        ));
                    }
                }
            }
        );
        
        // Register callback for UserLoggedIn events
        calimero_sdk::callback::register_callback_borrowed(
            "UserLoggedIn",
            |event_value| {
                match calimero_sdk::serde_json::from_value::<Event>(event_value.clone()) {
                    Ok(event) => {
                        calimero_sdk::callback::with_current_app_mut(|app: &mut AppState| {
                            app.on_user_logged_in(event);
                        }).expect("Failed to get mutable app reference for callback");
                    }
                    Err(err) => {
                        calimero_sdk::env::log(&format!(
                            "Failed to deserialize UserLoggedIn event: {:?}",
                            err
                        ));
                    }
                }
            }
        );
    }
}
