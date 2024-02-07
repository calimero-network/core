use serde::{Deserialize, Serialize};

use crate::commands::AppId;

#[derive(Serialize, Deserialize, Debug)]
pub struct App {
    id: AppId,
    name: String,
    description: String,
    tags: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct AppList {
    apps: Vec<App>,
}

pub async fn list_apps() -> Result<Vec<App>, reqwest::Error> {
    let resp = reqwest::get("http://127.0.0.1:3000/apps")
        .await?
        .json::<Vec<App>>()
        .await?;

    return Ok(resp);
}
