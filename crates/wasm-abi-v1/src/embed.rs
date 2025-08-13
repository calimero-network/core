use serde_json;

use crate::schema::Manifest;

/// Embed the ABI manifest into the WASM binary as a custom section
/// This macro generates the custom section and exports the get_abi* functions
#[macro_export]
macro_rules! embed_abi {
    ($manifest:expr) => {
        // Serialize the manifest to JSON
        const ABI_JSON: &str = $manifest;

        // Create the custom section with the ABI JSON
        #[cfg_attr(target_os = "macos", link_section = "__DATA,calimero_abi_v1")]
        #[cfg_attr(not(target_os = "macos"), link_section = "calimero_abi_v1")]
        static ABI: [u8; ABI_JSON.len()] = *ABI_JSON.as_bytes();

        // Export functions to access the ABI at runtime
        #[no_mangle]
        pub extern "C" fn get_abi_ptr() -> u32 {
            ABI.as_ptr() as u32
        }

        #[no_mangle]
        pub extern "C" fn get_abi_len() -> u32 {
            ABI.len() as u32
        }

        #[no_mangle]
        pub extern "C" fn get_abi() -> u32 {
            get_abi_ptr()
        }
    };
}

/// Embed ABI manifest into WASM binary
#[must_use]
pub fn embed(manifest: &Manifest) -> proc_macro2::TokenStream {
    let json = serde_json::to_string(manifest).expect("Failed to serialize manifest");
    let bytes = json.as_bytes();
    let bytes_list = bytes
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let json_len = bytes.len();

    format!(
        "// Generated ABI embed code\n// Create the custom section with the ABI JSON\n#[cfg_attr(target_arch = \"wasm32\", link_section = \".custom_section.calimero_abi_v1\")]\nstatic ABI_SECTION: [u8; {json_len}] = [{bytes_list}];\n"
    )
    .parse()
    .expect("Failed to parse generated code")
}

/// Generate embed code for build script
#[must_use]
pub fn generate_embed_code(manifest: &Manifest) -> String {
    let json = serde_json::to_string(manifest).expect("Failed to serialize manifest");
    let bytes = json.as_bytes();
    let bytes_list = bytes
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let json_len = bytes.len();

    format!(
        "// Generated ABI embed code\n// Create the custom section with the ABI JSON\n#[cfg_attr(target_arch = \"wasm32\", link_section = \".custom_section.calimero_abi_v1\")]\nstatic ABI_SECTION: [u8; {json_len}] = [{bytes_list}];\n"
    )
}
