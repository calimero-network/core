use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct InstallApplicationRequest {
    pub application: calimero_primitives::application::ApplicationId, // TODO: rename to application_id
    pub version: semver::Version,
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ApplicationListResult {
    pub apps: Vec<calimero_primitives::application::Application>,
}
