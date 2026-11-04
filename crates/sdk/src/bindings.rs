//! Guest bindings for the imports of the `calimero:app` world, or of the `compute` world in a
//! build with `--cfg calimero_compute`.
//!
//! The `handle` (or `run`) export is declared here but exported only by the code
//! `#[app::logic]` generates, through `__calimero_export_handle!` (`__calimero_export_run!`;
//! see `__private::Export`).

#[cfg(not(calimero_compute))]
wit_bindgen::generate!({
    world: "app",
    path: "../app-wit/wit",
    generate_all,
    pub_export_macro: true,
    export_macro_name: "__calimero_export_handle",
    default_bindings_module: "::calimero_sdk::__private::bindings",
});

#[cfg(calimero_compute)]
wit_bindgen::generate!({
    world: "compute",
    path: "../app-wit/wit",
    generate_all,
    pub_export_macro: true,
    export_macro_name: "__calimero_export_run",
    default_bindings_module: "::calimero_sdk::__private::bindings",
});
