use std::collections::HashMap;
use std::fs;

use calimero_network::client::NetworkClient;
use camino::Utf8PathBuf;
use libp2p::gossipsub::TopicHash;
use tracing::info;

#[derive(Clone)]
pub struct Application {
    pub name: String,
    pub path: Utf8PathBuf,
}

pub(crate) struct ApplicationManager {
    pub network_client: NetworkClient,
    pub applications: HashMap<TopicHash, Application>,
}

impl ApplicationManager {
    pub fn new(network_client: NetworkClient) -> Self {
        Self {
            network_client: network_client,
            applications: HashMap::default(),
        }
    }

    pub async fn register_application(&mut self, application: Application) {
        let app_blob = fs::read(&application.path).unwrap();
        let app_topic = self
            .network_client
            .subscribe(calimero_network::types::IdentTopic::new(format!(
                "/calimero/experimental/app/{}",
                calimero_primitives::hash::Hash::hash(&app_blob),
            )))
            .await
            .unwrap()
            .hash();

        self.applications
            .insert(app_topic.clone(), application.clone());

        info!(
            "Registered application {} with hash: {}",
            application.name, app_topic
        );
    }

    pub fn get_registered_applications(&self) -> Vec<&TopicHash> {
        Vec::from_iter(self.applications.keys())
    }

    pub fn is_application_registered(&self, application_id: TopicHash) -> bool {
        self.applications.contains_key(&application_id)
    }

    pub fn load_application_blob(&self, application_id: TopicHash) -> Vec<u8> {
        fs::read(&self.applications.get(&application_id).unwrap().path).unwrap()
    }
}
