use std::time::Duration;

use color_eyre::eyre;

use calimero_primitives::app;
use tracing::info;

pub async fn list_remote_apps() -> eyre::Result<Vec<calimero_primitives::app::App>> {
    Ok(vec![
        app::App {
            id: 1000,
            description: "Chat".to_string(),
        },
        app::App {
            id: 2000,
            description: "Forum".to_string(),
        },
    ])
}

pub async fn list_installed_apps() -> eyre::Result<Vec<calimero_primitives::app::InstalledApp>> {
    Ok(vec![
        app::InstalledApp {
            id: 1,
            app_id: 1000,
        },
        app::InstalledApp {
            id: 2000,
            app_id: 1000,
        },
    ])
}

pub async fn install_binary_app(_: app::AppBinary) -> eyre::Result<app::InstalledAppId> {
    info!("installing app binary...");
    tokio::time::sleep(Duration::from_secs(10)).await;
    info!("installation complete");
    Ok(rand::random())
}

pub async fn install_remote_app(_: app::AppId) -> eyre::Result<app::InstalledAppId> {
    info!("installing app from remote store...");
    tokio::time::sleep(Duration::from_secs(20)).await;
    info!("installation complete");
    Ok(rand::random())
}

pub async fn uninstall_app(id: app::InstalledAppId) -> eyre::Result<app::InstalledAppId> {
    info!("installing app binary...");
    tokio::time::sleep(Duration::from_secs(5)).await;
    info!("installation complete");
    Ok(id)
}
