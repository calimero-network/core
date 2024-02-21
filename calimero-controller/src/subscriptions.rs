use std::collections::{HashMap, HashSet};

pub(crate) struct Subscriptions {
    /// Key is app_id, value is set of clients to which client is subscribed.
    app_to_clients: HashMap<
        calimero_primitives::app::InstalledAppId,
        HashSet<calimero_primitives::api::WsClientId>,
    >,
    /// Key is client_id, value is set of app_ids to which client is subscribed.
    client_to_apps: HashMap<
        calimero_primitives::api::WsClientId,
        HashSet<calimero_primitives::app::InstalledAppId>,
    >,
}

impl Subscriptions {
    pub(crate) fn new() -> Self {
        Self {
            app_to_clients: HashMap::new(),
            client_to_apps: HashMap::new(),
        }
    }

    pub(crate) fn subscribe(
        &mut self,
        app_id: calimero_primitives::app::InstalledAppId,
        client_id: calimero_primitives::api::WsClientId,
    ) {
        self.app_to_clients
            .entry(app_id)
            .or_insert_with(HashSet::new)
            .insert(client_id);
        self.client_to_apps
            .entry(client_id)
            .or_insert_with(HashSet::new)
            .insert(app_id);
    }

    pub(crate) fn unsubscribe(
        &mut self,
        app_id: calimero_primitives::app::InstalledAppId,
        client_id: calimero_primitives::api::WsClientId,
    ) {
        self.app_to_clients
            .get_mut(&app_id)
            .map(|set| set.remove(&client_id));
        self.client_to_apps
            .get_mut(&client_id)
            .map(|set| set.remove(&app_id));
    }

    pub(crate) fn unsubscribe_from_all(&mut self, client_id: calimero_primitives::api::WsClientId) {
        // remove client_id from all apps
        if let Some(app_ids) = self.client_to_apps.get(&client_id) {
            app_ids.iter().for_each(|app_id| {
                self.app_to_clients
                    .get_mut(app_id)
                    .map(|set| set.remove(&client_id));
            });
        }
        self.client_to_apps.remove(&client_id);
    }

    // fn get_subscribed_clients(&self, app_id: InstalledAppId) -> Option<&HashSet<WsClientId>> {
    //     self.app_to_clients.get(&app_id)
    // }
}
