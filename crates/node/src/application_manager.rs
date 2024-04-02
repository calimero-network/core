use std::collections::HashMap;
use std::fs;

use calimero_network::client::NetworkClient;
use camino::Utf8PathBuf;
use tracing::info;

#[derive(Clone)]
pub struct Application {
    pub name: String,
    pub path: Utf8PathBuf,
}

pub(crate) struct ApplicationManager {
    pub network_client: NetworkClient,
    pub applications: HashMap<calimero_primitives::application::ApplicationId, Application>,
}

impl ApplicationManager {
    pub fn new(network_client: NetworkClient) -> Self {
        Self {
            network_client,
            applications: HashMap::default(),
        }
    }

    pub async fn register_application(&mut self, application: Application) -> eyre::Result<()> {
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

        self.applications.insert(
            calimero_primitives::application::ApplicationId(app_topic.clone().into_string()),
            application.clone(),
        );

        info!(
            "Registered application {} with hash: {}",
            application.name, app_topic
        );

        Ok(())
    }

    // unused ATM, uncomment when used
    // pub fn get_registered_applications(
    //     &self,
    // ) -> Vec<&calimero_primitives::application::ApplicationId> {
    //     Vec::from_iter(self.applications.keys())
    // }

    pub fn is_application_registered(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> bool {
        self.applications.contains_key(application_id)
    }

    pub fn load_application_blob(
        &self,
        application_id: &calimero_primitives::application::ApplicationId,
    ) -> eyre::Result<Vec<u8>> {
        match self.applications.get(application_id) {
            Some(application) => Ok(fs::read(&application.path)?),
            None => eyre::bail!(
                "failed to get application with id: {}",
                application_id.clone().to_string()
            ),
        }
    }
}
