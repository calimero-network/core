use serde::{Deserialize, Serialize};

/// Application ID is a globally unique identifier of a application.
pub type AppId = u32;
// Installed application ID is a locally unique identifier of a running application instance.
pub type InstalledAppId = u32;

pub type AppBinary = Vec<u8>;

#[derive(Serialize, Deserialize, Debug)]
pub struct App {
    pub id: AppId,
    pub description: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct InstalledApp {
    pub id: InstalledAppId,
    pub app_id: AppId,
}
