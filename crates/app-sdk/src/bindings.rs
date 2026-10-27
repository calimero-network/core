//! Guest bindings for the imports of the `calimero:app` world.
//!
//! The `handle` export is declared here but exported only by the code `#[app::logic]`
//! generates, through `__calimero_export_handle!` (see `__private::Export`).

wit_bindgen::generate!({
    world: "app",
    path: "../app-wit/wit",
    generate_all,
    pub_export_macro: true,
    export_macro_name: "__calimero_export_handle",
    default_bindings_module: "::calimero_app_sdk::__private::bindings",
});
