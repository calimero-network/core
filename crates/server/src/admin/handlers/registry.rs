pub mod install_app_from_registry;
pub mod list_apps_from_registry;
pub mod list_registries;
pub mod remove_registry;
pub mod setup_registry;
pub mod uninstall_app_from_registry;
pub mod update_app_from_registry;

// Re-export handler functions
pub use install_app_from_registry::handler as install_app_from_registry;
pub use list_apps_from_registry::handler as list_apps_from_registry;
pub use list_registries::handler as list_registries;
pub use remove_registry::handler as remove_registry;
pub use setup_registry::handler as setup_registry;
pub use uninstall_app_from_registry::handler as uninstall_app_from_registry;
pub use update_app_from_registry::handler as update_app_from_registry;
